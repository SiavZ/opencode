# Native platform execution, 2026-10-19

Owner: platforms. Scope approved by coordinator: `.github/workflows/rust.yml`, desktop updater subscriptions and their tests, this evidence note. No desktop application was launched. No software, target, image or paid infrastructure was installed. No source was uploaded to a new service.

## Observed native CI

Existing authenticated repository: `SiavZ/opencode`. GitHub CLI discovery selected the already installed official CLI. No credential values were read or printed. Repository self-hosted runner query returned `total_count=0`; hosted runners executed the following existing run.

Run `39227041182`, SHA `dd2499bdd2b8647a692b6d2cb25e6f899cd57315`, started October 19, 2026 at 09:02:03 UTC, completed with failure:

| Job | Job ID | Observed result | What this proves |
| --- | --- | --- | --- |
| gates | 117975734091 | success | Earlier structural/conformance checks, not current strict product parity |
| native ubuntu-24.04 | 117975734450 | executable all-target check passed; runtime test step failed | Actual Linux compiler execution, no packaged acceptance |
| native macos-14 | 117975734588 | executable all-target check passed; runtime test step failed | Actual macOS compiler execution, no packaged acceptance |
| windows-build | 117975734644 | success | Actual `cargo check -p opencode --bin opencode --target x86_64-pc-windows-msvc`; no linked executable, runtime or terminal proof |

Ubuntu and macOS both failed `cli_help::acp_help_normalizes_short_and_long_cwd_without_dropping_other_options`, `bins/opencode/tests/cli_help.rs:209`. The long-directory ACP help still had `[default:` on the `--cwd` row, yielding `left: true`, `right: false`. Exact output was sent to the CLI owner. Packaging and packaged acceptance were skipped in that run. Its overall failure must not be presented as native runtime success.

## Implemented verification changes

Commit `afde29295` adds Windows native runtime suites, isolated HOME/USERPROFILE/APPDATA/XDG directories, real release archive build and checksum/SBOM verification, extraction and execution, then all three existing executable acceptance workflows with no blocked candidate rows allowed. It also adds desktop and terminal integration to Unix CI and preserves test logs with acceptance artifacts. Independent proof steps continue after earlier test failures, while the job still fails.

The structural gate retains strict `xtask check`; no conformance bypass was introduced. Installed `actionlint .github/workflows/rust.yml` passed. Bun YAML parsing passed. These are configuration checks, not execution of the new Windows job. New run IDs and results remain pending coordinator publication.

Existing `ui-terminal/tests/terminal_pty.rs` and `platform-native/tests/process.rs` are Unix-only. A green portable terminal unit suite on Windows would not establish Windows PTY behavior. The native owner supplied a real cross-platform PTY spawn/read/write/resize/remove/shutdown test, verified on Linux below. Commit `7ab24ae9c` requires explicit execution of that named test with nonzero passed-count evidence on Windows and Unix; logs are retained separately.

## Desktop callback parity

Commit `77960ca99` corrects `desktop-host/src/updater/subscriptions.rs` against the full immutable `packages/desktop/src/main/updater-subscriptions.ts` function.

| Baseline requirement | Implementation | Observed result |
| --- | --- | --- |
| Callback executes before its key is deleted | `delete` clones a repeatable callback, invokes it outside the mutex, then removes the key | Old entry remains observable; nested removal invokes the old callback twice and removes the nested replacement |
| `set` disposes before replacement is installed | `set` calls `delete` before storing the new callback | Replacement observation test passes |
| `Map.forEach` visits callback insertions | Monotonic insertion sequence and live lookup per callback | Insertion trace `[1,2,3]` |
| Removed entries are skipped; reinsertion moves to the end | Live sequence lookup, replacement gets a new sequence | Replacement trace `[1,2,3,20]` |
| Callback failure aborts before deletion | No deletion runs after a callback panic | Entry remains present after caught panic |

The committed `tests/updater_subscriptions_baseline.ts` imports the actual TypeScript implementation and asserts its traces. Executed from `packages/desktop` with fresh scratch HOME/XDG, it returned `{"insert":[1,2,3],"replace":[1,2,3,20],"nested":[1,1]}`. Corresponding real Rust public API tests passed. This is actual baseline capture plus regression tests, not a copied JavaScript Map implementation.

