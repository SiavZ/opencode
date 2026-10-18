//! Purpose: contract tests for the platform matrix: the shipped matrix matches D13 (only
//! aarch64-apple-darwin host and docker linux/arm64 rows executable), unreachable runners and
//! missing artifacts are blocked, and an executed host smoke passes or fails on real output.
//! Scope: fixture tests with an injected Host; the host smoke runs a real /bin/sh script.
//! Uses:
//! rust/tests/
//! `-- platform/src/lib.rs
//! External:
//! `-- none
//! Used by:
//! `-- none, entrypoint
//! Invariants: tests never invoke docker (Host.docker is injected false).
//! Contract tests: this file

use opencode_platform_runner::{Host, Matrix, Runner};
use std::path::Path;

fn shipped() -> Matrix {
    Matrix::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("matrix.toml")).expect("matrix")
}

fn mac() -> Host {
    Host {
        triple: "aarch64-apple-darwin".into(),
        docker: false,
    }
}

#[test]
fn shipped_matrix_follows_d13() {
    let m = shipped();
    let executable: Vec<_> = m
        .targets
        .iter()
        .filter(|r| r.runner != Runner::None)
        .map(|r| (r.id.as_str(), r.docker_platform.as_deref()))
        .collect();
    assert_eq!(
        executable,
        [
            ("aarch64-apple-darwin", None),
            ("aarch64-unknown-linux-gnu", Some("linux/arm64")),
            ("aarch64-unknown-linux-musl", Some("linux/arm64")),
        ]
    );
    for id in [
        "x86_64-pc-windows-msvc",
        "x86_64-apple-darwin",
        "x86_64-unknown-linux-gnu",
    ] {
        let row = m.targets.iter().find(|r| r.id == id).expect(id);
        assert_eq!(
            row.run(&mac(), None).decision,
            "blocked: runner unavailable (D13)"
        );
    }
}

#[test]
fn missing_docker_and_artifact_block() {
    let m = shipped();
    let linux = m
        .targets
        .iter()
        .find(|r| r.id == "aarch64-unknown-linux-gnu")
        .expect("row");
    assert_eq!(
        linux.run(&mac(), None).decision,
        "blocked: docker daemon unavailable"
    );
    let host = &m.targets[0];
    assert_eq!(
        host.run(&mac(), None).decision,
        "blocked: artifact not built (OPENCODE_ARTIFACT_AARCH64_APPLE_DARWIN unset)"
    );
    let other = Host {
        triple: "x86_64-unknown-linux-gnu".into(),
        docker: false,
    };
    assert!(
        host.run(&other, None)
            .decision
            .starts_with("blocked: host is")
    );
}

#[test]
fn host_smoke_passes_and_fails_on_real_process() {
    let dir = tempfile_dir();
    let good = dir.join("good");
    let bad = dir.join("bad");
    std::fs::write(&good, "#!/bin/sh\necho 0.0.0\n").expect("write");
    std::fs::write(&bad, "#!/bin/sh\nexit 3\n").expect("write");
    for p in [&good, &bad] {
        std::process::Command::new("chmod")
            .arg("+x")
            .arg(p)
            .status()
            .expect("chmod");
    }
    let row = &shipped().targets[0];
    let pass = row.run(&mac(), Some(good));
    assert!(pass.decision.starts_with("pass"), "{pass:?}");
    assert_eq!(pass.detail, "--version -> 0.0.0");
    assert_eq!(row.run(&mac(), Some(bad)).decision, "fail");
    std::fs::remove_dir_all(dir).expect("cleanup");
}

fn tempfile_dir() -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("opencode-p15-platform-{}", std::process::id()));
    std::fs::create_dir_all(&d).expect("mkdir");
    d
}
