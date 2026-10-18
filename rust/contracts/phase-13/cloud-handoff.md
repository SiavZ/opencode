# p13.cloud handoff (from p13 parent)

An earlier agent committed `dc962b08c`, a working `cargo xtask cloud` (plan/diff/golden), its inventory `rust/contracts/phase-13/cloud-inventory.toml` and golden plans in `rust/contracts/phase-13/cloud-golden/`. The p13 parent adopted it on 2026-10-03: `cargo test -p xtask` passed 13/13.

The p13.cloud child extends this code. Do not rewrite it. These gaps from the node brief are still open:

1. Add a baseline scan test. It reads `sst.config.ts` and `infra/*.ts` text, finds every `new sst.*`, `new cloudflare.*`, `new aws.*`, `new planetscale.*`, `new stripe.*`, `sst.Secret` and `sst.Linkable` construction, and asserts each one appears in the inventory with a file:line. Today's `inventory_matches_every_checked_stage` only compares the generator with the inventory, which is circular.
2. Add `cargo xtask cloud inventory` to print the committed inventory. You may also add `generate` as an alias of `plan`.
3. Audit each resource in `infra/*.ts` by hand against the generated plan, including properties (domains, bindings, DO migrations, VPC/ARM64, IAM, alerts, retain/protect). Record each mismatch you fix.
4. Run `terraform validate` offline only if terraform is already installed. Otherwise record it as blocked.
5. Write `rust/contracts/phase-13/cloud.md` with the contract, an inventory summary with H13 IDs, and a status + blocker table.
6. Check `rust/xtask/src/main.rs` and `rust/xtask/Cargo.toml`, which Phase 01 owns. The earlier commit edited both. DM kitten to confirm the edits are acceptable.
