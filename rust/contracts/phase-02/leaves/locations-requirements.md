# p02.locations: additional binding requirements (Phase 91 coordinator input, 2026-10-03)

Source: coordinator DM and `rust/contracts/phase-91/composition-notes.md`.

- Registry keyed by `Location.Ref { directory, workspaceID? }`, lazily built per Ref.
- Generic over the per-location service bundle: `LocationRegistry<S>` with a constructor closure/trait and an async teardown trait, so `crates/runtime` injects the real bundle. `locations` must not depend on session/tool crates.
- Idle eviction after 60 minutes without use (`LayerMap` idleTimeToLive, `packages/core/src/location-services.ts:83-112`).
- Ordered teardown (reverse construction order) joined before eviction completes.
- Global-tagged services shared process-wide, never rebuilt per location.
- Emit a `booting location services` log with directory and workspaceID.
- Conformance tests: idle eviction under a paused tokio clock, concurrent `get` for the same Ref builds once, teardown ordering.

## Consumer request from Phase 04 (humpback, 2026-10-03)

- Expose `opencode_locations::project::{ProjectResolver, Resolved { id, directory, vcs }}` with
  `async fn resolve(&self, directory: &str) -> Result<Resolved, LocationError>` (port of
  `packages/core/src/project.ts:110` `ProjectV2.resolve`). session-runtime will depend on it for Session.create.
