# Phase 09 status

## Implemented command slice

The Rust executable now dispatches real local `run`, `models`, and the default terminal entrypoint, alongside `serve`. This is a bounded usable slice, **not complete CLI or TUI parity**.

- `run`: combines positional/passthrough message text and piped stdin, creates or resumes a durable Session, admits the prompt with `resume: false`, explicitly joins `Process.execution.resume`, and streams projected assistant text. Provider and durable assistant failures produce exit 1. Supports explicit provider/model, agent, variant with explicit model, `--dir`, `--continue`, `--session`, and optional projected reasoning output.
- Default TUI and `run --interactive` / `--mini`: basic full-screen local chat using the existing `ui-terminal` terminal guard and typed event normalization. Supports text entry, backspace, paste, resize redraw, durable history replay, real model turns, cancellation, and terminal restoration. It is a basic chat renderer, not the TypeScript application's route/dialog/plugin/markdown/tool-widget implementation.
- `models [provider]`: lists the production Location's actual configured/catalog models, sorts OpenCode providers first and model IDs within providers, supports provider filtering and verbose model metadata. Does not perform inference to list models.
- `serve`: malformed or unreadable global configuration fails before listener/readiness, ports must be finite integers in `0..=65535`, actual zero retains automatic selection, and Unix SIGTERM registration is cfg-gated with a non-Unix Ctrl-C path.
- Global CLI middleware values are passed through the runtime's explicit environment snapshot, without unsafe process-environment mutation.
- Help/version/completion remain parser outcomes. ACP cwd fixture normalization covers the entire final option, including long wrapped type/default rows, with genuine short and long cwd controls.

## Validation

Startup fixes are committed as `4fb351576` and `7798ebb3d`; real command dispatch is committed as `9b1d1d342`. With the production runtime/catalog composition in place, `cargo test --locked --offline -p opencode` passes all 28 tests: 13 binary unit tests, 2 help tests, 9 command/provider/PTY tests, and 4 serve executable tests. None are ignored. `cargo clippy --locked --offline -p opencode --all-targets --no-deps -- -D warnings` also passes.

The command regressions in `rust/bins/opencode/tests/commands.rs` exercise the built executable against a controlled TCP OpenAI-compatible SSE server. Production code uses the actual Process, durable Session service, serialized runner, model catalog, credentials, and provider transport. The tests cover legacy and canonical V2 config, configured Bearer credentials, positional plus piped input, provider errors, provider filtering and verbose metadata, and `--continue` across process restarts without replaying old output. Three real no-argument PTY scenarios cover a completed provider turn, provider failure, and in-flight Ctrl-C cancellation. Each checks terminal modes, alternate screen, and bracketed-paste restoration. Only the external provider response is a fixture; no injected runner or fabricated production success is used.

All tests isolate HOME/XDG/TMPDIR. The nine command/provider/PTY tests also passed three consecutive additional runs after the TCP fixture was hardened for idle connections and bounded slow reads. No live credentials, paid inference, remote publishing, or user-state writes are used.

## Shipped executable acceptance

The `commands` integration target accepts an optional test-only `OPENCODE_ACCEPTANCE_BIN` path. Both ordinary command children and PTY children select that same executable. Without the variable, selection remains `CARGO_BIN_EXE_opencode`. An invalid override fails rather than falling back; all existing functional assertions and isolated child environments remain unchanged. The variable is not a production CLI option.

Run from `rust/`, with the same isolated test environment used above:

```sh
OPENCODE_ACCEPTANCE_BIN=/absolute/path/to/extracted/opencode \
  cargo test --locked --offline -p opencode --test commands
```

### Extracted release artifact result

On 2026-10-19 the same nine tests passed with the override set to the coordinator's extracted shipped artifact, `~/.jcode/scratch/opencode-rust-audit/native-package-gbZLRy/extracted/opencode`. No test or assertion was replaced for this run. The unchanged Cargo default also passed all nine tests, strict all-target bin clippy passed, and a nonexistent override was confirmed to fail rather than use the default binary.

SHA-256: `d0f7cc65de10c47c1af66c022ee96ba8e63fa591514cba9870b27324e7fceb16`

Preserved output from the extracted artifact run:

```text
running 9 tests
test provider_failure_is_a_failed_command_not_admission_success ... ok
test run_uses_production_provider_and_combines_stdin ... ok
test run_and_models_use_canonical_v2_configuration ... ok
test models_lists_real_configured_catalog_and_filters_unknown_provider ... ok
test input_errors_and_unimplemented_features_are_honest_failures ... ok
test run_continue_reuses_durable_session_without_reprinting_old_response ... ok
test pty::default_tui_restores_terminal_after_provider_error ... ok
test pty::default_tui_interrupts_an_active_turn_and_restores_terminal ... ok
test pty::default_tui_runs_a_real_turn_and_restores_terminal ... ok

test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.69s
```

## 2026-10-19 remaining-port implementation

