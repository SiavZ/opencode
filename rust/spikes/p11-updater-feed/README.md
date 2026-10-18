# Phase 11 updater feed evidence

Date: 2026-10-03. Leaf: `p11.shell.feed`. Implementation commit: `6019efa7d`, followed by staged-artifact hardening and expanded tests.

## Inventory and boundary

| Baseline / contract | Rust implementation | Proof |
| --- | --- | --- |
| `packages/desktop/src/main/updater.ts:13-20`, latest channel, `allowPrerelease=false`, `allowDowngrade=true` | `desktop-host/src/updater/feed.rs:147-180`, SemVer selection allows lower stable versions, ignores equal versions and prereleases | `updater_feed.rs:216` |
| `packages/desktop/src/main/updater.ts:29-46`, backend check/download/install handoff | `FeedBackend:104`, `Installer:83`, `UnsupportedInstaller:88` | `updater_feed.rs:171` |
| `packages/desktop/src/main/updater-controller.ts:36-64`, errors return through controller | Feed methods return typed `UpdaterError`, clear stale selection before new checks and failed downloads | `updater_feed.rs:197,241` |
| Tauri 2 static latest.json: required version and platform URL/signature | `FeedRelease:44`, `FeedPlatform:38` | Actual HTTP fixture emits `platforms.darwin-aarch64` |
| Signature verification before staging | Injected existing `SignatureVerifier`, production `MinisignVerifier`, second verification before installer handoff | `updater_feed.rs:197,318,366` |

Official contract consulted: <https://v2.tauri.app/plugin/updater/> (retrieved 2026-10-03), static manifest required keys `version`, `platforms.[target].url`, `platforms.[target].signature`. Optional notes/pub_date are retained. No Tauri dependency is introduced into the host crate.

`FeedConfig::new(feed_url, current_version, platform, staging_dir)` defaults to 60s total HTTP request timeout, 1 MiB feed and 256 MiB artifact limits. Caller supplies the channel-selected URL. Channel persistence belongs to the sibling `UpdaterFileStore`, not this backend. Redirects and implicit environment proxies are disabled. URL policy permits HTTPS or numeric loopback HTTP without credentials. Chunks are checked even when Content-Length is absent. Download uses a unique create-new stage filename unrelated to feed text, verifies before creating the staging directory, syncs the file, and only then publishes the staged path. Installer callbacks run outside the selection lock. Filesystem bytes are reverified before handoff. A future native installer must preserve that verified artifact identity across replacement rather than reopening a mutable path.

## Executed local spike

From `rust/`:

```sh
export RUSTC_WRAPPER=sccache CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=3
export SCCACHE_BASEDIRS="$PWD/target" CARGO_TARGET_DIR="$PWD/target/p11"
cargo check -p opencode-desktop-host
cargo test -p opencode-desktop-host --test updater_feed -- --nocapture
cargo clippy -p opencode-desktop-host --test updater_feed -- -D warnings
```

Captured result at 18:32 UTC:

```text
running 9 tests
127.0.0.1 latest.json -> 2.0.0 -> minisign verified -> staged -> Unsupported install
test result: ok. 9 passed; 0 failed; 0 ignored
```

Tests create actual ephemeral loopback axum servers, gracefully stop and join them, generate unrelated throwaway minisign keys, and write key material only beneath auto-deleted tempdirs. No keys, signatures or installer artifacts are checked in. HTTP and signature behavior are live local execution. Payloads are test fixtures, not shipping installers.

Cases: verified stage and typed unsupported install, modified download rejection and successful retry, stable downgrade / equal version / prerelease policy, malformed JSON, missing platform, retryable 503, 204 no release, declared and chunked size bounds, timeout, refused redirect, unsafe feed/artifact URL, empty successful feed, invalid SemVer, invalid signature encoding, wrong signing key, disk tampering at handoff and unwritable staging location.

`cargo check` passes. Strict clippy was blocked by unrelated owner code: `connections/health.rs:56`, `platform/apps.rs:104,111` (`collapsible_if`), `platform/wsl/exe.rs:145` (`chunks_exact_to_as_chunks`). Coordinator notified. Re-running with only these two lint categories allowed passed:

```sh
cargo clippy -p opencode-desktop-host --test updater_feed -- \
  -D warnings -A clippy::collapsible_if -A clippy::chunks_exact_to_as_chunks
```

## Blockers and unchecked scope

| Item | Missing behavior | Why | Owner | Proof needed |
| --- | --- | --- | --- | --- |
| Native installation/relaunch | `UnsupportedInstaller` returns typed Unsupported, never claims success | OS replacement/restart/rollback not implemented | Phase 11 shell / Phase 15 | Real signed installer staging, replacement, restart and rollback on supported runners |
| Production update trust | No shipping public key/feed is configured | Real keys and publishing prohibited | Phase 15 | Approved staging signing keys and release qualification |
| Feed transport parity | Proxy/certificate settings are not inherited from Electron, endpoint fallback/dynamic manifests not implemented | Leaf implements requested static URL contract only | Phase 11 | Approved network policy and local TLS/proxy fixtures |
| Stage retention | Successful obsolete stage files retained in supplied directory | No retention policy specified | Phase 11 | Bounded stage GC and restart adoption design |
| Platforms | Windows/Linux installers and remote HTTPS execution not tested | No native runners or production network calls used | Phase 15 | Platform matrix acceptance |
| Whole crate lint | Strict -D warnings currently fails outside feed ownership | Sibling lint errors | Phase 11 connections | Rerun strict clippy after owner fixes |

Did not check real OS install, rollback, crash recovery of staged artifacts, concurrent install/check across direct backend callers, native UI updater dialogs, full shell integration, channel wiring in the shell, live HTTPS or proxy/custom CA handling. The controller and persisted channel tests are sibling scope. No full workspace tests were run.
