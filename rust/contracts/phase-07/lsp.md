# Phase 07 / p07.lsp: LSP client adapter

Baseline `packages/opencode/src/lsp/{lsp,client,server,language,launch,diagnostic}.ts`. Rust: `rust/crates/integration-adapters/src/lsp/`. Implements `opencode_integration_api::language::LanguageService` (`LspService`).

## Implemented

| Area | Rust | Baseline |
| --- | --- | --- |
| JSON-RPC 2.0 stdio, `Content-Length` framing (case-insensitive headers, extra headers ignored), id correlation, server requests (unknown -> -32601), notifications, per-request timeout, pending requests resolve `Closed` on EOF, joined reader | `jsonrpc.rs` | vscode-jsonrpc via client.ts:132-207 |
| Server request handlers: `window/workDoneProgress/create`, `workspace/configuration` (dotted section lookup into initialization options), `client/registerCapability` / `unregisterCapability` (textDocument/diagnostic only), `workspace/workspaceFolders`, `workspace/diagnostic/refresh` | `client.rs` | client.ts:107-114,173-207 |
| `initialize` with baseline capabilities, rootUri, processId, workspaceFolders, initializationOptions, 45s timeout -> `Failed{LSPInitializeError}` and the process is killed and reaped; `initialized`; `workspace/didChangeConfiguration` when options exist | `client.rs` | client.ts:209-266 |
| Document sync: didOpen v0 (clears push+pull cache, languageId from LANGUAGE_EXTENSIONS), re-open -> didChange v+1 full or incremental (whole previous range, UTF-16 end position over `\r\n|\r|\n`), `didChangeWatchedFiles` created/changed, `didSave`, `didClose` (Rust addition, not used by the baseline flow) | `client.rs`, `text.rs` | client.ts:83-89,554-621, language.ts:1-121 |
| Push diagnostics with publish timestamp/version, typescript first-push seeding, merged push+pull view with baseline dedupe key | `client.rs`, `diagnostics.rs` | client.ts:91-105,139-172,623-629 |
| Pull: `textDocument/diagnostic` (static provider + registered identifiers, relatedDocuments), `workspace/diagnostic` (registrations with workspaceDiagnostics), 3s per-request timeout, mergeResults semantics | `diagnostics.rs` | client.ts:270-444 |
| touchFile wait modes: document (5s, unblocks on first pull batch with items, slower identifier pulls keep merging on an owned background task joined at shutdown), full (10s, all pulls), fresh-push wait with version/after filter and 150ms debounce, registration-change re-pull | `diagnostics.rs`, `requests.rs` | client.ts:446-541, lsp.ts:279-297 |
| 38 builtin servers: id, extensions, root rule, command resolution, args, initializationOptions (inventory below) | `inventory.rs`, `roots.rs`, `resolve.rs`, `resolve_extra.rs` | server.ts |
| Config: `lsp` unset/false -> none, true -> builtins, object -> disabled removes, override keeps builtin position/root/extensions unless given, custom ids appended with root = Location directory, `extensions` empty matches every file; experimental ty vs pyright filter | `inventory.rs` | lsp.ts:96-197 |
| Service: Location guard (directory, or worktree unless `/`), (server, root) dedup, broken set keyed `root+id`, in-flight spawn dedupe via shared future on an owned task, duplicate client after a race is stopped, `lsp.updated` broadcast per new client, status (root relative to directory), hasClients (no availability check, as baseline) | `service.rs`, `requests.rs` | lsp.ts:200-277 |
| Requests: hover (per-client array, null fallback), definition/implementation (null fallback), references (includeDeclaration, [] fallback), documentSymbol, workspaceSymbol (8 kinds, max 10 per client), prepareCallHierarchy, incoming/outgoing via first prepared item; `flat().filter(Boolean)` | `requests.rs` | lsp.ts:299-468 |
| Lifecycle: `shutdown` closes the service, joins spawn tasks, kills+waits every process, joins reader/stderr/background tasks; `restart` does the same then clears the broken set and reopens | `service.rs`, `client.rs` | lsp.ts:187-195 finalizer |
| Launch seam `LaunchEnv`: injectable PATH, `bin_dir` (Global.Path.bin), `npm_cache`, home, env overlay, `disable_download` (OPENCODE_DISABLE_LSP_DOWNLOAD), `experimental_ty` (OPENCODE_EXPERIMENTAL_LSP_TY), serialized roslyn install lock; `which` = PATH + bin_dir | `launch.rs` | launch.ts, core util/which.ts, core npm.ts:200-246 |
| Installs that only shell out to a local toolchain (go install, gem install, dotnet tool install for fsautocomplete/roslyn) run into `bin_dir` only when the tool exists and downloads are enabled | `resolve_extra.rs` | server.ts:366-389,396-421,737-775,827-853 |

