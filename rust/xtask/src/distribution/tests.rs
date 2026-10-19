//! Purpose: contract tests for `xtask distribution`: baseline asset naming, archive layout and
//! determinism, SHA256SUMS format and tamper detection, manifest drift, SBOM shape and typed errors.
//! Scope: unit tests. Fixture-based (synthetic binaries in temp dirs, a hand-written cargo metadata
//! fixture) except `sbom_live_workspace` and `checked_in_manifest_matches_code`, which read the real
//! workspace metadata and the checked-in manifest.
//! Uses:
//! rust/xtask/
//! |-- src/distribution/archive.rs
//! |-- src/distribution/build.rs
//! |-- src/distribution/matrix.rs
//! |-- src/distribution/mod.rs
//! |-- src/distribution/sbom.rs
//! `-- src/distribution/verify.rs
//! External:
//! |-- archive::write
//! |-- build::require_bin
//! |-- matrix::manifest
//! |-- matrix::render
//! |-- sbom::from_metadata
//! |-- sbom::generate
//! |-- serde_json::Value
//! |-- serde_json::json
//! |-- serde_json::to_string
//! |-- tempfile::TempDir
//! |-- tempfile::tempdir
//! |-- verify::check_dir
//! |-- verify::check_manifest
//! `-- verify::write_checksums
//! Used by:
//! rust/xtask/
//! `-- src/distribution/mod.rs
//! Invariants: tests never build release binaries, touch the network or write outside temp dirs.
//! Contract tests: this file

use serde_json::{Value, json};

use super::matrix::{self, TARGETS};
use super::{DistError, archive, build, sbom, verify, workspace_root};

fn names() -> Vec<String> {
    TARGETS.iter().map(|t| t.archive()).collect()
}

#[test]
fn asset_names_mirror_baseline_build_ts() {
    // Exactly the keys build.ts produces, then `.tar.gz` for linux and `.zip` otherwise.
    assert_eq!(
        names(),
        [
            "opencode-linux-arm64.tar.gz",
            "opencode-linux-x64.tar.gz",
            "opencode-linux-x64-baseline.tar.gz",
            "opencode-linux-arm64-musl.tar.gz",
            "opencode-linux-x64-musl.tar.gz",
            "opencode-linux-x64-baseline-musl.tar.gz",
            "opencode-darwin-arm64.zip",
            "opencode-darwin-x64.zip",
            "opencode-darwin-x64-baseline.zip",
            "opencode-windows-arm64.zip",
            "opencode-windows-x64.zip",
            "opencode-windows-x64-baseline.zip",
        ]
    );
    assert_eq!(
        matrix::find("opencode-windows-x64").unwrap().executable(),
        "opencode.exe"
    );
    assert_eq!(
        matrix::find("linux-arm64-musl").unwrap().triple,
        "aarch64-unknown-linux-musl"
    );
    assert!(!matrix::find("x86_64-apple-darwin").unwrap().baseline);
    assert!(matrix::find("linux-riscv64").is_none());
}

#[test]
fn d13_only_marks_reachable_rows_executable() {
    let m = matrix::manifest();
    let exec: Vec<&str> = m
        .targets
        .iter()
        .filter(|r| r.status == "executable")
        .map(|r| r.asset.as_str())
        .collect();
    assert_eq!(
        exec,
        [
            "opencode-linux-arm64",
            "opencode-linux-arm64-musl",
            "opencode-darwin-arm64"
        ]
    );
    assert!(
        m.targets
            .iter()
            .filter(|r| r.status == "blocked")
            .all(|r| r.reason.is_some() && r.local_runner.is_none())
    );
}

#[test]
fn checked_in_manifest_matches_code() {
    verify::check_manifest(&workspace_root().join("distribution/release-matrix.json")).unwrap();
}

#[test]
fn manifest_drift_is_typed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("m.json");
    let mut m = matrix::manifest();
    m.targets[7].status = "executable".into();
    std::fs::write(&path, matrix::render(&m)).unwrap();
    match verify::check_manifest(&path) {
        Err(DistError::ManifestDrift { detail, .. }) => {
            assert!(detail.contains("opencode-darwin-x64"), "{detail}")
        }
        other => panic!("expected drift, got {other:?}"),
    }
    std::fs::write(&path, serde_json::to_string(&matrix::manifest()).unwrap()).unwrap();
    assert!(matches!(
        verify::check_manifest(&path),
        Err(DistError::ManifestDrift { .. })
    ));
}

#[test]
fn archives_are_deterministic_and_hold_only_the_executable() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("fake-bin");
    std::fs::write(&bin, b"\x7fELF fake opencode").unwrap();
    for name in ["opencode-linux-arm64.tar.gz", "opencode-windows-x64.zip"] {
        let entry = if name.contains("windows") {
            "opencode.exe"
        } else {
            "opencode"
        };
        let a = dir.path().join(format!("a-{name}"));
        let b = dir.path().join(format!("b-{name}"));
        archive::write(&bin, entry, &a).unwrap();
        archive::write(&bin, entry, &b).unwrap();
        assert_eq!(
            std::fs::read(&a).unwrap(),
            std::fs::read(&b).unwrap(),
            "{name} not reproducible"
        );
        assert_eq!(
            archive::list(&a).unwrap(),
            vec![(entry.to_string(), b"\x7fELF fake opencode".to_vec())]
        );
    }
}

