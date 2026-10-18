# Phase 11 contract bundle (desktop host)

Root owner: p11. Final `contract.md`, `inventory.md`, `status.md` are written by the p11 root at synthesis.
Child nodes write their sections to `notes/<child>.md` (inventory rows with baseline line ranges, contract surface, tests, blockers).

| Node | Owns |
| --- | --- |
| p11.lifecycle | `crates/desktop-host/src/{bridge,lifecycle,windows,deeplinks,menu}.rs`, `crates/desktop-host/tests/{bridge,lifecycle,windows,deeplinks,menu}*.rs`, `notes/lifecycle.md` |
| p11.connections | `crates/desktop-host/src/connections.rs`, `crates/desktop-host/src/platform/**`, `crates/desktop-host/tests/{connections,platform,wsl}*.rs`, `notes/connections.md` |
| p11.persistence | `crates/desktop-host/src/{drafts,store,attachments,migrate}.rs`, `crates/desktop-host/tests/{drafts,store,attachments,migrate}*.rs`, `notes/persistence.md` |
| p11.shell | `bins/desktop/**`, `crates/desktop-host/src/updater.rs`, `crates/desktop-host/tests/updater*.rs`, `rust/spikes/p11-*/`, `notes/shell.md` |
| p11.verify | `notes/verify.md`, review only |

Root keeps `crates/desktop-host/{Cargo.toml,src/lib.rs}`. Children ask the root for new dependencies or submodules (a module may become a directory `foo/mod.rs` only by its owner, and the root edits lib.rs if needed).
