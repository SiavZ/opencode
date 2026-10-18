//! Purpose: live ripgrep adapter conformance (port of packages/core/test/ripgrep.test.ts) plus
//! limits, invalid patterns, exact-file grep and binary resolution order.
//! Scope: real `rg` processes in temp dirs.
//! Uses:
//! rust/crates/
//! |-- tools/src/search/binary.rs
//! |-- tools/src/search/ripgrep.rs
//! `-- tools/tests/search_support.rs
//! External:
//! `-- tempfile::tempdir
//! Used by:
//! `-- none, entrypoint
//! Invariants: `.git` metadata never appears; ignored files stay out of catch-all results.
//! Contract tests: this integration suite

#[path = "search_support.rs"]
mod support;

use opencode_tools::search::binary::{RipgrepBinary, download_url, platform_asset};
use opencode_tools::search::ripgrep::{GlobInput, GrepInput};
use opencode_tools::search::{NoInstaller, RipgrepError};
use std::sync::Arc;
use support::{ripgrep, temp, write};

fn paths(entries: &[opencode_schema::filesystem::Entry]) -> Vec<String> {
    entries.iter().map(|entry| entry.path.to_string()).collect()
}

#[tokio::test]
async fn keeps_ignored_files_out_of_catch_all_find_results() {
    let (_guard, dir) = temp();
    std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(&dir)
        .status()
        .unwrap();
    write(&dir.join(".gitignore"), "node_modules/\n");
    write(&dir.join("node_modules/pkg/index.js"), "ignored\n");
    write(&dir.join("src/index.js"), "included\n");
    let files = ripgrep()
        .find(GlobInput {
            cwd: dir.clone(),
            pattern: "*".into(),
            limit: 10,
            ..GlobInput::default()
        })
        .await
        .unwrap();
    let files = paths(&files);
    assert!(files.contains(&"src/index.js".to_string()));
    assert!(!files.contains(&"node_modules/pkg/index.js".to_string()));
}

#[tokio::test]
async fn never_includes_git_metadata_and_limit_stops_early() {
    let (_guard, dir) = temp();
    write(&dir.join(".opencode/config"), "needle\n");
    write(&dir.join(".git/config"), "needle\n");
    let rg = ripgrep();
    let input = GlobInput {
        cwd: dir.clone(),
        pattern: "**/*".into(),
        limit: 10,
        hidden: true,
        ..GlobInput::default()
    };
    let files = paths(&rg.find(input.clone()).await.unwrap());
    assert!(files.contains(&".opencode/config".to_string()));
    assert!(!files.contains(&".git/config".to_string()));
    let limited = rg.find(GlobInput { limit: 1, ..input }).await.unwrap();
    assert_eq!(limited.len(), 1);
    let matches = rg
        .grep(GrepInput {
            cwd: dir.clone(),
            pattern: "needle".into(),
            include: Some("config".into()),
            limit: 10,
            ..GrepInput::default()
        })
        .await
        .unwrap();
    let files: Vec<String> = matches.iter().map(|m| m.entry.path.to_string()).collect();
    assert!(files.contains(&".opencode/config".to_string()));
    assert!(!files.contains(&".git/config".to_string()));
}

#[tokio::test]
async fn does_not_split_surrogate_pairs_in_oversized_previews() {
    let (_guard, dir) = temp();
    write(
        &dir.join("unicode.txt"),
        &format!("needle{}😀\n", "x".repeat(1_993)),
    );
    let matches = ripgrep()
        .grep(GrepInput {
            cwd: dir.clone(),
            pattern: "needle".into(),
            limit: 10,
            ..GrepInput::default()
        })
        .await
        .unwrap();
    assert_eq!(matches[0].text, format!("needle{}...", "x".repeat(1_993)));
}

