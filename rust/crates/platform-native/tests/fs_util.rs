//! Purpose: real temp-dir tests for `NativeFileSystem` helpers (safe reads, `up`/`findUp`,
//! `globUp`, glob dot/symlink handling, write_with_dirs, read_range, error kinds) and the
//! ignore/protected rule sets.
//! Scope: live filesystem tests (tempfile), no fixtures.
//! Uses:
//! rust/crates/
//! |-- platform-api/src/fs.rs
//! |-- platform-native/src/fs/glob.rs
//! |-- platform-native/src/fs/ignore.rs
//! `-- platform-native/src/fs/native.rs
//! External:
//! `-- tempfile::TempDir
//! Used by:
//! `-- none, entrypoint
//! Invariants: every test owns its temp dirs and never touches user data.
//! Contract tests: platform-native/tests/fs_util.rs

use opencode_platform_api::PlatformErrorKind;
use opencode_platform_api::fs::{FileSystem, FileType};
use opencode_platform_native::fs::ignore::{
    FOLDERS, ignore_match, patterns, protected_below, protected_paths_for,
};
use opencode_platform_native::fs::{GlobOptions, NativeFileSystem, glob_match};
use std::path::{Path, PathBuf};

fn tmp() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    (dir, root)
}

fn touch(path: &Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, "x").unwrap();
}

#[tokio::test]
async fn read_to_string_safe_treats_missing_as_none_and_strips_bom() {
    let (_dir, root) = tmp();
    let fs = NativeFileSystem::new();
    assert_eq!(
        fs.read_to_string_safe(&root.join("missing")).await.unwrap(),
        None
    );
    std::fs::write(root.join("bom.txt"), "\u{feff}hi").unwrap();
    assert_eq!(
        fs.read_to_string_safe(&root.join("bom.txt"))
            .await
            .unwrap()
            .as_deref(),
        Some("hi")
    );
    std::fs::write(root.join("bad.txt"), [0x61, 0xff, 0x62]).unwrap();
    assert_eq!(
        fs.read_to_string(&root.join("bad.txt")).await.unwrap(),
        "a\u{fffd}b"
    );
    // A directory is not NotFound/PermissionDenied: the baseline surfaces the error.
    let error = fs.read_to_string_safe(&root).await.unwrap_err();
    assert_eq!(error.kind, PlatformErrorKind::IsADirectory);
}

#[cfg(unix)]
#[tokio::test]
async fn read_to_string_safe_treats_permission_denied_as_none() {
    use std::os::unix::fs::PermissionsExt;
    let (_dir, root) = tmp();
    let path = root.join("secret.txt");
    std::fs::write(&path, "s").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
    let readable = std::fs::read(&path).is_ok(); // running as root bypasses modes
    let result = NativeFileSystem::new()
        .read_to_string_safe(&path)
        .await
        .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    if !readable {
        assert_eq!(result, None);
    }
}

#[tokio::test]
async fn up_collects_targets_nearest_first_and_stops_at_stop() {
    let (_dir, root) = tmp();
    let deep = root.join("a/b/c");
    std::fs::create_dir_all(&deep).unwrap();
    touch(&root.join("opencode.json"));
    touch(&root.join("a/opencode.jsonc"));
    touch(&root.join("a/opencode.json"));
    touch(&root.join("a/b/c/opencode.json"));
    let fs = NativeFileSystem::new();
    let found = fs
        .up(&["opencode.jsonc", "opencode.json"], &deep, Some(&root))
        .await
        .unwrap();
    assert_eq!(
        found,
        vec![
            deep.join("opencode.json"),
            root.join("a/opencode.jsonc"),
            root.join("a/opencode.json"),
            root.join("opencode.json"),
        ]
    );
    let stopped = fs
        .up(&["opencode.json"], &deep, Some(&root.join("a")))
        .await
        .unwrap();
    assert_eq!(
        stopped,
        vec![deep.join("opencode.json"), root.join("a/opencode.json")]
    );
    let find = fs
        .find_up("opencode.jsonc", &deep, Some(&root))
        .await
        .unwrap();
    assert_eq!(find, vec![root.join("a/opencode.jsonc")]);
}

#[tokio::test]
async fn glob_follows_brace_recursive_dot_and_symlink_rules() {
    let (_dir, root) = tmp();
    touch(&root.join("agent/a.md"));
    touch(&root.join("agents/nested/.hidden/b.md"));
    touch(&root.join("agents/c.txt"));
    touch(&root.join("agent/.d.md"));
    let fs = NativeFileSystem::new();
    let options = |dot, follow| GlobOptions {
        cwd: root.clone(),
        absolute: false,
        include_dirs: false,
        dot,
        follow_symlinks: follow,
    };
    let all = fs
        .glob("{agent,agents}/**/*.md", options(true, true))
        .await
        .unwrap();
    assert_eq!(
        all,
        vec!["agent/.d.md", "agent/a.md", "agents/nested/.hidden/b.md"]
    );
    let visible = fs
        .glob("{agent,agents}/**/*.md", options(false, true))
        .await
        .unwrap();
    assert_eq!(visible, vec!["agent/a.md"]);
    let shallow = fs
        .glob("{agent,agents}/*.md", options(true, true))
        .await
        .unwrap();
    assert_eq!(shallow, vec!["agent/.d.md", "agent/a.md"]);
    assert_eq!(
        fs.glob("missing/**/*.md", options(true, true))
            .await
            .unwrap(),
        Vec::<String>::new()
    );

    #[cfg(unix)]
    {
        let outside = tempfile::tempdir().unwrap();
        touch(&outside.path().join("linked.md"));
        std::os::unix::fs::symlink(outside.path(), root.join("agent/link")).unwrap();
        // Self-referential loop must not hang or fail the scan.
        std::os::unix::fs::symlink(root.join("agent"), root.join("agent/loop")).unwrap();
        let followed = fs.glob("agent/**/*.md", options(true, true)).await.unwrap();
        assert!(
            followed.contains(&"agent/link/linked.md".to_string()),
            "{followed:?}"
        );
        let unfollowed = fs
            .glob("agent/**/*.md", options(true, false))
            .await
            .unwrap();
        assert!(
            !unfollowed.iter().any(|item| item.contains("link")),
            "{unfollowed:?}"
        );
    }

    let absolute = fs
        .glob(
            "agent/*.md",
            GlobOptions {
                absolute: true,
                ..options(false, false)
            },
        )
        .await
        .unwrap();
    assert_eq!(
        absolute,
        vec![root.join("agent/a.md").to_string_lossy().into_owned()]
    );
}

