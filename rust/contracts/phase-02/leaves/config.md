# Phase 02 leaf: configuration (`opencode-config`)

Owner: p02.config. Crate: `rust/crates/config` (deps: `opencode-schema`, `opencode-platform-api`).

## Public surface

| Module | Baseline | What it provides |
| --- | --- | --- |
| `current` | `packages/core/src/config.ts:109-227` | `CurrentConfig::load(env, LocationSnapshot)` ordered `Entry` list (Document/Directory), `latest`, `policies`, `evaluate_policy`, V1 auto-migration, read-once Location snapshot |
| `provider` | `packages/core/src/config/plugin/provider.ts:9-113` | `apply_integrations` / `apply_catalog` over `IntegrationDraft` / `CatalogDraft` traits using `opencode_schema::provider::{ProviderInfo, ModelInfo}` |
| `resources` | `packages/core/src/config/plugin/{agent,command,skill,reference,external}.ts` | ordered agent docs (markdown dirs expanded in place, `~`/`$HOME` path permission expansion), commands, skill sources, references, external plugin refs |
| `migrate`, `provider_options` | `packages/core/src/v1/config/{migrate,provider-options}.ts` | `is_v1`, `migrate`, `migrate_agent`, per-package option lowerers |
| `schema` | `Config.Info`, `ConfigV1.Info` | Effect-Schema-compatible decoder (`errors: all`, ignore excess props, original order) |
| `legacy` | `packages/opencode/src/config/*.ts` | `load(env, Instance, Sources) -> LegacyState { config, plugin_origins, directories, console_state, provenance, diagnostics, console_token }`, `update`, `update_global`, `lower` (v2-compat), markdown loaders, plugin spec helpers, `RemoteFetcher`/`AccountSource`/`ManagedPreferences` traits |
| `jsonc` | jsonc-parser 3.3.1 + `ConfigParse.jsonc` | UTF-16-exact scanner/parser/error offsets, TS-identical error message text, comment-preserving `modify` with TS formatting |
| `variable` | `packages/opencode/src/config/variable.ts:25-91` | `{env:VAR}` (overlay wins, empty -> `""`), `{file:path}` (`~/`, relative to config dir, skipped on `//` lines, trimmed + JSON-escaped, `bad file reference` errors) |
| `value` | remeda `mergeDeep`, `mergeConfigConcatArrays` | deep merge, `instructions` ordered union, `Provenance` (per JSON path, longest prefix lookup) |
| `env` | `Flag`, `Global.Path`, `managed.ts` | hermetic `Environment` (vars, home, global config, username, platform); only `from_process` reads the real env |
| `error` | `ConfigJsonError`, `ConfigInvalidError`, `ConfigFrontmatterError`, `ConfigRemoteAuthError` | `ConfigError` + `to_named_json()` `{ name, data }`; `RemoteError::{Unsupported, Fetch, Decode}` |

## Precedence matrix

Current (V2) config, lowest to highest priority (`config.ts:173-203`):

| # | Source | Notes |
| --- | --- | --- |
| 1 | `<global>/opencode.json`, `<global>/opencode.jsonc` | `<global>` = `OPENCODE_CONFIG_DIR` or `$XDG_CONFIG_HOME/opencode` |
| 2 | Directory entry `<global>` | skills/agents/commands/plugins discovered by consumers |
| 3 | `opencode.json`/`opencode.jsonc` from project root down to the opened directory | walk stops at `project.directory`; skipped when the location is the global dir; `config.json` never read |
| 4 | each `.opencode/` from project root down: its json, jsonc, then the Directory entry | |
| - | Policies | evaluated in reverse document order, last matching statement wins, so global rules override repository rules |

Legacy (V1) merged config (`config.ts:328-610`), each step deep-merges over the previous:

| # | Source | Plugin scope |
| --- | --- | --- |
| 1 | well-known org config per `Auth.wellknown` entry: `<url>/.well-known/opencode` `config` deep-merged with fetched `remote_config` (nested `config` wins over root) | global |
| 2 | global: seed `opencode.jsonc` (unless `OPENCODE_CONFIG*` set), then `config.json`, `opencode.json`, `opencode.jsonc`, then legacy TOML `config` migration | global |
| 3 | `OPENCODE_CONFIG` file | by path |
| 4 | project `opencode.json`, `opencode.jsonc` from worktree down to directory (unless `OPENCODE_DISABLE_PROJECT_CONFIG`) | local |
| 5 | directories: global dir, `.opencode` from worktree down (unless disabled), `~/.opencode`, `OPENCODE_CONFIG_DIR`; for `.opencode` and config dir: json then jsonc; for all: `.gitignore`, markdown commands, agents, modes, `{plugin,plugins}/*.{ts,js}` | by path |
| 6 | `OPENCODE_CONFIG_CONTENT` (virtual, dir = instance directory) | local |
| 7 | active account org config `<url>/api/config` (`OPENCODE_CONSOLE_TOKEN` overlay, failures ignored) | global |
| 8 | managed dir (`OPENCODE_TEST_MANAGED_CONFIG_DIR` or `/Library/Application Support/opencode`, `%ProgramData%\opencode`, `/etc/opencode`) json then jsonc | global |
| 9 | macOS MDM `/Library/Managed Preferences[/<user>]/ai.opencode.managed.plist` via `plutil` (metadata keys stripped) | none |
| post | `mode` -> `agent` (primary), `OPENCODE_PERMISSION` deep-merge (invalid JSON skipped), `tools` -> `permission` (explicit permission wins, write/edit/patch -> edit), username fallback (`user`), `autoshare` -> `share: auto`, `OPENCODE_DISABLE_AUTOCOMPACT`/`OPENCODE_DISABLE_PRUNE` | |

