//! Purpose: verify custom theme file discovery on real isolated directories.
//! Scope: temporary filesystem directories, no real user configuration.
//! Uses:
//! rust/crates/
//! `-- ui-terminal/src/theme/mod.rs
//! External:
//! |-- serde_json::json
//! `-- tempfile::tempdir
//! Used by:
//! `-- none, entrypoint
//! Invariants: dot files, symlinks and platform case matching follow the baseline glob.
//! Contract tests: ui-terminal/tests/theme_discovery.rs

use opencode_ui_terminal::theme::{DiscoverError, discover_themes};
use serde_json::json;

#[test]
fn dot_files_and_platform_specific_extensions_match_the_baseline_glob() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir(tmp.path().join("themes")).unwrap();
    for name in [
        ".json",
        ".hidden.json",
        "upper.JSON",
        "lower.json",
        "skip.txt",
    ] {
        std::fs::write(tmp.path().join("themes").join(name), "{}").unwrap();
    }
    std::fs::create_dir(tmp.path().join("themes/directory.json")).unwrap();
    let found = discover_themes(&[tmp.path().to_path_buf()]).unwrap();
    for name in [".json", ".hidden", "lower"] {
        assert_eq!(found.get(name), Some(&json!({})));
    }
    assert!(!found.contains_key("skip"));
    assert!(!found.contains_key("directory"));
    assert_eq!(
        found.contains_key("upper.JSON"),
        cfg!(any(target_os = "macos", windows))
    );
    assert!(!found.contains_key("upper"));
}

#[cfg(unix)]
#[test]
fn symlinks_are_read_and_missing_directories_are_ignored() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir(tmp.path().join("themes")).unwrap();
    let original = tmp.path().join("original.json");
    std::fs::write(&original, r#"{"theme":{}}"#).unwrap();
    std::os::unix::fs::symlink(&original, tmp.path().join("themes/linked.json")).unwrap();
    let found = discover_themes(&[tmp.path().join("missing"), tmp.path().to_path_buf()]).unwrap();
    assert_eq!(found.get("linked"), Some(&json!({"theme":{}})));
}

#[test]
fn json_errors_retain_the_path_and_parser_source() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir(tmp.path().join("themes")).unwrap();
    let file = tmp.path().join("themes/broken.json");
    std::fs::write(&file, "{").unwrap();
    let error = discover_themes(&[tmp.path().to_path_buf()]).unwrap_err();
    match error {
        DiscoverError::Json { path, source } => {
            assert_eq!(path, file);
            assert!(source.is_eof());
        }
        error => panic!("expected JSON error, got {error}"),
    }
}

#[test]
fn read_errors_retain_the_path_and_io_kind() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir(tmp.path().join("themes")).unwrap();
    let file = tmp.path().join("themes/non-utf8.json");
    std::fs::write(&file, [0xff]).unwrap();
    let error = discover_themes(&[tmp.path().to_path_buf()]).unwrap_err();
    match error {
        DiscoverError::Read { path, source } => {
            assert_eq!(path, file);
            assert_eq!(source.kind(), std::io::ErrorKind::InvalidData);
        }
        error => panic!("expected I/O error, got {error}"),
    }
}