Local isolated macOS validation:

- `cargo test --locked --offline -p opencode-desktop-host --test updater_subscriptions`: 8 passed.
- `cargo test --locked --offline -p opencode-desktop-host --tests`: 252 passed, 0 ignored, 31 test executables, exit 0.
- `cargo clippy --locked --offline -p opencode-desktop-host --all-targets -- -D warnings`: exit 0.
- Owned-path `git diff --check`: passed.

All Cargo commands used the mandated `repair-env.sh` isolation/cache wrapper. Callback registration now accepts `Fn + Send + Sync` rather than consuming `FnOnce`, because the baseline permits repeated nested invocation. Only test callers existed in this checkout; their required capture cloning was updated. No manifest or crate edge changed for this repair.

## Local Linux execution attempts

The already running Colima Docker VM reports Linux aarch64, 4 CPU and 8 GiB. Existing local image `rust:1.99.0`, image ID `15ad267e7a4c`, successfully executed `uname`, `rustc 1.99.0 (b940084d7 2026-09-28)` and Cargo. Image pulls and networking were disabled. Checkout and existing Cargo registry were mounted read-only; build output used a dedicated task scratch directory, with 2 CPU and 3 GiB limits and isolated product directories.

Attempt 1, background task `6772166hp3`, exited 101. Real native compilation reached the owner's newly edited PTY source while Cargo had resolved the pre-edit manifest, yielding unresolved `portable_pty` imports. The native owner was notified; no dependency error was bypassed or owner file changed.

Attempt 2, background task `040514iiry`, reached desktop/UI-terminal linking. Read-only Docker probes eventually answered and process inspection showed slow GNU bfd linking, not an unavailable VM or observed OOM. After more than eight minutes, only the task-owned container was stopped; the command exited 137 because of that explicit stop. No unrelated container or VM was restarted. This broad desktop test attempt did not pass.

Focused PTY attempt `580543o0w0` compiled in 4m17s then timed out at runtime. Diagnostic rerun `0159136taq` compiled in 4.52s and exposed the actual shell prompt prefix: expected `\r\nPTY_NATIVE_READY\r\n`, collected `echo PTY_NATIVE_READY\r\n\r\n# PTY_NATIVE_READY\r\n# # `. The native owner corrected the test command to emit a leading newline, preserving strict output matching rather than matching echoed commands.

Actual Linux PTY rerun `1423439jih` passed: `cargo test --locked --offline -p opencode-platform-native --test pty native_pty_write_read_resize_remove_and_shutdown -- --exact --nocapture`, 1 passed, 0 failed, 0 ignored, 1 filtered out, execution 0.01s after 11.56s compile. Environment: Linux aarch64 kernel `6.8.0-117-generic`, rustc `1.99.0 (b940084d7 2026-09-28)`, existing image `rust:1.99.0`, HEAD `cb6c8fb761561e886977727e8f48658abd8b3e42` plus current native-owner working changes. This exercises real PTY write/read, terminal attachment, `stty` resize, removal and joined shutdown. It is focused runtime proof, not full Linux product/package acceptance.

Task-specific scratch evidence is under `opencode-platform-proof`: `linux-native.log`, `linux-desktop-terminal.log`, `linux-pty.log`, `linux-pty-diagnostic.log`, `linux-pty-final.log`, `ci-39227041182-failed.log`, and `run-linux.sh`. The Linux proof script uses `--locked --offline`, no network, no image pulls, and no existing product state.

## Remaining proof

| Item | Missing observation | Owner/prerequisite |
| --- | --- | --- |
| Updated Windows runtime and package execution | New native CI run at an integrated SHA | Coordinator push, then existing hosted Windows runner |
| Windows PTY | Actual native spawn/read/write/resize/remove/shutdown | Test supplied and explicitly required; Windows CI execution pending |
| Broad Linux desktop and runtime | Focused PTY passed; broader executable suites remain incomplete | Existing native CI after coordinator publication |
| Linux packaged executable | Archive verification and all three candidate workflows at integrated SHA | Existing native CI after coordinator publication |
| Desktop bundles/signing/install/relaunch | Not exercised or changed here | Outside this scoped repair; existing desktop/Phase 15 blockers remain |