fn dist_dir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    std::fs::write(&bin, b"binary").unwrap();
    let out = dir.path().join("dist");
    std::fs::create_dir(&out).unwrap();
    archive::write(&bin, "opencode", &out.join("opencode-linux-arm64.tar.gz")).unwrap();
    archive::write(&bin, "opencode", &out.join("opencode-darwin-arm64.zip")).unwrap();
    std::fs::write(out.join("opencode-darwin-arm64.cdx.json"), "{}\n").unwrap();
    std::fs::write(out.join("notes.txt"), "ignored").unwrap();
    dir
}

#[test]
fn checksums_use_sha256sum_format_and_verify() {
    let dir = dist_dir();
    let out = dir.path().join("dist");
    let text = verify::write_checksums(&out).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 3);
    // sorted, `<64 hex>  <name>`
    assert!(lines[0].ends_with("  opencode-darwin-arm64.cdx.json"));
    assert!(lines.iter().all(|l| {
        l.split_once("  ")
            .is_some_and(|(h, _)| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()))
    }));
    assert!(
        lines[1].starts_with(&verify::sha256_file(&out.join("opencode-darwin-arm64.zip")).unwrap())
    );
    // known vector: sha256("{}\n")
    assert!(
        lines[0].starts_with("ca3d163bab055381827226140568f3bef7eaac187cebd76878e0b63e9e442356")
    );
    assert_eq!(verify::check_dir(&out).unwrap(), 3);
}

#[test]
fn verify_rejects_tampering_unlisted_and_foreign_archives() {
    let dir = dist_dir();
    let out = dir.path().join("dist");
    verify::write_checksums(&out).unwrap();
    std::fs::write(out.join("opencode-darwin-arm64.cdx.json"), "{\"x\":1}\n").unwrap();
    let err = verify::check_dir(&out).unwrap_err();
    assert!(
        matches!(err.downcast_ref(), Some(DistError::ChecksumMismatch { .. })),
        "{err}"
    );

    verify::write_checksums(&out).unwrap();
    std::fs::write(out.join("opencode-linux-x64.tar.gz"), "late").unwrap();
    let err = verify::check_dir(&out).unwrap_err();
    assert!(
        matches!(err.downcast_ref(), Some(DistError::UnlistedArtifact(n)) if n == "opencode-linux-x64.tar.gz")
    );

    std::fs::remove_file(out.join("opencode-linux-x64.tar.gz")).unwrap();
    std::fs::write(out.join("opencode-plan9-x64.zip"), "x").unwrap();
    verify::write_checksums(&out).unwrap();
    let err = verify::check_dir(&out).unwrap_err();
    assert!(
        matches!(err.downcast_ref(), Some(DistError::UnlistedArtifact(n)) if n == "opencode-plan9-x64.zip")
    );

    std::fs::remove_file(out.join("opencode-plan9-x64.zip")).unwrap();
    let bin = dir.path().join("bin");
    archive::write(&bin, "wrong-name", &out.join("opencode-linux-arm64.tar.gz")).unwrap();
    verify::write_checksums(&out).unwrap();
    let err = verify::check_dir(&out).unwrap_err();
    assert!(
        matches!(err.downcast_ref(), Some(DistError::ArchiveContents { .. })),
        "{err}"
    );

    std::fs::remove_file(out.join("opencode-linux-arm64.tar.gz")).unwrap();
    let err = verify::check_dir(&out).unwrap_err();
    assert!(
        matches!(err.downcast_ref(), Some(DistError::ChecksumMissing(n)) if n == "opencode-linux-arm64.tar.gz")
    );
}

fn metadata_fixture() -> Value {
    let pkg = |id: &str,
               name: &str,
               version: &str,
               source: Option<&str>,
               license: Option<&str>,
               bin: bool| {
        json!({ "id": id, "name": name, "version": version, "source": source, "license": license,
                "targets": [{ "name": name, "kind": [if bin { "bin" } else { "lib" }] }] })
    };
    let dep =
        |pkg: &str, kind: Option<&str>| json!({ "pkg": pkg, "dep_kinds": [{ "kind": kind }] });
    let reg = "registry+https://github.com/rust-lang/crates.io-index";
    json!({
        "workspace_members": ["path+file:///w/bins/app#0.0.0"],
        "packages": [
            pkg("path+file:///w/bins/app#0.0.0", "app", "0.0.0", None, Some("MIT"), true),
            pkg("reg#serde@1.0.0", "serde", "1.0.0", Some(reg), Some("MIT OR Apache-2.0"), false),
            pkg("reg#cc@1.2.0", "cc", "1.2.0", Some(reg), Some("MIT OR Apache-2.0"), false),
            pkg("reg#insta@1.0.0", "insta", "1.0.0", Some(reg), Some("Apache-2.0"), false),
            pkg("git#gitdep@0.1.0", "gitdep", "0.1.0", Some("git+https://example.com/g.git#abc"), None, false),
        ],
        "resolve": { "nodes": [
            { "id": "path+file:///w/bins/app#0.0.0", "deps": [
                dep("reg#serde@1.0.0", None), dep("reg#cc@1.2.0", Some("build")),
                dep("reg#insta@1.0.0", Some("dev")), dep("git#gitdep@0.1.0", None) ] },
            { "id": "reg#serde@1.0.0", "deps": [] },
            { "id": "reg#cc@1.2.0", "deps": [] },
            { "id": "reg#insta@1.0.0", "deps": [] },
            { "id": "git#gitdep@0.1.0", "deps": [] },
        ] },
    })
}

