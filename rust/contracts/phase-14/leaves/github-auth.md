# GitHub auth and Feishu bridge leaf

Status on October 3, 2026: Rust library implemented and fixture-tested. Live adapter qualification and hosted mounting are not complete. This leaf adopted the interrupted worker's auth modules, bridge, tests and test RSA keys rather than replacing them.

## Implemented behavior

| Item | Baseline | Rust implementation | Fixture proof |
| --- | --- | --- | --- |
| Actions OIDC verification | `packages/function/src/api.ts:262-283`, `packages/function/src/github.ts:3-14` | `src/github/auth/oidc.rs`: issuer `https://token.actions.githubusercontent.com`, audience `opencode-github-action`, fetched JWKS, kid selection, RSA signatures through jsonwebtoken with rust_crypto, key use/operations filtering, cached keys and unknown-kid refetch. Signature is verified before claims are used. | `tests/github_auth_oidc.rs`, 13 tests |
| Repository and claim validation | Same ranges | Exactly two nonempty repository components. Scalar issuer, audience string/array, optional NumericDate exp/nbf/iat with numeric type checks. Expiry has zero tolerance and rejects equality with now. Missing exp remains accepted because the baseline jose call does not require it. Unknown critical extensions and unencoded JWT payloads are rejected. | Valid/forged/expired/future tokens, issuer array, malformed dates, missing expiry, audience array, key operations, unknown kid, rotation and critical headers |
| App JWT and installation access token | `packages/function/src/api.ts:286-300,329-349` | `src/github/auth/app.rs`: PEM and escaped-newline PEM loading, RS256 signing with iat=now-30 and exp=iat+600, repository installation lookup, then access-token creation. GitHub base URL is injectable. | `tests/github_auth_exchange.rs`: fake GitHub verifies App JWT signature/issuer/lifetime and records lookup before token creation |
| PAT write permission | `packages/function/src/api.ts:312-358` | Read repository permissions using PAT auth. Any of admin/push/maintain permits exchange. Read-only, missing permissions, missing authorization and upstream errors preserve baseline 401 messages. Malformed JSON and null bodies return the uncaught-handler 500. | PAT success, read-only denial, missing permission object, bad credentials and missing installation |
| Hosted route dispatch | `packages/function/src/api.ts:262-387` | `src/github/auth/handlers.rs`: method/path/query/headers/body request values, response status/body and content-type headers. Unknown routes return None. Missing authorization is 401, OIDC verification failure 403, exchange upstream failure 502. Installation lookup echoes raw installation JSON, Not Found yields `{}`, other errors yield 500. | Exact JSON/error-string assertions in `tests/github_auth_exchange.rs`, 5 tests |
| Action token acquisition and cleanup | `packages/opencode/src/cli/cmd/github.handler.ts:474-484,688-700,984-1020,1593-1604`, `github/index.ts:369-401,1060-1072` | `src/github/auth/client.rs`: GITHUB_TOKEN/use_github_token bypass, custom OIDC base URL, Actions ID-token request with audience and request-token bearer auth, bounded 502/503/504 retry, CLI PAT-prefix routing and separate root-host mock routing/error formatting. Returned token records ownership. Explicit revoke sends DELETE /installation/token only for an owned App token and treats failure as nonfatal. | `tests/github_auth_client.rs`, 8 tests, including OIDC retry, both exchange routes, root-host JSON error, revoke and borrowed-token bypass |
| Feishu projection and Discord request | `packages/function/src/api.ts:205-258` | `src/bridge.rs`: challenge echo, JSON/raw text fallback, ECMAScript whitespace, leading user mention removal, leading aiden rewrite, root/message ID suffix, empty success, Discord bot post, non-2xx 502, malformed-body/content and transport 500. Discord base URL is injectable. | `tests/bridge_feishu.rs`, 3 tests, including Unicode whitespace, primitive JSON bodies, request auth/channel/content and upstream errors |

Paths in the implementation and fixture columns are relative to `rust/crates/ecosystem`.

Installation token creation deliberately sends no permissions or repository restriction body. The baseline createAppAuth call specifies only installationId, so GitHub applies the installed App's granted scopes. This implementation does not pretend that it narrows the token to the claimed repository. A least-privilege change needs an approved contract.

## Consumer contract

Phase 13 can mount `ExchangeService::handle(&HttpRequest)` and `FeishuBridge::handle(&HttpRequest)`. Populate the separate decoded query pairs and case-insensitive request headers. Each method returns `Option<HttpResponse>`. Use the status, `headers()` and body unchanged. None means another router owns the route.

