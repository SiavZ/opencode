# Audit repair verification, 2026-10-19

Scope: repair the executable release blockers found against `f3ce997bc`, verify the real workflows, and push the shared `rust-port` branch. This is not a claim that the entire TypeScript product has been ported. Full-product compatibility remains tracked separately by the strict fixture gate and phase inventories.

## Requirement-to-check map

| Audit finding | Public or integration check | Observed status |
|---|---|---|
| 1. Production runner was `UnportedRunner` | `runtime/tests/production.rs`: configured loopback provider, real HTTP admission, terminal assistant, native Git snapshot, canonical stored credentials and restart persistence | Passed through the actual Process, SQLite projectors and provider transport. |
| 2. Normal CLI could not execute | `opencode/tests/commands.rs`: actual binary run/models, stdin, durable continue, provider failure, no-argument PTY success/error/cancellation and terminal restoration | All nine workflows also passed against the extracted release. Full TypeScript command/TUI parity is not claimed. |
| 3. Required legacy HTTP/SSE missing | `runtime/tests/legacy.rs` and shipped `serve-health`/`session-create-list` workflows | Actual persisted Git/nongit project/session identity, directory/global placed events, restart, authorization, CORS and errors pass. Fresh extracted-release health, session create/list/get and placed SSE acceptance also pass. |
| 4. Invalid server configuration discarded | `opencode/tests/serve.rs`: malformed isolated configuration before readiness | Passed. |
| 5. Invalid explicit/configured ports silently became zero | CLI unit and actual serve tests for negative, fractional and out-of-range values | Passed without reporting readiness. |
| 6. Unix signal imports broke Windows | Unix-specific serve/terminal/example boundaries, real terminal tests, native Windows CI compile job | Native Unix terminal tests and actual PTY restoration pass. Windows compilation is CI-only, not locally verified because the target is not installed. |
| 7. Leased runner lost cancellation cleanup | Actual public live SSE delta, interrupt, durable partial text/error/completion and idle; automatic and overflow held-summary cancellation | Passed. Location acquisition and both compaction awaits observe cancellation. Delta durability remains unchanged. |
| 8. Dangling external symlink escaped mutation boundary | `platform-native/tests/fs_location.rs`: dangling file/directory/link-chain rejection and internal missing-file controls | Passed in the complete native package suite. |
| 9. Duplicate permission assertion removed original pending request | `permissions/tests/service.rs`: duplicate, cancellation/publication failure and reused-ID ownership controls | Passed in the complete permission package suite. |
| 10. Successful leader left unbounded stdin delivery | `platform-native/tests/process.rs`: deadline, cancellation/drop, actual pipe EOF and unrelated-child survival | Passed. Input delivery is polled inside the same deadline future, without a detached writer task. |
| 11. Runtime smoke test treated idle as model success | Terminal durable assistant, provider rejection, actual read/write tool continuation, stored Bearer precedence and restart assertions | Passed. Idle alone cannot satisfy completion. The lifecycle-only smoke test is explicitly labeled insufficient model evidence. |
| 12. Gates/CI were stubs | Actual resolved native/wasm edges, headers, independent modularity review, inventory and real fixture execution; native packaging/acceptance CI | Real edges, headers, size, inventory and aggregate fixture conformance pass. Oversized files received real module extractions or individual independent expiring reviews. Strict full-product parity remains red with 384 known empty-host replay incompatibilities. |
| 13. ACP help normalizer failed under wrapped paths | Actual binary help fixture under short filesystem-root and long wrapped cwd | Both controls pass. |

## Executed integration evidence

- `defc5e41e` wires the real Location graph. `cb2b13f24` wires process globals, real legacy services/event bus and isolated Git/configuration inputs. Six production target tests pass: five real production workflows and one raw loopback fixture control. The stored-credential test uses the canonical provider integration identity, not a made-up configuration field.
- Four production-compaction target tests pass: automatic held-summary interruption, provider-overflow held-summary interruption, successful persisted summary plus reloaded continuation, and the shared raw HTTP fixture control. Separate adapter tests exercise native Git scoped diffs and actual provider summary transport with SQLite event projection.
- Two production legacy HTTP integration tests pass with real Git and nongit identities, persisted session list/get after restart, actual placed events, authorization/error ordering and CORS.
- The command target passes nine actual executable workflows, including three PTY cases that assert exact termios restoration and disabled alternate-screen/bracketed-paste state. A stop-aware loopback fixture fixes idle-probe/load handling without weakening provider assertions.
- The desktop subscription-order failure was a real insertion-order defect relative to the JavaScript Map baseline. `b0e0652e3` preserves insertion order; 100 targeted runs and the full desktop suite pass. Existing callback reentrancy differences are documented rather than mislabeled as parity.
- Earlier isolated domain sweep: 883 tests passed across ten domain packages. Three default-ignored local baseline differential tests were explicitly run and passed. This is historical supporting evidence, not the final whole-workspace total.
- Header metadata was regenerated from the actual source graph. `4b8a9ce4e` contains 821 independently filtered comment-only file changes; functional changes were excluded from that commit. Accurate physical counts then exposed seven additional size failures. Five received real module extractions, and two received individual independent lifecycle reviews with expiry dates. The final physical size and dependency header checks pass.

