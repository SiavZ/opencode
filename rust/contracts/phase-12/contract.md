# Phase 12 contract: hosted console, identity and billing

Status: in progress. Parent node `p12` owns this file, crate roots and shared seams. Children own the files listed below and nothing else.

## Crates and boundaries

| Crate | Path | Role | Edges (crate-edges.toml) |
| --- | --- | --- | --- |
| `opencode-hosted-domain` | `rust/crates/hosted-domain/src/console/` | pure console rules, no I/O | `schema` |
| `opencode-hosted-adapters` | `rust/crates/hosted-adapters/src/console/` | MySQL, OAuth issuer, Stripe, limits, gateway, mail, support | `schema`, `protocol`, `hosted-domain`, `llm-api`, `provider-adapters`, `observability` |
| `opencode-hosted` (bin) | `rust/bins/hosted/src/console/` | axum console routes, composition | `schema`, `protocol`, `hosted-domain`, `hosted-adapters`, `observability` |

Phase 13 owns `sharing`, `analytics`, `sites` modules in the same crates and adds its own `mod` lines.

## Shared foundation (parent-owned, frozen; request changes from p12)

- `hosted-domain/src/console/common.rs`: `IdKind` + `create_id` (`<prefix>_<ULID>`), `MicroCents = i64`, `cents_to_microcents(Decimal)`, `microcents_to_cents`, `js_round` (JS `Math.round` semantics), `Role`, `Actor` (wire shape `{"type","properties"}`), `ConsoleError` (Validation, PermissionDenied, NotFound, Conflict, Actor, NotYetPorted; Display = baseline message).
- `hosted-adapters/src/console/db.rs`: `ConsoleDb` (mysql_async pool), `read_migrations`/`baseline_migrations_dir`, `migrate` (drizzle `__drizzle_migrations` journal), `DbError::is_duplicate`, `TestDatabase::create(label)` / `drop_database()` for fixture tests.
- `hosted-adapters/src/console/mail.rs`: `InviteMail` + `InviteMailer` trait is the frozen seam between invite flow (p12.identity) and mail delivery (p12.ops).
- `bins/hosted/src/console/{mod.rs,state.rs}`: `ConsoleState { config: Arc<ConsoleConfig>, db }`. Every route module exposes `pub struct Config` (own `from_env`) and `pub fn routes() -> Router<ConsoleState>`; `mod.rs` already merges all five.
- Need a shared handle in `ConsoleState` (e.g. a Redis client, Stripe client)? Put it inside your own route module `Config` (it is cloned into `ConsoleConfig`) instead of editing `state.rs`.

## Decisions

- Console MySQL driver: `mysql_async` 0.37 (sqlx 0.9 was rejected: its optional sqlite crate collides with `rusqlite`'s `links = "sqlite3"`). Phase 13 stats matches.
- Money: integer microcents (`i64`), decimal inputs via `rust_decimal`. No `f64` in money paths. Where the baseline uses JS float math (e.g. cost per token), reproduce the exact rounding with `Decimal` and document it with a fixture test.
- Timestamps: `time::OffsetDateTime` UTC, MySQL `timestamp(3)`.
- Tests: fixture tests against local Docker MySQL 8.4 (`p12-mysql`, `127.0.0.1:33612`, env `OPENCODE_P12_MYSQL_URL`), local axum fake Stripe/SES/OAuth/provider servers. Test names start with `fixture_`. No real payments, mail, provider calls or deploys.
- Build env: `CARGO_TARGET_DIR=rust/target/p12`, `CARGO_BUILD_JOBS=3`, sccache. Scope cargo to `-p opencode-hosted-domain -p opencode-hosted-adapters -p opencode-hosted`.
- Unported behavior returns `ConsoleError::NotYetPorted` and is listed in `status.md`.

## File ownership by child node

| Child | Domain files | Adapter files | Route files | Tests |
| --- | --- | --- | --- | --- |
| p12.identity | `identity.rs`, `membership.rs`, `credentials.rs` | `tenant.rs`, `auth.rs` | `auth.rs`, `workspace.rs` | `hosted-adapters/tests/console_identity*.rs` |
| p12.commerce | `billing.rs`, `referral.rs`, `quotas.rs` | `billing_store.rs`, `stripe.rs` | `billing.rs` | `hosted-adapters/tests/console_commerce*.rs` |
| p12.gateway | `inference.rs` | `gateway.rs`, `limits.rs` | `zen.rs` | `hosted-adapters/tests/console_gateway*.rs` |
| p12.providers | none | `formats.rs` | none | `hosted-adapters/tests/console_formats*.rs` |
| p12.ops | none | `mail.rs`, `support.rs` | `support.rs` | `hosted-adapters/tests/console_ops*.rs` |
| p12.verify | none (review only) | none | none | may add `hosted-adapters/tests/console_verify*.rs` |

A child may split its file into a same-named directory (`billing.rs` + `billing/*.rs`). Cross-child calls go through the public API of the owning file; if you need something from another child that is not there yet, define the narrowest trait in your own file and adapt later, or DM the owner.

Children append their own sections below (public API, wire contract, consumers, conformance tests).
