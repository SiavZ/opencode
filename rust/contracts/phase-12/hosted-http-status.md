# Hosted HTTP transport status

Status: implemented as an explicit Rust JSON transport; baseline parity is partial and browser certification is outstanding.

Owner: Phase 12 hosted console HTTP review.

## Scope and transport boundary

The Rust transport is composed by `rust/bins/hosted/src/console/mod.rs` and is implemented in:

- `rust/bins/hosted/src/console/auth.rs`
- `rust/bins/hosted/src/console/auth/request.rs`
- `rust/bins/hosted/src/console/auth/session.rs`
- `rust/bins/hosted/src/console/workspace.rs`

These endpoints are explicit Axum JSON endpoints. They are not claimed to implement SolidStart server-function wire compatibility. The browser routes and server functions under `packages/console/app/src/routes` remain the behavior baseline.

## Configuration

`rust/bins/hosted/src/console/auth.rs` reads the first non-empty value from each listed pair:

| Environment variable | Fallback | Meaning |
| --- | --- | --- |
| `OPENCODE_CONSOLE_AUTH_URL` | `VITE_AUTH_URL` | OpenAuth issuer base URL; must be `http` or `https` with a host. |
| `OPENCODE_CONSOLE_SESSION_SECRET` | `ZEN_SESSION_SECRET` | Password for the sealed `auth` session cookie. |
| `OPENCODE_CONSOLE_MIGRATION_URL` | `CONSOLE_MIGRATION_URL` | Destination for migrated/no-Black workspaces; redirects to `${value}/login`. |
| `OPENCODE_CONSOLE_PUBLIC_ORIGIN` | none | Trusted external origin for reverse-proxied URL construction and same-origin checks. It must have only scheme, host, optional port, and `/` path, with no credentials, query, or fragment. |

`OPENCODE_STAGE` is read by `ConsoleConfig` and defaults to `dev`; it is consumed by other console modules. The auth session cookie is named `auth`, is `HttpOnly`, has `Path=/`, has a one-year lifetime, and intentionally has no `Secure` attribute to match the TypeScript baseline.

When `OPENCODE_CONSOLE_PUBLIC_ORIGIN` is configured, it is used instead of request `Host` and forwarding headers. Without it, the transport accepts direct HTTP requests and uses `Host`; it does not trust `x-forwarded-proto`. Production deployments should configure the public origin and ensure the proxy boundary is controlled.

## Auth endpoints

All paths below are relative to the hosted service origin. Successful session-bearing responses may emit a refreshed `Set-Cookie: auth=...` header.

| Method and path | Request | Response contract |
| --- | --- | --- |
| `GET /auth/authorize` | Optional query `continue`; when present it must be a local absolute path beginning with one `/`, not `//`, backslash, or control characters. | `302 Location: <issuer>/authorize?...` with `client_id=app`, `redirect_uri=<origin>/auth/callback<continue>`, `response_type=code`, and a generated UUID `state`. |
| `GET /auth/callback` and `GET /auth/callback/{path}` | OAuth query `code` is required. | Exchanges the code at `<issuer>/token` using the URL-encoded authorization-code fields, decodes the account subject, updates the sealed session, and returns `302` to `/auth` or the callback suffix. Locale selection may prefix supported paths according to request locale. Missing/invalid callback data returns `500` with a JSON-formatted body containing `error` and the received query `cause`; the current handler constructs a string response, so `application/json` content-type parity is not certified. |
| `GET /auth/status` | Optional sealed `auth` cookie. Missing or invalid cookies are treated as an empty session. Duplicate `auth` cookies return `400`. | `200` JSON session data (`account` map and optional `current`) plus a refreshed sealed cookie. |
| `GET /auth/logout` | Optional sealed `auth` cookie. | Removes the current account, selects the first remaining account, and returns `302 Location: /zen` plus the refreshed cookie. |
| `GET /auth` | Optional sealed session. | Last-seen live, non-migrated workspace redirects `302` to `/workspace/{id}`. No eligible workspace redirects `302` to the configured migration console `/login`. Resolver/database failures fall back to `302 /auth/authorize`, matching the baseline catch behavior. |
| `POST /auth/action/actor` | JSON `{ "workspaceID": string | null }`; `Origin` and `Sec-Fetch-Site`, when supplied, must be same-origin. Non-browser clients may omit `Origin`. | Resolves the account or workspace actor as JSON and may refresh the session cookie. Missing workspace membership redirects to `/auth/authorize`; no Black membership redirects to the migration console. Migrated workspace redirects with `303` for this POST endpoint, with `Cache-Control: no-store` (the shared admission rule uses `302` for GET/HEAD and `303` for other methods). |

The session payload follows the baseline shape: `account` is an insertion-ordered map of `{ id, email }`, and `current` is the selected account ID. Session seals are integrity-protected by the existing iron codec; invalid or expired seals become a fresh empty session.

## Workspace endpoints

