# Runbook: compromised release

Trigger: an artifact, checksum, SBOM, container image or install script that was not produced by `.github/workflows/rust.yml` from a reviewed commit, a leaked signing credential, or a malicious dependency found in a shipped SBOM.

## Immediate (Sev 1, see `incident-response.md`)

1. **Stop distribution.**
   - GitHub release: set it to draft so `install` and Homebrew/AUR URL resolution fail closed. Keep the assets privately for forensics.
   - npm: `npm deprecate` the affected versions with a security notice. Do not unpublish until forensics has copies.
   - Container registry: delete or retag the affected digests and publish the bad digest list.
   - Pause every release workflow (disable `publish.yml` / the Rust release job).
2. **Revoke trust material**: signing certificates (Apple Developer ID, Azure Trusted Signing profile) and GitHub tokens with release scope. See `credential-rotation.md`.

## Determine blast radius

1. Compare the published assets against a clean rebuild: `cargo xtask distribution build --target <row>` at the release tag on a trusted runner. Archives are deterministic for the same toolchain and inputs, so an unexpected difference is a signal. `SHA256SUMS` from the CI run artifact (`release-candidate`) is the reference, not the one attached to the release.
2. `cargo xtask distribution verify --dir <downloaded release>` flags checksum mismatches, unlisted files, non-matrix archive names and archives with extra entries.
3. SBOM: diff `<asset>.cdx.json` against the release tag's `Cargo.lock` (`cargo xtask distribution sbom --target <row>`). Run `cargo audit` and `cargo deny check` at the tag.
4. Download counts per asset and time window give the number of affected users.

## Recover

1. Fix the root cause: rotate credentials, pin or remove the dependency, harden the workflow (pinned SHAs, least-privilege `permissions:`).
2. Cut a **new version**, never a re-upload under the same version, through the full CI matrix. Run `distribution verify` and sign.
3. Publish an advisory (GitHub Security Advisory) with affected versions/assets/digests, the checksums of the bad files and upgrade instructions.
4. Users: upgrade, then rotate any provider credentials the compromised binary could read (`auth.json`).

## Verification

- [ ] No affected asset is resolvable from `install`, npm, Homebrew, AUR or the registry.
- [ ] The replacement passes `distribution verify` and its checksums match the CI artifact.
- [ ] Revoked certificates/tokens are confirmed revoked at their issuers.

## What is untested

Not exercised. Release, signing, npm and registry steps involve real publishing, which is forbidden here (Phase 94 owns them). Reproducible-build comparison was observed only for a same-machine rebuild of `opencode-darwin-arm64.zip`, not across runners. `distribution verify` detection paths are covered by unit tests with synthetic tampering.
