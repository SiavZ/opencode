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

Startup fixes are committed as `4fb351576` and `7798ebb3d`. The current startup suite passes 13 binary unit tests, 2 help integration tests, and 4 serve executable tests under `cargo test --locked --offline -p opencode --test serve --test cli_help --bin opencode`. Strict all-target bin clippy also passes with `-D warnings`.

Command and real PTY/provider regressions are in `rust/bins/opencode/tests/commands.rs`. They exercise the built executable against a controlled TCP OpenAI-compatible SSE server, not an injected runner or fixture response inside production code. Final command validation is pending the production runtime composition integration.

All tests isolate HOME/XDG/TMPDIR. No live credentials, paid inference, remote publishing, or user-state writes are used.

## Remaining parity blockers

| item | missing behavior | why | owner | proof needed |
| --- | --- | --- | --- | --- |
| Remaining command tree | ACP, MCP, provider login, debug, session administration, web/attach, import/export, upgrade and other exposed commands | Their concrete command implementations have not been ported; dispatch returns typed `NotYetPorted` exit 2 | Phase 09 | Real binary command-by-command baseline conformance |
| Advanced run flags | Remote attach/auth, slash commands, forking/sharing, file attachments, title updates, auto-permission modes, demo commands, replay limits, explicit run HTTP port | Missing concrete service/UI operations; explicitly rejected with typed `NotYetPorted` exit 2 before admission | Phase 09 | Actual executable workflows and error/permission behavior |
| JSON formatting | Legacy `--format json` event shapes and tool/step rendering | Current implementation streams projected text rather than fabricating legacy events; option explicitly unsupported | Phase 09 | Byte/shape-correct legacy event stream fixtures plus actual provider execution |
| Model cache refresh | `models --refresh` remote models.dev cache refresh | No complete catalog refresh service is exposed; explicitly unsupported | Phase 03 / Phase 09 | Controlled models.dev fetch/cache/refresh lifecycle |
| Full TUI | Navigation, dialogs, plugin host composition, markdown/diff/tool widgets, grapheme/display-width editor fidelity, model/agent pickers | Most `ui-terminal` app/prompt/timeline/route modules remain skeletons; basic chat does not claim those capabilities | Phase 09 | End-user PTY scenarios matching TypeScript TUI |
| Implicit model variants | `--variant` without explicit `--model` | No public default-selection update boundary is available; explicitly unsupported rather than ignored | Phase 09 / runtime | Default-model selection plus variant provider assertion |
| Platform coverage | Actual Windows compilation/runtime and terminal interaction | macOS ARM64 tests cannot establish Windows support | Integration / CI | Windows target compile and Windows terminal tests |