`Timeouts` (initialize 45s, debounce 150ms, document 5s, full 10s, diagnostics request 3s) default to the baseline; `request` (feature requests) defaults to None like the baseline and is injectable.

## Tests

All under `rust/crates/integration-adapters/`. Run: `cargo test -p opencode-integration-adapters` (target/p07).

| Test | Kind | Covers |
| --- | --- | --- |
| `src/lsp/text.rs` tests (3) | unit | extname/match key, UTF-16 end position, path.relative/contains, file URI round-trip |
| `tests/lsp_jsonrpc.rs` (2) | fixture (in-memory duplex) | framing, correlation, server requests, -32601, error/timeout/closed mapping, close joins |
| `tests/lsp_client.rs` (6) | fixture (`tests/fixtures/lsp/fake_lsp.py`, python3) | initialize params, handshake order, server requests, full + incremental didChange, versions, didSave/didClose, push wait + version filter timeout, pull dedupe/related/identifier registrations, full mode workspace diagnostics, init timeout kills process, shutdown reaps pid |
| `tests/lsp_service.rs` (4) | fixture (fake server as custom `lsp.fake`) | concurrent spawn dedupe (one process), one updated event, status, broken set for unavailable builtins and missing custom binary, restart clears broken, Location guard, every feature request shape and fallback, workspace symbol filter, disabled config, closed service never respawns |
| `tests/lsp_inventory.rs` (5) | fixture (temp PATH, fake executables) | 38 ids/order vs `BUILTIN_SERVER_IDS`, config merge, root rules (nearest/strict/exclude, rust workspace, go.work, jdtls Maven module chain, bash, nixd worktree), every builtin resolves `Ok(None)` with empty PATH + downloads disabled, PATH resolution args/init, pyright npm cache + venv, typed NotYetPorted download gaps, gopls install into injected bin dir via a local fake `go` |
| `tests/lsp_live.rs::live_clangd_diagnostics_and_hover` | LIVE (clangd on PATH, skips if absent) | builtin clangd through `LspService`: touchFile document wait, real diagnostics, hover, definition, shutdown. Passed on this machine (`/usr/bin/clangd`). |
| `tests/lsp_live.rs::live_rust_analyzer_symbols` | LIVE, opt-in `OPENCODE_LSP_LIVE_RA=1` | skipped here: `rust-analyzer` on this host is a rustup proxy without the component installed |

Last run: 27 tests passed, 3 consecutive runs, clippy `-D warnings` clean.

## Blockers

| item | missing behavior | why | owner | proof needed |
| --- | --- | --- | --- | --- |
| npm-backed servers (typescript, vue, pyright, svelte, astro, yaml-ls, php intelephense, bash, dockerfile, biome) | `Npm.which` cache miss runs an Arborist install into `<cache>/packages/<pkg>`. Rust looks up the cache and returns `NotYetPorted("lsp: npm package install (Npm.which cache miss)")` when downloads are enabled and the bin is absent | needs the Rust npm installer (core `Npm` service), not owned by p07 | p07 / package-install owner | test that a cache miss installs into an injected cache dir and resolves the bin |
| eslint | vscode-eslint zip download + `npm install` + `npm run compile` | HTTP download + archive extraction not ported | p07.lsp follow-up | local HTTP fixture serving a zip, install into temp bin dir |
| elixir-ls, zls, clangd, jdtls, kotlin-ls, lua-ls, terraform, texlab, tinymist | GitHub / HashiCorp / eclipse / JetBrains release download, archive extract (zip, tar.xz, tar.gz), chmod, symlink | needs an HTTP client + archive extractor in the adapter (no network in tests). Each returns a distinct `NotYetPorted` only when downloads are enabled and nothing is installed | p07.lsp follow-up | per-server test against a local HTTP fixture with a fake release JSON + archive |
| `module_resolve` | node `require.resolve` (package `exports`, `main`, symlinked workspaces) is approximated by walking `node_modules/<id>` | typescript/astro tsserver path and eslint/biome presence checks only | p07.lsp | fixture with exports-mapped package |
| lsp.updated event | emitted on an in-process `broadcast` (`LspService::subscribe_updated`), not published to EventV2 | event bus wiring belongs to the runtime composition | runtime / phase 04 | runtime test publishing `lsp.updated` |

## Inventory (baseline `packages/opencode/src/lsp/server.ts` line ranges)

