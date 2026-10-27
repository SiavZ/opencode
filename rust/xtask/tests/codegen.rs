//! Purpose: exercise real Protocol-derived generation, deterministic output and manifest-only writing.
//! Scope: isolated temporary output trees; packages and user data untouched.
//! Uses:
//! `-- rust/xtask/src/codegen/mod.rs
//! External:
//! |-- opencode_protocol
//! |-- tempfile
//! `-- syn
//! Used by:
//! `-- cargo test -p xtask
//! Invariants: failed safety checks preserve unrelated files.
//! Contract tests: this file.
use std::collections::BTreeMap;
use xtask::codegen::{self, Protocol};

#[test]
fn generated_tree_is_deterministic_parseable_complete_and_bounded() {
    let files = codegen::generate(&Protocol).unwrap();
    assert_eq!(files, codegen::generate(&Protocol).unwrap());
    for (path, content) in &files {
        syn::parse_file(content).unwrap_or_else(|error| panic!("{path}: {error}"));
        assert!(content.lines().count() < 500, "{path} exceeds line limit");
    }
    for (api, registry) in [
        ("current", opencode_protocol::current::api()),
        ("legacy", opencode_protocol::legacy::api()),
    ] {
        let operations = files
            .iter()
            .filter(|(path, _)| path.starts_with(&format!("{api}/")) && path.ends_with("/mod.rs"))
            .map(|(_, content)| content.as_str())
            .collect::<String>();
        for (_, endpoint) in registry.endpoints() {
            assert!(
                operations.contains(&format!("{:?}", endpoint.name)),
                "missing {api}: {}",
                endpoint.name
            );
            assert!(
                operations.contains(&format!("{:?}", endpoint.path)),
                "wrong path for {}",
                endpoint.name
            );
        }
    }
    assert!(files.contains_key("current/files/read.rs"));
    assert!(files.contains_key("current/ptys/connect.rs"));
    assert!(files.contains_key("current/ptys/connect_token.rs"));
}
#[test]
fn checked_in_rust_client_matches_real_registry_bytes() {
    codegen::write(
        &xtask::workspace::rust_root()
            .unwrap()
            .join("crates/client/src/generated"),
        &codegen::generate(&Protocol).unwrap(),
        true,
    )
    .unwrap();
}
#[test]
fn writer_checks_without_mutating_and_only_removes_prior_owned_entries() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let first = BTreeMap::from([("old.rs".into(), "old\n".into())]);
    codegen::write(&root, &first, false).unwrap();
    std::fs::write(root.join("unrelated"), "preserved").unwrap();
    let second = BTreeMap::from([("new.rs".into(), "new\n".into())]);
    assert!(codegen::write(&root, &second, true).is_err());
    assert!(root.join("old.rs").exists());
    assert!(!root.join("new.rs").exists());
    codegen::write(&root, &second, false).unwrap();
    assert!(!root.join("old.rs").exists());
    assert_eq!(
        std::fs::read_to_string(root.join("unrelated")).unwrap(),
        "preserved"
    );
    codegen::write(&root, &second, true).unwrap();
}
#[test]
fn writer_rejects_traversal_absolute_case_collisions_and_unowned_files() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    for path in ["../escape", "/absolute", "a/../b", "C:\\bad", ""] {
        assert!(
            codegen::write(&root, &BTreeMap::from([(path.into(), "bad".into())]), false).is_err()
        );
    }
    assert!(
        codegen::write(
            &root,
            &BTreeMap::from([("A.rs".into(), "a".into()), ("a.rs".into(), "b".into())]),
            false
        )
        .is_err()
    );
    std::fs::write(root.join("owned-by-human.rs"), "preserve").unwrap();
    assert!(
        codegen::write(
            &root,
            &BTreeMap::from([("owned-by-human.rs".into(), "overwrite".into())]),
            false
        )
        .is_err()
    );
    assert_eq!(
        std::fs::read_to_string(root.join("owned-by-human.rs")).unwrap(),
        "preserve"
    );
}
#[cfg(unix)]
#[test]
fn writer_rejects_symlinked_manifest_and_destinations() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path(), root.join("link")).unwrap();
    assert!(
        codegen::write(
            &root,
            &BTreeMap::from([("link/file.rs".into(), "bad".into())]),
            false
        )
        .is_err()
    );
    std::fs::write(outside.path().join("manifest"), "[]").unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("manifest"),
        root.join(".rust-codegen.json"),
    )
    .unwrap();
    assert!(codegen::write(&root, &BTreeMap::new(), false).is_err());
}
