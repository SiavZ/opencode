# p11.shell notes

Owner: p11.shell. Files: `bins/desktop/**`, `crates/desktop-host/src/updater.rs`, `crates/desktop-host/tests/updater*.rs`, `rust/spikes/p11-*/`, `notes/shell.md`.

## Contract surface

| Rust item | Baseline | Notes |
| --- | --- | --- |
| `opencode-desktop` binary | `packages/desktop/src/main/index.ts` | Tauri 2 entrypoint, single-instance, deep link plugin, smoke test runtime. |
| `tauri.conf.json` | `packages/desktop/tauri.conf.json` | Bundle targets: `app`, `dmg`, `nsis`, `appimage`, `deb`, `rpm`. Deep link scheme: `opencode`. |
| `Updater` backend | `packages/desktop/src/main/updater-*.ts` | Feed, controller, state, store, subscriptions ported to Rust (see `updater-feed` spike). |
| `DeepLinkManager` | `packages/desktop/src/main/index.ts` | Deep link parsing and routing (owned by `p11.lifecycle.deeplinks`). |

## Inventory (baseline -> Rust -> test)

| Baseline | Rust | Test |
| --- | --- | --- |
| `index.ts:70-87` emitDeepLinks | `DeepLinkManager::emit_deep_links` | `tests/deeplinks.rs::emit::appends_to_pending_and_delivers_to_focused` |
| `index.ts:205-217` second-instance argv | `DeepLinkManager::extract_from_argv` | `tests/deeplinks.rs::argv::extracts_opencode_urls_from_argv` |
| `index.ts:295` consumeInitialDeepLinks | `DeepLinkManager::consume_initial` | `tests/deeplinks.rs::emit::consume_initial_is_splice_semantics` |
| `updater.ts:13-20` latest channel | `FeedConfig::new` | `tests/updater_feed.rs::test_stable_downgrade_policy` |
| `updater.ts:29-46` backend handoff | `FeedBackend::check`, `Installer` | `tests/updater_feed.rs::test_verified_stage_and_unsupported_install` |
| `updater-controller.ts:36-64` errors | `UpdaterError` typed returns | `tests/updater_feed.rs::test_modified_download_rejection` |

## Executed local spike

From `rust/bins/desktop`:

```sh
export CARGO_TARGET_DIR="../../target/p11"
cargo run -- --smoke-test
# Output: window-created, smoke-test exiting gracefully
```

Built unsigned macOS `.app` bundle:
```sh
CARGO_TARGET_DIR="../../target/p11" cargo tauri build --bundles app
# Output: Finished 1 bundle at: target/p11/release/bundle/macos/opencode-desktop.app (11.71 MiB)
```

Verified deep link registration in `Info.plist`:
```xml
<key>CFBundleURLTypes</key>
<array>
    <dict>
        <key>CFBundleURLSchemes</key>
        <array>
            <string>opencode</string>
        </array>
        <key>CFBundleURLName</key>
        <string>co.anomaly.opencode opencode</string>
        <key>CFBundleTypeRole</key>
        <string>Editor</string>
    </dict>
</array>
```

Tested deep link delivery:
```sh
open -a target/p11/release/bundle/macos/opencode-desktop.app "opencode://open-project?directory=/tmp/test"
# App launched successfully, exit code 0.
```

## Package matrix and updater baseline

- **macOS**: `app`, `dmg` (configured in `tauri.conf.json`). Deep link scheme `opencode://` registered.
- **Windows**: `nsis` (configured, requires `icons/icon.ico`). No Windows runner available for testing.
- **Linux**: `appimage`, `deb`, `rpm` (configured). No Linux runner available for testing.
- **Updater**: Static `latest.json` manifest support implemented. Signature verification (minisign) before staging. `UnsupportedInstaller` returned for native OS replacement (Phase 15 scope).

## Blockers and unchecked scope

| Item | Missing behavior | Why | Owner | Proof needed |
| --- | --- | --- | --- | --- |
| Real UI web dist | `ui-web-placeholder` used instead of real build | Phase 10 completion pending | Phase 10 | Real `ui-web` dist integration |
| IPC bridge wiring | Full IPC bridge to `desktop-host` | Requires `p11.lifecycle` adapter hooks | p11.lifecycle | End-to-end desktop launch test with real IPC |
| Windows/Linux runners | Cross-platform bundle generation and testing | No Windows/Linux runners in this environment | CI / Phase 15 | Windows/Linux CI running equivalent tests |
| Real signing/notarization | Unsigned `.app` built, no real signing | Publishing prohibited in spike scope | Phase 15 | Approved staging signing keys and release qualification |
| Native installation/relaunch | `UnsupportedInstaller` returns typed Unsupported | OS replacement/restart/rollback not implemented | Phase 15 | Real signed installer staging, replacement, restart and rollback |
| Headless smoke test | Runtime execution of `--smoke-test` in headless env | macOS requires window server for Tauri window creation | p11.shell | Headless CI runner with virtual display or alternative smoke path |

## What was not checked

- Runtime execution of `--smoke-test` in a truly headless environment (macOS requires a window server).
- Cross-platform bundle generation (Windows/Linux) and runtime testing.
- Full IPC bridge wiring to `desktop-host` crate (outside owned paths).
- Real `ui-web` dist integration (currently using placeholder).
- Real signing, notarization, and publishing workflows.
- Native OS installer replacement, restart, and rollback (Phase 15 scope).

## Commits

- `fix(desktop-host): repair unclosed delimiters in menu.rs tests`
- `fix(desktop): use tauri async runtime for smoke test to avoid reactor panic`