const LOCK: &str = r#"version = 4
[[package]]
name = "serde"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
"#;

#[test]
fn sbom_shape_from_fixture() {
    let doc = sbom::from_metadata(&metadata_fixture(), LOCK, "app", Some(b"exe")).unwrap();
    assert_eq!(doc["bomFormat"], "CycloneDX");
    assert_eq!(doc["specVersion"], "1.5");
    assert!(
        doc["serialNumber"]
            .as_str()
            .unwrap()
            .starts_with("urn:uuid:")
    );
    let root = &doc["metadata"]["component"];
    assert_eq!(root["purl"], "pkg:cargo/app@0.0.0");
    assert_eq!(root["type"], "application");
    assert_eq!(root["licenses"][0]["expression"], "MIT");
    // root hash is the artifact digest: sha256("exe")
    assert_eq!(
        root["hashes"][0]["content"],
        "9095bdb859308b62acf04036ffd4adfe366d7f737d276eb6c46ae434f3816c9b"
    );
    let comps = doc["components"].as_array().unwrap();
    let refs: Vec<&str> = comps
        .iter()
        .map(|c| c["bom-ref"].as_str().unwrap())
        .collect();
    assert_eq!(
        refs,
        [
            "pkg:cargo/cc@1.2.0",
            "pkg:cargo/gitdep@0.1.0?vcs_url=git+https%3A//example.com/g.git%23abc",
            "pkg:cargo/serde@1.0.0"
        ]
    );
    let serde = &comps[2];
    assert_eq!(serde["licenses"][0]["expression"], "MIT OR Apache-2.0");
    assert_eq!(serde["hashes"][0]["alg"], "SHA-256");
    assert_eq!(serde["hashes"][0]["content"], "a".repeat(64));
    assert!(serde.get("scope").is_none());
    assert_eq!(comps[0]["scope"], "excluded", "build-only dependency");
    assert!(comps[1].get("licenses").is_none());
    let deps = doc["dependencies"].as_array().unwrap();
    let app = deps
        .iter()
        .find(|d| d["ref"] == "pkg:cargo/app@0.0.0")
        .unwrap();
    assert_eq!(
        app["dependsOn"].as_array().unwrap().len(),
        3,
        "dev dependency excluded"
    );
    // reproducible
    assert_eq!(
        doc,
        sbom::from_metadata(&metadata_fixture(), LOCK, "app", Some(b"exe")).unwrap()
    );
}

#[test]
fn sbom_and_build_reject_missing_package_typed() {
    assert!(matches!(
        sbom::from_metadata(&metadata_fixture(), LOCK, "opencode-nope", None),
        Err(DistError::PackageMissing(_))
    ));
    assert!(matches!(
        build::require_bin(&metadata_fixture(), "nope", "nope"),
        Err(DistError::PackageMissing(_))
    ));
    assert!(matches!(
        build::require_bin(&metadata_fixture(), "app", "other"),
        Err(DistError::BinMissing { .. })
    ));
    build::require_bin(&metadata_fixture(), "app", "app").unwrap();
}

#[test]
fn sbom_live_workspace() {
    // Live: real `cargo metadata` + Cargo.lock for the xtask package itself.
    let doc = sbom::generate(&workspace_root(), "xtask", None, None).unwrap();
    let comps = doc["components"].as_array().unwrap();
    assert!(comps.iter().any(|c| {
        c["name"] == "serde_json"
            && c["hashes"][0]["content"]
                .as_str()
                .is_some_and(|h| h.len() == 64)
    }));
    assert!(comps.iter().all(|c| {
        c["purl"]
            .as_str()
            .is_some_and(|p| p.starts_with("pkg:cargo/"))
    }));
    assert!(
        !comps.iter().any(|c| c["name"] == "tempfile"),
        "dev-dependencies must not appear"
    );
}

#[test]
fn flags_parse_and_reject_positionals() {
    let args: Vec<String> = ["--package", "xtask", "--no-sbom", "--out", "d"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let flags = super::parse_flags(&args).unwrap();
    assert_eq!(flags["package"], "xtask");
    assert_eq!(flags["no-sbom"], "");
    assert_eq!(flags["out"], "d");
    assert!(matches!(
        super::parse_flags(&["stray".to_string()]),
        Err(DistError::Usage(_))
    ));
}