## Final whole-result runs

### Fresh shipped archive

The final post-keybind source build is recorded in `/Users/siavash/.jcode/scratch/opencode-rust-audit/native-package-KsSvHd`. Every recorded command exited zero: actual native distribution build, checksum/SBOM verification, all 11 distribution tests, archive extraction, executable identity, actual corrupt/missing/foreign archive rejection controls, and public executable acceptance. The archive contains a Mach-O ARM64 executable and the SBOM lists 402 components.

| Output | SHA-256 |
|---|---|
| Extracted and freshly built executable, identical | `70b78c2d2488ac90aa102e7a11efa672424bf0125439e8a9142df902c53a0b65` |
| `opencode-darwin-arm64.zip` | `8b7a2c689e92d30ea401c0ae291ada11695ad062a4a292e65dd354a56d150b0f` |
| `opencode-darwin-arm64.cdx.json` | `af431b4aea6b6f8423cce8f7752cfcd47f853745062a0e1567d6bfb8856ca30e` |

The extracted executable passes `--version`, health/global SSE, and session create/list/get/directory SSE. An independent reviewer checked the candidate observations against each requirement. The three baseline executable rows remain explicitly blocked because `BASELINE_CMD` is unset; candidate acceptance is not a baseline parity comparison.

The same nine unchanged command/provider/PTY tests also passed against that exact extracted executable at 08:54:00 UTC on October 19, 2026, with zero failures or ignored tests. This includes real configured-provider turns, restart continuation, rejection handling and three real terminal restoration cases. Evidence is preserved in `final-post-keybind-acceptance.log` and background run `032291zdq7`. No test was replaced for the archive run.

### Whole-workspace verification

The first broad run, `final-workspace-Rxcw6M`, passed workspace all-target strict Clippy and the real aggregate gate but exposed eight SDK launcher fixture failures plus two formatting failures. It was not reported as green. A combined wrapper subsequently reached its 600-second limit during a separate ignored-test compile, so the final runs use independent bounded jobs.

The SDK tests reproduced macOS shell children delayed in dyld before shell main under concurrent fixture load. The repair serializes unrelated shell scenarios while retaining an explicit real two-server concurrent launch/shutdown test and every existing production deadline/assertion. The TUI fixture now waits for the actual complete child record instead of assuming an immediately returned launch has completed its write. The exact workspace-feature launcher target passes 16 tests, and three concurrent stress copies pass 48 tests. No production launcher code or timeout changed. The formatting repair removes a stray blank line and extracts the unchanged keybind defaults table; an independent ordered-literal comparison found no data or API change.

The final `cargo test --locked --offline --workspace --no-fail-fast` completed with exit zero at 09:00:22 UTC on October 19, 2026. `/Users/siavash/.jcode/scratch/opencode-rust-audit/workspace-acceptance-ecNm1z/tests.log` contains 283 test-summary blocks totaling **2,068 passed, zero failed and three default-ignored**. Those three are exactly the local Bun baseline differentials; all three passed when explicitly enabled in the separate `final-ignored-local-differentials.log` run. Nothing remains skipped without an observed result in this local test set.

The SDK fixture repair was on disk when the workspace run started and was committed as `3fbfac01c` during the run without further source edits. The workspace-feature launcher target passed all 16 tests in that full run. `source-status-before.txt` and `source-status-after.txt` record that commit transition; it did not change the tested file contents.

The final quality run, `/Users/siavash/.jcode/scratch/opencode-rust-audit/final-quality-b8O3Zc`, records exit zero for all-target workspace Clippy with `-D warnings`, full workspace rustfmt check, actual `xtask check`, and diff whitespace validation. The aggregate gate executes resolved native/wasm edges, graph headers, physical size, inventory and real fixture conformance, while still explicitly reporting the 384 known strict product-parity incompatibilities.

No TypeScript baseline file under `packages/` changed relative to the audited starting commit. The repaired native archive, controlled production workflows, every local workspace test and the separate ignored differentials have observed passing results. The only remaining delivery step is non-force publication of the commits and confirmation that the remote branch SHA matches local HEAD; that confirmation is reported separately to avoid a self-referential commit hash in this document.

## Explicit limits outside the repaired audit slice

- Strict fixtures still reject 384 known empty-host legacy replay mismatches. A stable expected-failure ledger establishes fixture conformance, not full product parity.
- Unregistered legacy families, cursor pagination, interactive permission reply/saved-grant composition, additional tool/command families and full TUI parity remain unfinished integrations. See `composition-notes.md` and phase inventories for their exact scope.
- Native Windows and Linux checks are configured in CI; this local verification is macOS ARM64 only. No missing target was installed, no live model provider was contacted, and no user credentials or production data were used by the controlled acceptance workflows.