| # | id | lines | root | resolution | install / download |
| --- | --- | --- | --- | --- | --- |
| 1 | deno | 88-113 | strict deno.json/jsonc | `deno lsp` | none |
| 2 | typescript | 115-142 | nearest JS locks, excl deno | tsserver in node_modules + npm `typescript-language-server --stdio`, init tsserver.path | npm: blocker |
| 3 | vue | 144-171 | nearest JS locks | `vue-language-server` or npm `@vue/language-server` | npm: blocker |
| 4 | eslint | 173-222 | nearest JS locks | node `<bin>/vscode-eslint/server/out/eslintServer.js --stdio` | zip: blocker |
| 5 | oxlint | 224-294 | nearest .oxlintrc/locks/package.json | local/up/PATH oxlint `--lsp` (help probe), else oxc_language_server | none |
| 6 | biome | 296-356 | nearest biome.json + locks | local bin, PATH, npm `biome` `lsp-proxy --stdio` | npm: blocker |
| 7 | gopls | 358-390 | go.work then go.mod/go.sum | PATH gopls | `go install` to bin_dir: ported |
| 8 | ruby-lsp | 392-422 | Gemfile | rubocop `--lsp` | `gem install --bindir` bin_dir: ported |
| 9 | ty | 424-483 | python markers + ty.toml | experimental only, PATH or venv ty `server`, init pythonPath | none |
| 10 | pyright | 485-527 | python markers | PATH or npm pyright-langserver `--stdio`, init pythonPath | npm: blocker |
| 11 | elixir-ls | 529-583 | mix.exs/lock | PATH or bin_dir release script | zip+mix: blocker |
| 12 | zls | 585-685 | build.zig | PATH | GitHub: blocker |
| 13 | csharp | 687-701 | .slnx/.sln/.csproj/global.json | roslyn `--stdio --autoLoadProjects` | dotnet tool (serialized): ported |
| 14 | razor | 703-821 | same | roslyn + vscode razor extension args | as csharp |
| 15 | fsharp | 823-854 | .slnx/.sln/.fsproj/global.json | fsautocomplete | dotnet tool `--tool-path` bin_dir: ported |
| 16 | sourcekit-lsp | 856-888 | Package.swift, *.xcodeproj (literal) | PATH or `xcrun --find` | none |
| 17 | rust | 890-933 | crate root then `[workspace]` ancestor | rust-analyzer | none |
| 18 | clangd | 935-1067 | compile_commands/flags/.clangd | PATH, bin_dir/clangd, bin_dir/clangd_*/bin | GitHub: blocker |
| 19 | svelte | 1069-1094 | JS locks | svelteserver or npm | npm: blocker |
| 20 | astro | 1096-1131 | JS locks | astro-ls or npm, init typescript.tsdk | npm: blocker |
| 21 | jdtls | 1147-1271 | gradle wrapper/settings/build, Maven module chain, eclipse | java >= 21 + bin_dir/jdtls launcher jar, temp -data dir | eclipse.org: blocker |
| 22 | kotlin-ls | 1273-1359 | settings.gradle(.kts) (baseline: NearestRoot never undefined) | bin_dir/kotlin-ls/kotlin-lsp.sh `--stdio` | JetBrains: blocker |
| 23 | yaml-ls | 1361-1385 | JS locks | PATH or npm | npm: blocker |
| 24 | lua-ls | 1387-1513 | .luarc/stylua/selene | PATH | GitHub: blocker |
| 25 | php intelephense | 1515-1544 | composer/.php-version | PATH or npm, init telemetry off | npm: blocker |
| 26 | prisma | 1546-1561 | schema.prisma/prisma, excl package.json | `prisma language-server` | none |
| 27 | dart | 1563-1578 | pubspec/analysis_options | `dart language-server --lsp` | none |
| 28 | ocaml-lsp | 1580-1595 | dune/merlin/opam | ocamllsp | none |
| 29 | bash | 1596-1620 | directory | PATH or npm `start` | npm: blocker |
| 30 | terraform | 1622-1693 | .terraform.lock.hcl/tfstate/*.tf (literal) | `terraform-ls serve`, init experimentalFeatures | HashiCorp: blocker |
| 31 | texlab | 1695-1772 | latexmkrc/texlabroot | PATH | GitHub: blocker |
| 32 | dockerfile | 1774-1798 | directory | PATH or npm `--stdio` | npm: blocker |
| 33 | gleam | 1800-1815 | gleam.toml | `gleam lsp` | none |
| 34 | clojure-lsp | 1817-1835 | deps.edn/project.clj/... | `clojure-lsp listen` | none |
| 35 | nixd | 1837-1865 | flake.nix (not directory), else worktree, else directory | nixd | none |
| 36 | tinymist | 1867-1949 | typst.toml | PATH | GitHub: blocker |
| 37 | haskell-language-server | 1951-1966 | stack/cabal/hie, *.cabal (literal) | wrapper `--lsp` | none |
| 38 | julials | 1968-1983 | Project/Manifest.toml, *.jl (literal) | julia LanguageServer runserver | none |

Glob-looking markers (`*.xcodeproj`, `*.tf`, `*.cabal`, `*.jl`) are literal names in the baseline (`fs.stat(join(dir, target))`); the port keeps that behavior.
