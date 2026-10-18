# Context port integration note

## Delivery and validation

- Algebra and registry/builtins: commits `9cd72d704` and `d6bad8327`.
- Epoch persistence and raw Session history: commit `9596e0923`.
- `cargo test -p opencode-context`: 50 passed. Strict all-target clippy (`cargo clippy -p opencode-context --all-targets -- -D warnings`) and owned-file rustfmt checks passed. Eight targeted original TypeScript SessionRunner tests passed with isolated HOME/XDG.
- Persistence tests use real file-backed SQLite with explicitly labeled Database, EventStore and system-projector fixtures. Production adapter/projector composition, replay/listeners and complete runner acceptance are not yet proven by these tests.
- **Validation correction**: Previous claims citing 65 passed tests for `opencode-context` or 286 passed tests for `opencode-session-runtime` were incorrect. The actual `opencode-context` test count is 50, and `opencode-session-runtime` has 229 tests across all targets. Additionally, no `runner_context` test target exists; the correct strict clippy command is `cargo clippy -p opencode-session-runtime --all-targets -- -D warnings`.

## Approved baseline correction: sequence zero

Coordinator approval: October 3, 2026, 22:03 local time (+03:00).

`packages/core/src/session/context-epoch.ts:59-62` computes an optional `replacementSeq`, then selects replacement using its truthiness. A compaction sequence of `0` after an initial baseline sequence of `-1` therefore incorrectly chooses reconciliation. Rust intentionally tests optional presence (`Some(0)`), preserving the contract that every post-baseline compaction replaces the context generation. This is an approved correction of the baseline falsy-zero defect, not an unresolved parity deviation.

Proof: `rust/crates/context/tests/history.rs:357`, `compaction_at_sequence_zero_replaces_an_empty_event_baseline`, committed in `9596e0923`. It verifies replacement baseline text and baseline sequence `0` without an extra ContextUpdated event.

## Source interface for domain producers

Domain producers implement `Source<A>` with a namespaced `Key`, async loader returning `Result<Observation<A>, ContextError>`, baseline renderer and change renderer. Optional removal text is configured through `with_removed`. Values implement serde serialization/deserialization, equality and Send/Sync. `make` erases each source's value type while preserving its codec boundary. Registry entries return a `SystemContext` and keep their RAII registration alive for the Location scope. An empty producer context means removal, while `Observation::Unavailable` retains an admitted snapshot. Instruction, skill guidance and reference guidance producers stay with their owning domains.

## Pending integration follow-through

The coordinator's event transaction contract v2, committed upstream as `849d43b09`, removes the EventStore `Tx` associated type and uses non-generic higher-ranked `&dyn Sql` projector/commit callbacks. Context epoch and fixture hooks have migrated to that contract, including removal of the obsolete explicit static executor lifetime workaround. All 50 context tests, strict all-target clippy, formatting and whitespace checks passed again after migration. Production SqlEventStore composition remains unverified. Concurrent same-Session prepares and production SessionRunner orchestration are not checked here. The runner is expected to serialize Session drains.
