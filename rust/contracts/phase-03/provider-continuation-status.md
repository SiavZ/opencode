# Provider catalog and auth continuation observations

Date: 2026-10-19. Scope owner: provider/catalog/auth continuation. This is not a claim that every Phase 03 blocker is closed. Existing protocol and SDK-specific blockers remain tracked in `providers-blockers.md`.

## Baseline to public Rust observations

| Baseline | Concrete Rust boundary | Observation and limit |
| --- | --- | --- |
| `packages/core/src/models-dev.ts`: explicit source, source-hashed cache, `OPENCODE_MODELS_PATH`, disable-download policy, TTL refresh | `provider-adapters::catalog::{ModelsService,cache_filename}` | Controlled HTTP tests cover real GET, user agent, cache persistence, force/TTL, validation before atomic replacement, invalid cache recovery, read-only override and cross-instance file locking. SHA1 uses exact source bytes. Refresh invalidates the same catalog read by runtime resolution. Periodic refresh must be owned and joined by Process lifecycle, not a detached service task. |
| ModelsDev inventory projection and experimental modes | `CatalogStore::load_cache` | Real inventory parsing retains environment names/raw public metadata; mode models retain base API identity, merge cost tiers and provider request headers/body. No generated reasoning-variant table is claimed. |
| V2 `Catalog.model.default`, legacy `Provider.defaultModel`, small-model selection | `CatalogStore::{default_model,legacy_default,legacy_default_for,small_model}` | Distinct V2 release ordering and V1 priority/latest/ID ordering. V1 recent and explicit identities are accepted as explicit inputs. Small-model selection uses explicit clock, active/enabled text modalities, positive cost, age, preferred size names and weighted cost/age. CLI composition must supply persisted recent history to obtain full legacy behavior. |
| Canonical `Integration.connection.active` and credentials | `runtime::provider_services::ProviderServices::credentials`, `runtime::location_models` | Every lookup reads actual SQLite records and explicit Location environment; auth changes and refreshed inventory are not independent stale snapshots. Canonical `integrationID`, not display/provider alias, identifies credentials. Model resolver strips `apiKey` from request bodies. |
| Legacy `ProviderAuth.methods/authorize/callback` and plugin `auth` hooks | additive `plugin_abi` `auth.methods`, `auth.authorize`, `auth.callback`; `ProviderServices` | Native plugin host invokes real authorizer, validators and pending callback. Supports code and automatic callbacks, typed missing/code/validation/failure results, key/OAuth storage rotation and metadata. Controlled loopback OAuth test uses fixture tokens only. No browser login, user credential, package installation or provider network. |
| Legacy provider/auth and current provider/model/credential HTTP families | `ProviderServices: server::services::legacy::LegacySessions` | Shared adapter exposes legacy provider projection/auth methods/OAuth and mutations; V2 provider/model projections and credential-ID update/remove operate against concrete catalog/SQLite. Server host owns location envelopes and route registration. Direct adapter tests do not substitute for whole Process HTTP replay. |
| Catalog I/O failure distinct from missing model or authorization | additive `ModelCatalog::check`, `ModelError::Catalog`, live runtime override | Resolver validates fallible live catalog before historical optional lookup ports; error tag `Catalog.Error`. Explicit regression ensures failure precedes model and credential lookup. |

## Validation ledger

Commands run through the isolated repair environment with explicit temporary HOME/XDG, `--locked --offline`, no installs:

- `cargo test --locked --offline -p opencode-provider-adapters`: passed, 236 tests across 23 result groups, 2026-10-19 11:46:55 UTC. Covers full provider adapter suite, including loopback refresh/policy and selection/mode discovery tests.
- Focused runtime OAuth/current credential tests and resolver typed-failure tests: validation in progress at this document's initial write.
- `cargo clippy --locked --offline -p opencode-provider-adapters -p opencode-runtime --all-targets -- -D warnings`: validation in progress at this document's initial write.
- Earlier runtime attempts stopped at shared root manifest/lock churn and then root-owned `global_services.rs` Environment access compile failures. These are compile blockers, not passed auth observations.

## Explicit remaining gaps

- Full `transform.ts` generated reasoning variants, provider-specific reasoning toggles/defaults and dynamic discovery parity are not closed by configured variants or ModelsDev mode expansion.
- Plugin authorization/callback execution is concrete. Built-in OAuth plugin registration, plugin auth-loader-returned custom JS fetch closures and provider-specific refresh/discovery policies still need composition and differential proof. Saving OAuth tokens alone does not prove those provider runtimes work.
- V1 provider projection does not claim every custom SDK/autoload rule. Existing unsupported Cohere/gateway/GitLab/SAP/dynamic provider and cloud credential-chain rows remain real blockers unless separately proven by their owners.
- CLI interactive login prompts/browser handling and persisted recent-model history are CLI/root ownership. Public auth operations are available; no mock success flag substitutes for missing UI flow.
- No live provider, real OAuth account, external network capture or complete product-parity claim is used as evidence here.
