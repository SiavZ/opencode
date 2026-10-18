# Phase 05 status: p05.tools-exec (search, shell, native process runner)

Owner: p05.tools-exec. Last updated 2026-10-04.

## Implemented

| Area | Rust | Baseline |
| --- | --- | --- |
| Native process runner (process group, SIGTERM, grace, SIGKILL, drop guard, bounded capture, live sink, run_stream) | `platform-native/src/process/{mod,kill,capture,stream}.rs` | `packages/core/src/{process.ts,cross-spawn-spawner.ts}` |
| ripgrep binary resolution (PATH plus managed bin, managed `rg`, installer port) | `tools/src/search/binary.rs` | `packages/core/src/ripgrep/binary.ts` |
| Ripgrep adapter glob/find/grep (limit+1 early kill, invalid pattern, 64 KiB record bound, 2000-unit preview) | `tools/src/search/ripgrep.rs` | `packages/core/src/ripgrep.ts` |
| Core glob/grep tools | `tools/src/search/core.rs` | `packages/core/src/tool/{glob,grep}.ts` |
| Legacy glob/grep tools | `tools/src/search/legacy.rs` | `packages/opencode/src/tool/{glob,grep}.ts` |
| Core bash tool | `tools/src/shell/core.rs` | `packages/core/src/tool/bash.ts` |
| Legacy shell tool (streamed progress, rolling window, retained file, tail, timeout/abort metadata) | `tools/src/shell/legacy.rs`, `output.rs` | `packages/opencode/src/tool/shell.ts` |
| tree-sitter-bash permission scan and BashArity | `tools/src/shell/{scan,arity}.rs` | `shell.ts:84-260,374-410`, `permission/arity.ts` |
| Shell selection and args | `tools/src/shell/select.rs` | `packages/core/src/shell.ts` |
| Prompt rendering (all shell profiles) | `tools/src/shell/prompt.rs` | `shell/prompt.ts`, `shell/shell.txt` |

## Tests (all live unless noted)

- `platform-native/tests/process.rs` (9): exit codes, bounded capture, sink, cwd/env/stdin, timeout/cancel/drop kill descendants, SIGTERM-ignoring group escalates, run_stream.
- `tools/tests/search_ripgrep.rs` (6): ports `core/test/ripgrep.test.ts`, plus limits, invalid pattern, binary resolution order.
- `tools/tests/search_tools.rs` (7): core/legacy glob+grep, descriptions byte-identical, schema fixtures, alias path, containment finding.
- `tools/tests/shell_scan.rs` (7): prompts byte-identical to captured baseline for 8 variants (fixture-based), arity table drift check, permission patterns, external dirs.
- `tools/tests/shell_core.rs` (9) and `shell_legacy.rs` (8): real `/bin/sh` runs covering approvals, denial, timeout kills children, abort, truncation and retention.

Fixture-only: schema JSON (`search/fixtures`, `shell/fixtures/schemas.json`) and rendered prompts (`shell/fixtures/prompts.json`), captured from baseline producers (see `search/fixtures/README.md`).

## Findings

- **Search containment (preserved, test-backed):** core glob/grep assert permission on the pattern only, then resolve the unvalidated `path` without Location containment or external_directory approval. `../x` or absolute paths search outside the Location. Test: `search_tools.rs::finding_core_search_escapes_location_without_external_approval`. Baseline: `packages/core/src/tool/glob.ts:62-78`, `grep.ts:77-97`. Semantics are not changed. A decision for the coordinator.
- The baseline has no mtime sort in glob/grep (ripgrep order is preserved), so none was ported.
- Legacy grep passes no `file` to ripgrep, so an exact file path searches its parent directory (baseline grep.ts:58-66). This is preserved.

## Blockers

| item | missing behavior | why | owner | proof needed |
| --- | --- | --- | --- | --- |
| PowerShell scan | tree-sitter-powershell parsing, `$env:`/provider path expansion | no Rust grammar crate in workspace deps; `ScanError::PowerShellNotPorted` returned and tested | p05 / coordinator (dep approval) | port shell.test.ts PowerShell cases |
| Windows shells | Git Bash resolution, cygpath, cmd/pwsh-specific invocation, taskkill path exercised | no Windows CI | p05 | Windows runs of shell_* suites |
| ripgrep download | HTTP download + tar/zip extraction | network installer not composed; `NoInstaller` typed refusal tested | p05 / runtime composition | installer adapter + test with local archive |
| shell.env plugin hook | plugin env augmentation | `ShellEnv` port only; plugin host not ported | p07.plugins | composition test |
| fff search layer, FileSystemSearch.find | fuzzy find / fff backend | out of tool scope | p05.native-fs / runtime | |
| Config/RuntimeFlags wiring | `tool_output` limits, `bashDefaultTimeoutMs`, config `shell` | constructor parameters; composition is phase 91 | p91 | composition test |