Merge rules: plain objects merge recursively, arrays and scalars replace, except `instructions` (ordered dedupe union) and `plugin` (deduped by identity: npm package name or exact file URL; highest-priority origin survives, survivors keep relative order, `plugin_origins` stays aligned with `plugin`). Path-like plugin specs resolve against the declaring file to `file://` URLs (directories -> `package.json` dir or `index.*`).

## Secret safety

- `$schema` seeding rewrites the original file text, never the substituted value (`preserves env variables when adding $schema`).
- `update`/`update_global` start from raw on-disk text/JSON; `{env:}`/`{file:}` tokens survive; JSONC files are patched with comment-preserving `modify`; empty `shell` is removed from global writes.
- Files with native V2 `permissions` (top-level or per agent) fail validation before any write; error issues carry paths only.
- Compat diagnostics carry source, kind and path, never values. Remote auth overlays never mutate the process env; the console token is returned to the caller (`console_token`) instead of being written to `process.env`.

## Tests (all temp dirs, never the real HOME)

| File | Kind | Derived from |
| --- | --- | --- |
| `tests/jsonc.rs` | fixture (captured from TS jsonc-parser 3.3.1 + `ConfigParse.jsonc`, temp HOME) | parse values, error codes/offsets/messages, 13 `modify` cases |
| `tests/compat_fixtures.rs` | fixture (verbatim copies of `packages/opencode/test/config/fixtures/v2-compat`) | 18 read cases, update-global json/jsonc exact text + normalized, update-project |
| `tests/current.rs` | fixture; `current-v1-expected.json` captured by running the real TS `Config` service | core `config.test.ts` :53-145, :146-265, :455-668, :669-798 |
| `tests/legacy_load.rs` | fixture | opencode `config.test.ts` defaults/seeding, JSONC precedence, substitution :680-742, invalid values, `.opencode` markdown, instructions/plugins merge + origins :1189-1322, disable project config, managed :1372-1422, `OPENCODE_PERMISSION`, `OPENCODE_CONFIG_CONTENT`, plugin spec resolution |
| `tests/legacy_remote.rs` | fixture (in-memory `RemoteFetcher`) | well-known :1673-1860, account org token, `parseManagedPlist` :2129-2230, MDM precedence, typed `Unsupported` default |
| `tests/legacy_update.rs` | fixture | :360-445, :489-526 + resource consumers |
| `tests/markdown.rs` | fixture (verbatim TS markdown fixtures) | `markdown.test.ts`, `entry-name.test.ts`, substitution edge cases, provider plugin |

Live paths: none. No network, no real MDM plist, no real managed dir.

## Blockers and gaps

| item | missing behavior | why | owner | proof needed |
| --- | --- | --- | --- | --- |
| remote HTTP transport | real GET with Accept json and transient retry for well-known/remote_config | transport is Phase 03/06 | 03/06 | adapter implementing `legacy::RemoteFetcher` + live smoke test |
| account/auth stores | `Auth.all()` wellknown entries, `Account.active/config/token` | stores are Phase 06 | 06 | adapter implementing `legacy::AccountSource` |
| npm dependency install | `Npm.install(dir, @opencode-ai/plugin)` per config dir + `waitForDependencies` | Npm service not in this crate | 07 | installer wired by runtime after `LegacyState.directories` |
| platform-api fs contract | safe read, `up`, glob | not in platform-api yet (coordinator DM'd) | 05 | switch `config/src/fs.rs` to platform-api |
| service caching | `getGlobal` cache + `invalidate`, per-instance state | runtime composition concern; `load` is pure per call | 04/runtime | wrapper caching `LegacyState`/`CurrentConfig` |
| schema issue messages | exact Effect `SchemaIssue` formatter text | only paths are wire-relevant; messages differ | 02 follow-up | capture TS issue messages and match |
| npm-package-arg | full `parsePluginSpecifier` (git/tarball/alias forms) | approximated for name@version, scoped and npm: alias | 07 | fixture from TS `parsePluginSpecifier` |
| legacy TOML `config` migration | implemented, untested | low value | 02 follow-up | temp-dir test |
| fs watcher / reload | `ConfigWatcher` semantics | outside loader | 05 | |