#[tokio::test]
async fn glob_up_collects_matches_from_every_ancestor() {
    let (_dir, root) = tmp();
    let deep = root.join("x/y");
    std::fs::create_dir_all(&deep).unwrap();
    touch(&root.join("AGENTS.md"));
    touch(&root.join("x/y/AGENTS.md"));
    touch(&root.join("x/.AGENTS.md"));
    let found = NativeFileSystem::new()
        .glob_up("*AGENTS.md", &deep, Some(&root))
        .await;
    assert_eq!(
        found,
        vec![
            deep.join("AGENTS.md"),
            root.join("x/.AGENTS.md"),
            root.join("AGENTS.md")
        ]
    );
}

#[tokio::test]
async fn native_filesystem_basics_and_error_kinds() {
    let (_dir, root) = tmp();
    let fs = NativeFileSystem::new();
    let file = root.join("deep/nested/file.txt");
    fs.write_with_dirs(&file, b"0123456789").await.unwrap();
    assert_eq!(fs.read_range(&file, 3, 4).await.unwrap(), b"3456");
    assert_eq!(fs.read_range(&file, 8, 10).await.unwrap(), b"89");
    assert!(fs.read_range(&file, 50, 10).await.unwrap().is_empty());
    let meta = fs.metadata(&file).await.unwrap();
    assert_eq!((meta.file_type, meta.size), (FileType::File, 10));
    assert!(meta.identity.is_some() || cfg!(windows));
    assert!(fs.exists(&file).await.unwrap());
    assert!(!fs.exists(&root.join("nope")).await.unwrap());
    assert!(fs.is_file(&file).await && fs.is_dir(&root.join("deep")).await);
    let entries = fs.read_dir(&root.join("deep")).await.unwrap();
    assert_eq!(entries, vec![("nested".to_string(), FileType::Directory)]);
    let missing = fs.read(&root.join("nope")).await.unwrap_err();
    assert_eq!(missing.kind, PlatformErrorKind::NotFound);
    let below_file = fs.read(&file.join("child")).await.unwrap_err();
    assert_eq!(below_file.kind, PlatformErrorKind::NotADirectory);
    fs.ensure_dir(&root.join("deep")).await.unwrap();
    fs.rename(&file, &root.join("moved.txt")).await.unwrap();
    fs.remove_file(&root.join("moved.txt")).await.unwrap();
    fs.remove_dir_all(&root.join("deep")).await.unwrap();
    assert!(!fs.exists(&root.join("deep")).await.unwrap());
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(root.join("target"), root.join("dangling")).unwrap();
        let lstat = fs.symlink_metadata(&root.join("dangling")).await.unwrap();
        assert_eq!(lstat.file_type, FileType::SymbolicLink);
        assert!(!fs.exists(&root.join("dangling")).await.unwrap());
        assert!(
            fs.real_path(&root.join("dangling"))
                .await
                .unwrap_err()
                .is_not_found()
        );
    }
}

#[test]
fn ignore_matches_nested_and_non_nested_folders() {
    for path in [
        "node_modules/index.js",
        "node_modules",
        "node_modules/",
        "node_modules/bar",
        "node_modules/bar/",
    ] {
        assert!(ignore_match(path, &[], &[]), "{path}");
    }
    assert!(ignore_match("src/app.log", &[], &[]));
    assert!(ignore_match("a/logs/x.txt", &[], &[]));
    assert!(ignore_match(".DS_Store", &[], &[]));
    assert!(
        !ignore_match("logs", &[], &[]),
        "minimatch **/logs/** needs a child"
    );
    assert!(!ignore_match("src/main.rs", &[], &[]));
    assert!(ignore_match("src/gen.rs", &["src/gen.*".into()], &[]));
    assert!(!ignore_match("dist/keep.js", &[], &["dist/**".into()]));
    assert_eq!(patterns().len(), FOLDERS.len() + 11);
    assert_eq!(patterns()[0], "**/*.swp");
    assert!(glob_match("**/*.log", "x/.a.log"));
    assert!(!glob_match("*.md", "a/b.md"));
}

#[test]
fn protected_paths_follow_the_platform_tables() {
    let home = Path::new("/Users/someone");
    let paths = protected_paths_for(home);
    if cfg!(target_os = "macos") {
        assert_eq!(paths.len(), 9 + 10 + 4);
        assert!(paths.contains(&home.join("Library/Application Support/com.apple.TCC")));
        assert!(paths.contains(&PathBuf::from("/.fseventsd")));
        let below = protected_below(home, &paths);
        assert_eq!(below.len(), 19, "root-level entries are not below home");
        assert!(
            protected_below(&home.join("Documents"), &paths).is_empty(),
            "self is excluded"
        );
    } else if !cfg!(windows) {
        assert!(paths.is_empty());
    }
}