This wave adds production CLI handlers in `bins/opencode/src/cli/commands/{sessions,sanitize,config,diagnostics,database,agents,providers,integrations,json_run}.rs`, using the actual Process composition. It does **not** establish full CLI parity. Historical artifact results above apply only to the preceding nine-test slice, not these additions.

| requirement | implementation | observed executable result |
| --- | --- | --- |
| Session list JSON, delete, import/export and sanitize | Actual legacy session service, persisted messages/parts and field-specific sanitizer | Disposable import -> restart/export -> sanitized export -> delete passes; malformed import creates no session |
| Explicit database SQL, path and formatting | Actual database transaction/query service; JSON and TSV with null/blob behavior | Create/insert/restart/select/invalid-query fixture passes |
| Debug paths/config and native/custom agent listing | Actual path/config/agent services; pure sensitive-field redaction | Paths and native/custom permission overlays pass; resolved precedence/redaction pass but subsequent real provider custom-header preservation currently fails |
| MCP list/OAuth status and skill discovery | Actual Location integration service, transport/auth state and native skill discovery | Skill fixture passes; disabled MCP fixture currently reports not initialized and remains failing |
| Implicit run variant and V2 fork | Actual default selection/model variant resolution and typed V2 session fork | Default variant reaches controlled provider, unknown variant makes no request; fork replay preserves original across restarts |
| Forced models refresh | Actual ModelsService forced fetch and URL-specific persisted cache; configured provider activates discovery without configuring the model | Loopback fetch, refreshed model listing and restart from persisted cache pass |
| Provider credentials | Actual process-global auth/catalog services, credential metadata only, explicit logout by ID/name | Canonical credential seed -> list -> unknown logout failure -> case-insensitive name logout -> restart list passes, even with malformed global/project config; secrets and env values absent from output |
| JSON run events, stdin, local files and title | Actual legacy prompt/event projections, completed parts retain domain IDs, bounded event queue and explicit error propagation | Provider-error fixture passes; attachment fixture exposed genuine legacy/Core read-schema mismatch (`filePath` versus `path`) and remains failing pending native materializer composition |
| ACP long-cwd help invariant | Compare complete default value on option row, not merely the prefix that can wrap independently; commit `26bf41fb7` | Both actual help regressions pass, including short/long controls |

Current real admin execution is **11 passed / 3 failed**. The remaining failures are retained functional assertions, coordinated with provider, integration and session/native owners. An earlier complete package attempt passed 16 unit tests and 2 help tests, then 24/29 command/PTY tests (the three service failures plus two TUI-owner failures). New dual-schema startup and headless permission rejection fixtures are pending. Strict all-target Clippy found duplicate test-module loading and a typed question-ID comparison; both were corrected, rerun pending. No live provider, auth, user data, upgrade or deletion was exercised; all mutations target disposable fixtures.

## Remaining parity blockers

| item | missing behavior | why | owner | proof needed |
| --- | --- | --- | --- | --- |
| Remaining command tree | ACP protocol, provider login and interactive logout picker, MCP add/auth/logout/debug, remaining debug commands, agent creation, web/attach, stats, plugins, upgrade/uninstall and other non-dispatched families | Explicit typed `NotYetPorted` exit 2 remains; no generic successful fallback | Phase 09 / owning services | Real binary command-by-command baseline conformance |
| Session/admin interactive paths | Locale-aware session table/pager, export picker, share-URL import and interactive SQLite shell | Only implemented noninteractive formats are admitted; other paths explicitly rejected | Phase 09 | Actual locale/pager/picker/share fixtures |
| Advanced plain run flags | File/title outside JSON, remote attach/auth, slash commands, sharing, auto-permission modes, demo/replay limits and explicit run HTTP port | Missing concrete service/UI operations remain explicitly rejected before admission | Phase 09 | Actual workflows and permission/error behavior |
| JSON remaining conformance | Child-session permission families, exact styled output, tool-specific pretty output and cross-generation V1/V2 resume | Real legacy event output is implemented, not complete run parity; actual local-file native seam currently failing | Phase 09 / Phase 04 / native | Passing attachment/step/tool/error/restart/cancellation workflows and baseline shape comparisons |
| Config acceptance | Custom provider header preservation | Real executable fixture retained until provider fix (now supplied) is observed passing | Phase 03 / runtime / Phase 09 | Controlled executable fixture passing |
| MCP state acceptance | Configured disabled transport state through real CLI composition | Actual fixture currently returns missing status, not a fabricated disabled result | Integrations / runtime / Phase 09 | Real disabled/connected/error/OAuth-state fixtures |
| Full TUI | Application remains incomplete despite substantive application/editor work | See `tui-application-status.md` for application owner's exact remaining workflows; basic or expanded chat is not full parity | Phase 09 TUI owners | End-user PTY scenarios matching TypeScript TUI |
| Locale and platform coverage | Non-ASCII localeCompare ordering and actual Windows terminal runtime | Rust ordering/macOS observations do not establish baseline locale or Windows behavior | Phase 09 / Integration / CI | Locale fixtures and actual Windows compile/terminal tests |