#[tokio::test]
async fn glob_hidden_flag_limit_and_no_match() {
    let (_guard, dir) = temp();
    write(&dir.join("a.ts"), "a");
    write(&dir.join(".hidden/b.ts"), "b");
    let rg = ripgrep();
    let input = GlobInput {
        cwd: dir.clone(),
        pattern: "**/*.ts".into(),
        limit: 100,
        ..GlobInput::default()
    };
    assert_eq!(paths(&rg.glob(input.clone()).await.unwrap()), ["a.ts"]);
    let mut hidden = paths(
        &rg.glob(GlobInput {
            hidden: true,
            ..input.clone()
        })
        .await
        .unwrap(),
    );
    hidden.sort();
    assert_eq!(hidden, [".hidden/b.ts", "a.ts"]);
    let none = rg
        .glob(GlobInput {
            pattern: "*.nope".into(),
            ..input
        })
        .await
        .unwrap();
    assert!(none.is_empty());
}

#[tokio::test]
async fn grep_rows_limits_exact_file_and_invalid_pattern() {
    let (_guard, dir) = temp();
    for index in 0..30 {
        write(
            &dir.join(format!("f{index}.txt")),
            "needle one\nneedle two\n",
        );
    }
    let rg = ripgrep();
    let input = GrepInput {
        cwd: dir.clone(),
        pattern: "needle (\\w+)".into(),
        limit: 5,
        ..GrepInput::default()
    };
    let rows = rg.grep(input.clone()).await.unwrap();
    assert_eq!(rows.len(), 5);
    assert_eq!(rows[0].submatches[0].start.get(), 0);
    let exact = rg
        .grep(GrepInput {
            file: Some("f3.txt".into()),
            limit: 100,
            ..input.clone()
        })
        .await
        .unwrap();
    assert_eq!(exact.len(), 2);
    assert!(exact.iter().all(|m| m.entry.path.to_string() == "f3.txt"));
    assert_eq!(exact[1].line.get(), 2);
    assert_eq!(exact[1].offset.get(), 11);
    assert_eq!(exact[1].text, "needle two\n");
    let error = rg
        .grep(GrepInput {
            pattern: "(".into(),
            ..input.clone()
        })
        .await
        .unwrap_err();
    assert!(
        matches!(error, RipgrepError::InvalidPattern { .. }),
        "{error:?}"
    );
    let none = rg
        .grep(GrepInput {
            pattern: "absent-xyz".into(),
            ..input
        })
        .await
        .unwrap();
    assert!(none.is_empty());
}

#[tokio::test]
async fn binary_resolution_prefers_path_then_managed_bin_then_installer() {
    let (_guard, dir) = temp();
    let empty = dir.join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    let managed = dir.join("bin");
    let binary = RipgrepBinary::new(
        &managed,
        Some(empty.to_string_lossy().into()),
        Arc::new(NoInstaller),
    );
    let error = binary.filepath().await.unwrap_err();
    assert!(error.starts_with("ripgrep is not installed and downloading is not ported yet: https://github.com/BurntSushi/ripgrep/releases/download/15.1.0/ripgrep-15.1.0-"), "{error}");
    // Managed bin is appended to PATH, so a managed rg is found.
    write(&managed.join("rg"), "#!/bin/sh\n");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(managed.join("rg"), std::fs::Permissions::from_mode(0o755))
            .unwrap();
    }
    let binary = RipgrepBinary::new(
        &managed,
        Some(empty.to_string_lossy().into()),
        Arc::new(NoInstaller),
    );
    assert_eq!(binary.filepath().await.unwrap(), managed.join("rg"));
    assert_eq!(
        platform_asset("aarch64", "macos"),
        Some(("aarch64-apple-darwin", "tar.gz"))
    );
    assert_eq!(platform_asset("riscv64", "linux"), None);
    assert_eq!(
        download_url("x86_64-pc-windows-msvc", "zip"),
        "https://github.com/BurntSushi/ripgrep/releases/download/15.1.0/ripgrep-15.1.0-x86_64-pc-windows-msvc.zip"
    );
}
