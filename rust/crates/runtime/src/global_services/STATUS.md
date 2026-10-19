# Global runtime adapter evidence

Owned implementation: `runtime/src/global_services.rs`, its child modules, and `runtime/tests/global_services.rs`.

## Concrete operations

- Configuration: legacy global/instance resolution, comment-preserving updates, explicit captured environment, actual auth.json well-known sources, controlled HTTP transport, nonterminal location invalidation.
- Projects: real ProjectStore resolution and SQLite CRUD/directory rows; native Git initialization; committed update publication.
- Files: native reads/listing, base64 binary responses, lexical and symlink containment, actual ripgrep searches. `file.status` and `find.symbols` return empty arrays because their TypeScript producers do so.
- VCS: native branch/default-branch resolution, porcelain status, tracked/untracked patches, branch diffs, raw text metadata, real patch application and typed application errors.
- Catalogs: legacy builtin/configured agents; command templates, config/resource overlays, actual MCP prompt catalog and skills. MCP lazy template values preserve the baseline HTTP Promise serialization.
- Legacy workspace transfer: actual target resolution, source cancellation/final history synchronization, replay-owner claims, optional patch copying, batched remote replay/steal, and durable local workspace updates. Controlled remote transfer acceptance is still pending.
- Synchronization: real event replay with strict ownership, durable LegacyStore workspace changes, actual persisted event history.
- V2 local relocation: actual SessionStore lookup, ProjectStore identity checks, native change capture/apply, durable Moved publication and source discard with index preservation.
- TUI: actual ephemeral EventStore publication and per-process FIFO control queues. `GlobalServices::tui_control()` exposes request submission and response consumption to the host.

## Acceptance checks

The focused `global_services` integration test exercises the production Process and Services adapter boundary with isolated SQLite/config/checkouts. Git tests create actual repositories and apply patches. Remote configuration uses only a controlled loopback HTTP fixture. Catalog/workspace/history tests use actual Process domains. TUI tests observe actual EventStore listeners and exchange queue values.

Build/test result is recorded in the worker completion report after compilation finishes. Formatting and file-size checks are scoped to these owned files. No external network, installs or live credentials are used.

## Remaining behavior to close

- Console account/organization operations need a runtime account domain equivalent to the baseline account service. Existing hosted-console tenant persistence is a different domain and must not be substituted.
- Legacy `experimental.workspace.warp` is implemented using the real host/transport seams. Native worktree acceptance is queued; remote replay/steal, ownership conflicts and copy-change failures still need controlled peer fixtures.
- `global.upgrade` needs the real installation updater; it must not report success without installation.
- File fuzzy ranking currently uses a subsequence score, not full fuzzysort/FFF scoring. Ordering parity needs a faithful ranking implementation and differential fixtures.
- Agent external-directory defaults discover skill directory roots rather than the complete per-skill/reference whitelist. Plan-path rules need nested-directory differential fixtures.
- HTTP route-level differential replay, watcher events, binary/oversized patch caps, linked-worktree change moves, and remote workspace/account flows still need explicit acceptance evidence.
