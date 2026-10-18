# p09.cli-spec status

Owner paths: `rust/bins/opencode/src/main.rs`, `src/cli/spec/**`, `tests/cli_help*.rs`, `tests/cli_parse.rs`, `tests/fixtures/help/**`.

## Done (committed)

- Model of the whole legacy yargs tree (`spec/model.rs`, `tree.rs`, `tree_debug.rs`, `tree_misc.rs`): every command, alias, positional, option, choice, default, hidden option and `describe: false` command from `packages/opencode/src/index.ts` and `cli/cmd/**`.
- yargs 18 help renderer (`help.rs`) on a byte-compatible port of cliui 9 + wrap-ansi 9 (`cliui.rs`), including the `show()` logo prefix (plain wordmark when not a TTY, colored glyph logo on a TTY).
- yargs-parser compatible tokenizer (`argv.rs`) and command resolution/strict failure/default command/`populate--` (`parse.rs`). Help is checked before version. `completion --help` exits 1, like the baseline.
- Typed result `spec::Invocation` / `spec::Parsed { command: CommandId, path, globals, args, double_dash, raw }`. `Globals::middleware_env(pid)` returns the OPENCODE_PRINT_LOGS/OPENCODE_LOG_LEVEL/OPENCODE_PURE/AGENT/OPENCODE/OPENCODE_PID env as data, because `set_var` is unsafe in edition 2024 and unsafe is denied.
- Completion: the yargs `completion` script (zsh/bash, chosen by `$SHELL`/`$ZSH_NAME`) and the `--get-yargs-completions` reply protocol (`complete.rs`). clap projection (`clap_tree.rs`) plus `spec::clap_completion(Shell)` for native bash/zsh/fish scripts.
- v2 tree (`v2.rs`): `spec::v2_command()` (clap) and `spec::v2_help(path)` (effect CliOutput layout). Fixtures are in `tests/fixtures/help/v2/`.
- `tests/cli_help.rs`: all 80+ legacy fixtures (help, errors, exit codes, completion replies) pass byte-exact.

## Remaining

- v2 error formatting is not implemented: `\nERROR\n  ...` on stderr, help on stdout, exit 1 (fixtures `v2/err_*`). Neither is a v2 entry binary. Only the tree (`v2_command`) and help (`v2_help`) exist, and v2 help is tested against the fixtures (`spec/tests.rs`).
- main.rs still prints NotYetPorted (exit 2) for `Invocation::Run`. Once p09.cli-commands exposes it, switch to `cli::dispatch::run(parsed)`. `mod.rs` has `#![allow(dead_code)]` until then.
- Tests: `tests/cli_help.rs` (legacy golden, all pass) and `src/cli/spec/tests.rs` (9 unit tests: v2 help, coercion, aliases, strict failures, globals env, clap_complete). Clippy is clean with `-D warnings`.

## Residual differences vs baseline

- The acp `--cwd` default row depends on the process cwd, so the test masks that row.
- The yargs completion script is reproduced verbatim. The clap_complete scripts are an additional, non-baseline output.
- Validation messages for the unhandled yargs failures (e.g. `Missing required argument` for required non-positional options) are not printed. The baseline tree has no such options.