These are explicit JSON alternatives to the Solid actions. Workspace-scoped endpoints first resolve the cookie session, then require a `user` actor whose admitted `workspaceID` exactly equals the path ID. A public actor receives `401`; an authenticated actor for another workspace, account actor, or system actor receives `403`.

| Method and path | JSON request | Response and authorization |
| --- | --- | --- |
| `GET /workspaces` | none | `200` array of `{ id, name, slug }` for the current account's non-deleted memberships. Requires an account actor. |
| `POST /workspaces` | `{ "name": string }` | `201` `{ "id": string }`. Requires an account actor; creates workspace, admin membership, zero billing, and the default API key. Empty names are rejected. |
| `GET /workspaces/{id}` | none | `200` `{ id, name, slug }`; requires membership in that exact workspace. |
| `PATCH /workspaces/{id}` | Any subset of `name`, `region` (`us`, `eu`, `sg`, `cn` array), and `allow_training` (`boolean`). JSON `null` for these optional fields is rejected. | `204`; requires an admitted **admin** actor. Name is 1..255 UTF-16 code units and region arrays are non-empty. Unauthenticated/mismatched/member access is rejected. |
| `DELETE /workspaces/{id}` | none | `204`; requires an admitted actor for the exact workspace. This intentionally follows the baseline: `Workspace.remove` has no admin assertion, so an admitted member may delete. |
| `POST /workspaces/{id}/default-region` | `{}` or `{ "country": string }` | `200` JSON region list. Uses `us,eu,sg` except country `CN`, which includes `cn`; only fills a currently-null region. Any admitted member may invoke it, matching baseline. |
| `GET /workspaces/{id}/members` | none | `200` member list scoped to the admitted workspace. |
| `PATCH /workspaces/{id}/members/{member_id}` | `{ "role": "admin" | "member", "monthlyLimit": integer | null }` | `204`; admin-only, never permits demoting/removing self according to domain rules. |
| `DELETE /workspaces/{id}/members/{member_id}` | none | `204`; admin-only and never self. |
| `GET /workspaces/{id}/keys` | none | `200` key list scoped to workspace; secret visibility follows the adapter rules. |
| `POST /workspaces/{id}/keys` | `{ "name": string }` | `201` `{ "id": string }`; creates a key for the admitted user's own user ID. |
| `DELETE /workspaces/{id}/keys/{key_id}` | none | `204`; adapter/domain authorization applies and cross-workspace access is denied. |

Workspace mutation requests are intended to enforce the same-origin browser guard. The controlled test evidence below verifies cross-origin rejection for PATCH; every future mutation route must retain this guard, including DELETE, default-region, member, and key actions.

Domain/store failures map to JSON errors: validation `400`, permission/actor errors `403`, not found `404`, conflict `409`, not-yet-ported `501`, and database failures `500`.

## Controlled verification

Run from `rust/` with local fixture prerequisites as described in `rust/contracts/phase-12/contract.md`:

```sh
cargo fmt -p opencode-hosted
cargo test -p opencode-hosted console::tests -- --nocapture
```

Final controlled result reported by the HTTP verifier on 2026-10-26: **5 passed, 0 failed, 11 filtered, 5.05s**. The covered cases are:

- missing and tampered cookies return `200` with an empty session;
- local fake OpenAuth token exchange, callback, session login, and logout round trip;
- workspace A cannot read or PATCH workspace B;
- a member can read but cannot PATCH workspace settings;
- PATCH persists name, region, and `allow_training`;
- default-region is fill-only;
- an evil `Origin` plus `Sec-Fetch-Site: cross-site` PATCH returns `403`.

Each database fixture uses a unique `TestDatabase`. No live issuer, payment, mail, or provider network was used, and no tests were silently skipped.

The verifier has not yet added explicit acceptance coverage for `POST /auth/action/actor` or workspace `DELETE` routes. Those remain required before calling the entire transport acceptance-complete.

## Limitations and inherited risks

- SolidStart server-function wire format, browser navigation behavior, and the full hosted frontend workflow are not certified by these Axum tests.
- The live OpenAuth issuer and real OAuth provider callbacks were not exercised; only local fake issuer fixtures were used.
- JWT subject decoding intentionally remains unverified, matching the existing OpenAuth client adapter behavior. The Rust route must not describe this as signature-validation certification.
- OAuth `state` is generated but not persisted or checked on callback, also inherited from the baseline flow. This remains an explicit security risk and is not completion certification.
- Invite-mail delivery is not part of this HTTP evidence. The tenant adapter's mail seam is fixture-oriented; live invite mail is deferred/not exercised.
- Billing, provider, model, usage, and Zen route parity is outside this document's tested surface.
- Public-origin hardening is present in `auth/request.rs` and has a unit test proving configured origin ignores an attacker-controlled Host. Same-origin guard calls are present on workspace create, update, default-region, key create/remove, member update/remove, and workspace delete. The passing cross-site PATCH acceptance case does not prove every mutation's guard end-to-end; add explicit route acceptance coverage for the remaining methods.
