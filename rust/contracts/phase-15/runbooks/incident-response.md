# Runbook: incident response

Scope: user-facing incidents in the Rust `opencode` CLI/server/container and its release channel. Hosted console incidents follow Phase 12/13 runbooks. Compromised artifacts follow `compromised-release.md`.

## Severity

| Sev | Examples | Response |
| --- | --- | --- |
| 1 | data loss/corruption, credential leak, malicious artifact, RCE | immediate, stop the release channel, all hands |
| 2 | server will not start on a supported target, provider calls broken for everyone | same day |
| 3 | one platform/provider degraded, workaround exists | next working day |

## Roles

Incident lead (decides, owns the timeline), operator (runs commands), communicator (status page, GitHub issue, Discord). The roster is still to be assigned (Phase 93). Recovery authority for pulling a release is the Phase 94 release owner.

## Steps

1. **Declare.** Open an incident issue/channel, set the severity, and start a UTC timeline.
2. **Contain.**
   - Bad release: mark the GitHub release as pre-release or draft so `install` stops resolving it, and stop channel promotion. Do not delete assets until forensics has copies.
   - Data risk: tell users to stop opencode and take an online backup (`sqlite-backup-restore.md`).
   - Credential exposure: `credential-rotation.md`.
3. **Diagnose.** Collect `opencode --version`, OS/arch/asset name, logs from `$XDG_DATA_HOME/opencode/log/`, OTLP traces if an exporter is configured, `PRAGMA integrity_check` output, and the SBOM of the release (`<asset>.cdx.json`) to check for vulnerable dependencies (`cargo audit --file Cargo.lock` at the release tag).
4. **Mitigate.** Roll back (`install-upgrade-rollback.md`) or ship a fix through the normal CI matrix. Never hot-patch artifacts by hand: every artifact must come from `rust.yml` with matching `SHA256SUMS`.
5. **Verify recovery.** Run `distribution verify` on the replacement release. Health is 200 on the affected targets, the reporter confirms, and error rates are back to baseline.
6. **Close.** Write a blameless postmortem within 5 working days: timeline, root cause, detection gap, follow-ups with owners.

## Verification

- [ ] Every action is in the timeline with UTC timestamps.
- [ ] The replacement artifacts passed `distribution verify` and the CI matrix.
- [ ] Users are notified on all channels the incident touched.

## What is untested

Nothing in this runbook has been exercised: no staged incident drill yet (Phase 93), no on-call roster, no status page integration. Log paths assume baseline layout parity and are not verified for the Rust port.