The CLI consumer should call `TokenClient::acquire`, retain the `AcquiredToken`, then await `revoke` during owned shutdown. `TokenSource::GithubToken` skips exchange and revoke. Root-host consumers can use `exchange_root` to preserve explicit mock mode and its different error text. The root host's TOKEN/MOCK_EVENT environment interpretation and runner cleanup orchestration remain consumer-owned, not hidden in the CLI TokenEnv loader.

App credentials, acquired tokens, request-token fields and bridge credentials redact Debug output where those credential types are exposed. URL details are removed from Action/App transport errors. Route logging records error kinds rather than tokens or request bodies. Do not log HttpRequest/HttpResponse bodies or custom upstream error messages: those plain wire values can contain credentials, and the baseline client exchange error intentionally includes an upstream response body.

## Tests and fixture limits

All auth and bridge tests have `fixture_` names and use local axum servers bound to 127.0.0.1. The two PEM files under `tests/fixtures/auth` are TEST KEYS ONLY, as labeled in their README. Fixture token strings grant no access. No live GitHub, Actions, Feishu or Discord request ran.

Validation environment:

```sh
cd rust
export RUSTC_WRAPPER=sccache CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=3
export SCCACHE_BASEDIRS="$PWD/target" CARGO_TARGET_DIR="$PWD/target/p14"
cargo test -p opencode-ecosystem
cargo clippy -p opencode-ecosystem --all-targets -- -D warnings
cargo fmt -p opencode-ecosystem
```

The full package suite passed: 29 auth/bridge fixture tests plus 2 parent-owned session tests, 31 total. Strict all-target clippy passed. Formatting is scoped to ecosystem to avoid rewriting another owner's files. All owned Rust files are below 400 physical lines and have the mandatory header block. No manifest or dependency edits were needed.

Regression checks first reproduced four failures in adopted work: JWK key_ops was ignored, malformed iat was accepted, a PAT null body returned 401 instead of 500, and Feishu scalar bodies returned 500 instead of empty success. All four now pass. Additional checks cover expiry equality, scalar issuer shape, critical headers, root-host error formatting, OIDC URL redaction and JavaScript whitespace.

## Blockers and qualification work

| Item | Missing behavior | Why | Owner | Proof needed |
| --- | --- | --- | --- | --- |
| Hosted composition | Actual phase 13 bins mounting these library handlers | Outside leaf ownership | Phase 13 hosted composition | Mounted-route request/response acceptance using the returned headers/body |
| Feishu trust | Signature/verification-token checks, replay protection and challenge authorization | Baseline has none. No approved correction or ingress credentials supplied | Phase 14 security and Phase 13 ingress | Approved trust contract plus forged/replayed/valid staging-delivery tests before public mounting |
| Discord abuse controls | Authorized destination policy, mention-abuse policy, rate limits and delivery deduplication | Baseline forwards content without these controls | Phase 14 bridge/security | Approved policy and local plus staging abuse/rate-limit fixtures |
| Logging changes | Approval of raw-body/cause suppression | Rust removes sensitive baseline logs instead of reproducing them | Phase 14 security | Privacy review accepting the documented diagnostic differences |
| App least privilege | Repository-specific and explicitly narrowed installation-token scope | Baseline inherits the App installation's complete grants | Phase 14 GitHub/security | Approved permissions/repository restriction contract and App-scope acceptance test |
| Live adapter qualification | Real scoped App/OIDC/PAT exchange, revoke and owned Feishu-to-test-Discord delivery | Production side effects prohibited and staging credentials not supplied | Phase 14 qualification | Sanitized staging captures and token-expiry/revocation evidence |
| Runner cleanup integration | Actual run exit/cancellation invoking revoke, root TOKEN/mock selection and Git credential restoration | Run/host lifecycle is outside auth library ownership | Phase 14 GitHub run/host leaf | Representative runner tests showing only owned tokens are revoked and shutdown awaits cleanup |

## Not checked

No live credentials, deployment, public ingress or remote mutations were used. No Windows/Linux/WASM compilation, full Action/root-host execution, Phase 13 mount test, cross-phase host composition, or full TypeScript differential route execution ran. The claim verifier covers RSA algorithms, not jose's EC/EdDSA alternatives. Cache expiry under load, concurrent JWKS refresh storms, retry exhaustion timing, hostile oversized input, arbitrary out-of-schema nested values and redirect/proxy behavior need separate qualification. Fixture servers are spawned inside each test runtime and torn down with that runtime, not qualified as graceful production shutdown examples.
