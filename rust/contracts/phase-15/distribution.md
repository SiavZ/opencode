# Phase 15: distribution contract

Owner: Phase 15 child `p15.distribution-ops`. Status date: 2026-10-03.

## Surfaces

| Surface | Path | State |
| --- | --- | --- |
| `cargo xtask distribution build\|checksums\|sbom\|verify\|matrix` | `rust/xtask/src/distribution/` | implemented, tested, run end to end on `aarch64-apple-darwin` |
| Release matrix manifest | `rust/distribution/release-matrix.json` | generated from code (`matrix --write`), drift fails `verify` and a unit test |
| Container | `rust/distribution/Dockerfile` (+ `Dockerfile.dockerignore`) | builds for `linux/arm64` locally; runtime health check blocked on Phase 09 `serve` |
| Nix derivation | `rust/distribution/nix/opencode-rust.nix` | written, **evaluation blocked: nix not installed** |
| CI | `.github/workflows/rust.yml` | `actionlint` clean, never run on GitHub (not pushed) |
| Runbooks | `rust/contracts/phase-15/runbooks/*.md` | written, only the SQLite backup procedure was exercised (temp DB) |

## Naming contract (mirrors the baseline)

Source: `packages/opencode/script/build.ts:53-200` (`allTargets`, name = `[pkg.name, os, arch, baseline?, abi?].join("-")`), `build.ts:241-247` (linux `tar -czf`, everything else `zip -r`, executed inside `dist/<name>/bin` so the executable sits at the archive root), `install:117-168` (client-side selection `opencode-<os>-<arch>[-baseline][-musl].<ext>`), `.github/workflows/publish.yml:176-290` (darwin/windows zips).

- 12 rows, order equal to `allTargets`: linux arm64/x64/x64-baseline (glibc), linux arm64/x64/x64-baseline (musl), darwin arm64/x64/x64-baseline, windows arm64/x64/x64-baseline.
- Archive entry: `opencode` (`opencode.exe` on Windows), mode 0755, mtime 0 (tar) / 1980-01-01 (zip), uid/gid 0. Same input gives byte-identical archives (tested, and observed by rebuilding).
- "baseline" vs default on x64 maps to `-C target-cpu=x86-64` vs `-C target-cpu=x86-64-v3` (the Bun `avx2: false` split). Applied only through `CARGO_ENCODED_RUSTFLAGS` for that row.
- `SHA256SUMS`: coreutils format `<hex>  <name>`, sorted, covers `*.tar.gz`, `*.zip`, `*.cdx.json`. `sha256sum -c` / `shasum -a 256 -c` accept it (observed).
- SBOM: `<asset>.cdx.json`, CycloneDX 1.5 JSON, built in Rust from `cargo metadata --locked --filter-platform <triple>` + `Cargo.lock`. Root component carries the SHA-256 of the built executable. Each component has `purl` (`pkg:cargo/<name>@<version>`, `vcs_url` qualifier for git sources), `licenses[].expression` from the manifest license field, `hashes` = the Cargo.lock registry checksum, `scope: excluded` for build-only crates. Dev-dependencies are excluded. There is no timestamp, and `serialNumber` is derived from the content, so the SBOM is reproducible.

## D13 status per row

`executable`: `opencode-darwin-arm64` (local), `opencode-linux-arm64`, `opencode-linux-arm64-musl` (docker linux/arm64). Every other row is `blocked: runner unavailable (D13)` and has a ready job in `.github/workflows/rust.yml`.

## Typed errors (`DistError`)

`Usage`, `UnknownTarget`, `PackageMissing` (the default `opencode` package missing, or a bad `--package`), `BinMissing`, `TargetNotInstalled` (no rustup target and not the host, e.g. Windows rows here), `BuildFailed`, `ManifestDrift`, `ChecksumMismatch`, `ChecksumMissing`, `UnlistedArtifact` (not covered by SHA256SUMS, or a name that is not a matrix row), `ArchiveContents` (anything other than exactly the product executable).

## Evidence (recorded 2026-10-03)

Tests: `cargo test -p xtask --bin xtask distribution`: 11 passed. These are fixture tests except `sbom_live_workspace` and `checked_in_manifest_matches_code`, which read the real workspace. `cargo clippy -p xtask --all-targets -- -D warnings` is clean.

End to end, live, default package `opencode` (rust/bins/opencode), host row:

```
$ CARGO_TARGET_DIR=target/p15 cargo xtask distribution build --out target/p15/dist-e2e
built opencode (opencode) for aarch64-apple-darwin
archive .../target/p15/dist-e2e/opencode-darwin-arm64.zip
sbom .../target/p15/dist-e2e/opencode-darwin-arm64.cdx.json (204 components)
SHA256SUMS
2c1e1f716809a67f13a971ed81a65991fd56cb7174f89e64f3b84d1997ca8ccb  opencode-darwin-arm64.cdx.json
2cfb455ade6e0f48adc4c235bad92b7f5fd3462c5d434d64a2f1034fa57227d9  opencode-darwin-arm64.zip
$ cargo xtask distribution verify --dir target/p15/dist-e2e
manifest .../distribution/release-matrix.json ok: 12 targets, 3 executable, 9 blocked (D13)
target/p15/dist-e2e ok: 2 artifacts match SHA256SUMS
$ shasum -a 256 -c SHA256SUMS      -> both OK
$ unzip -l opencode-darwin-arm64.zip -> single entry `opencode`, 425824 bytes, 01-01-1980
$ ./opencode --version              -> "command line not yet ported" exit 2 (Phase 09 stub, expected)
```

All 204 SBOM components carry licenses. 203 carry lock checksums: the missing one is a workspace path crate, which has no registry checksum. A second build into another directory produced a byte-identical zip (`cmp` passed).

Typed failures observed: `--package nope` gives `PackageMissing`, `--target linux-riscv64` gives `UnknownTarget`, `--target opencode-windows-x64` gives `TargetNotInstalled`.

Container, live, linux/arm64 (Docker 29.5.2, `rust:1.99-slim-trixie`, `debian:trixie-slim`): `docker build -f distribution/Dockerfile --platform linux/arm64 -t opencode-rust:p15 .` from `rust/` exits 0. The image is arm64, 72 MB, `User=opencode:opencode` (uid 10001), `Volumes=/data`, and has a HEALTHCHECK on `/global/health`. `/data` is writable and `/usr` is not. Running the image exits 2 with the Phase 09 stub message, so the health check, signal handling and persistence of a running server are **untested** until `opencode serve` exists.

Nix: blocked, because `nix` is not on PATH. The derivation is unevaluated. To verify, run `nix build --impure --expr '(import <nixpkgs> {}).callPackage ./rust/distribution/nix/opencode-rust.nix {}'` on aarch64/x86_64 Linux and macOS.

CI: `actionlint .github/workflows/rust.yml` is clean. Action pins reuse the baseline SHAs (checkout v4.3.1, upload-artifact v4.6.2, download-artifact v4.3.0). New pins were resolved with `git ls-remote` on 2026-10-03: `Swatinem/rust-cache` v2.9.2 at `6323deb1…` (peeled tag) and `taiki-e/install-action` v2.87.22 at `83ac0ad6…`. The workflow has never run. `cargo xtask headers/edges/size` are Phase 01 tasks that currently report "not yet ported", so the lint job will fail on them until Phase 01 lands.

## Blockers

| item | missing behavior | why | owner | proof needed |
| --- | --- | --- | --- | --- |
| 9 non-local release rows | build + run of x64 Linux (gnu/musl), darwin x64, Windows x64/arm64 artifacts | D13: no runner | Phase 92 | `rust.yml` build matrix green with artifacts |
| container runtime | health check, SIGTERM drain, `/data` persistence across restart | `opencode serve` not ported | Phase 09/06 then Phase 15 | `docker run` health=healthy, restart keeps DB |
| nix | eval/build of `opencode-rust.nix`, flake wiring | nix not installed; `flake.nix` outside ownership | Phase 15 + flake owner | `nix build` on Linux + macOS |
| signing/notarization | macOS codesign/notarize, Windows Authenticode (baseline `publish.yml`) | needs secrets, publishing forbidden | Phase 94 | signed artifacts verified with `codesign -v` / `signtool verify` |
| npm/Homebrew/AUR/install script | Rust-built archives consumed by `install`, `publish.ts` Homebrew/AUR formulas | no release exists | Phase 94 | install script against a staged release |
| SBOM schema validation | validation against the official CycloneDX 1.5 JSON schema | no validator installed offline | Phase 15 | `cyclonedx validate` or a JSON-schema run in CI |
| provenance | SLSA/attestations for artifacts | not in scope of this node | Phase 94 | `gh attestation verify` |
