//! Purpose: formatter table, config merge, enablement detection and invocation on an edited file.
//! Scope: `Formatters` with a temp PATH holding fake formatter scripts (unix). Live processes.
//! Uses:
//! rust/crates/
//! |-- platform-native/src/process/mod.rs
//! |-- platform-native/src/vcs/format/mod.rs
//! `-- platform-native/src/vcs/format/table.rs
//! External:
//! |-- serde_json::json
//! |-- tempfile::tempdir
//! `-- tokio::test
//! Used by:
//! `-- none, entrypoint
//! Invariants: PATH is replaced by a temp bin dir so no real formatter runs.
//! Contract tests: platform-native/tests/format.rs

use opencode_platform_native::process::NativeProcessRunner;
use opencode_platform_native::vcs::format::table::{BUILT_INS, find_up};
use opencode_platform_native::vcs::format::{FormatContext, FormatterConfig, Formatters, extname};
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn context(root: &Path, bin: &Path) -> FormatContext {
    FormatContext {
        directory: root.to_path_buf(),
        worktree: root.to_path_buf(),
        experimental_oxfmt: false,
        path: Some(bin.to_string_lossy().into_owned()),
        bin_dir: None,
        cache_dir: Some(root.join("cache")),
        runner: Arc::new(NativeProcessRunner::new()),
    }
}

#[cfg(unix)]
fn script(bin: &Path, name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = bin.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[test]
fn table_matches_baseline_order_and_helpers() {
    let names: Vec<_> = BUILT_INS.iter().map(|b| b.name).collect();
    assert_eq!(names.len(), 26);
    assert_eq!(&names[..5], ["gofmt", "mix", "prettier", "oxfmt", "biome"]);
    assert_eq!(names.last(), Some(&"dfmt"));
    assert_eq!(extname(Path::new("/a/b.test.ts")), ".ts");
    assert_eq!(extname(Path::new("/a/.bashrc")), "");
    let tmp = tempfile::tempdir().unwrap();
    let nested = tmp.path().join("a/b");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(tmp.path().join("a/.clang-format"), "").unwrap();
    assert_eq!(find_up(".clang-format", &nested, Some(tmp.path())).len(), 1);
    assert!(find_up(".clang-format", &nested, Some(&nested)).is_empty());
}

#[tokio::test]
async fn disabled_when_config_absent_or_false() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = context(tmp.path(), tmp.path());
    assert!(Formatters::new(ctx.clone(), None).status().await.is_empty());
    let off = FormatterConfig::from_json(&serde_json::json!(false)).unwrap();
    let svc = Formatters::new(ctx, Some(&off));
    assert!(svc.status().await.is_empty());
    assert!(!svc.file(&tmp.path().join("x.rs")).await);
}

#[cfg(unix)]
#[tokio::test]
async fn detects_and_runs_enabled_formatters() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    script(&bin, "rustfmt", "echo formatted > \"$1\"");
    script(&bin, "clang-format", "exit 0");
    let ctx = context(&root, &bin);

    let svc = Formatters::new(ctx.clone(), Some(&FormatterConfig::Toggle(true)));
    let status = svc.status().await;
    let enabled: Vec<_> = status
        .iter()
        .filter(|s| s.enabled)
        .map(|s| s.name.as_str())
        .collect();
    // clang-format also needs a .clang-format file, so only rustfmt is on.
    assert_eq!(enabled, vec!["rustfmt"]);

    let file = root.join("main.rs");
    std::fs::write(&file, "fn main(){}").unwrap();
    assert!(svc.file(&file).await);
    assert_eq!(std::fs::read_to_string(&file).unwrap().trim(), "formatted");
    assert!(!svc.file(&root.join("x.unknown")).await);

    std::fs::write(root.join(".clang-format"), "").unwrap();
    let svc = Formatters::new(ctx, Some(&FormatterConfig::Toggle(true)));
    let on = svc
        .status()
        .await
        .into_iter()
        .find(|s| s.name == "clang-format")
        .unwrap();
    assert!(on.enabled);
}

#[cfg(unix)]
#[tokio::test]
async fn config_overrides_custom_commands_and_linked_ruff_uv() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let marker = root.join("marker");
    let tool = script(
        &bin,
        "custom",
        &format!("printf '%s|%s' \"$1\" \"$FOO\" > '{}'", marker.display()),
    );
    let config = FormatterConfig::from_json(&serde_json::json!({
        "uv": { "disabled": true },
        "gofmt": { "disabled": true },
        "mine": {
            "command": [tool.to_string_lossy(), "$FILE"],
            "extensions": [".foo"],
            "environment": { "FOO": "bar" }
        }
    }))
    .unwrap();
    let svc = Formatters::new(context(&root, &bin), Some(&config));
    let status = svc.status().await;
    let names: Vec<_> = status.iter().map(|s| s.name.as_str()).collect();
    assert!(!names.contains(&"ruff") && !names.contains(&"uv") && !names.contains(&"gofmt"));
    assert_eq!(names.last(), Some(&"mine"));
    let mine = status.last().unwrap();
    assert!(mine.enabled);
    assert_eq!(mine.extensions, vec![".foo"]);

    let file = root.join("a.foo");
    std::fs::write(&file, "").unwrap();
    assert!(svc.file(&file).await);
    let written = std::fs::read_to_string(&marker).unwrap();
    assert_eq!(written, format!("{}|bar", file.display()));
}

#[cfg(unix)]
#[tokio::test]
async fn npm_formatters_need_dependency_and_installed_bin() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let ctx = context(&root, &root.join("bin"));
    std::fs::write(
        root.join("package.json"),
        r#"{"devDependencies":{"prettier":"^3"}}"#,
    )
    .unwrap();
    let svc = Formatters::new(ctx.clone(), Some(&FormatterConfig::Toggle(true)));
    let prettier = |s: &[opencode_platform_native::vcs::format::FormatterStatus]| {
        s.iter().find(|s| s.name == "prettier").unwrap().enabled
    };
    // Not installed in the npm cache: disabled (installation is the runtime npm service's job).
    assert!(!prettier(&svc.status().await));
    let bin_dir = root.join("cache/packages/prettier/node_modules/.bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    script(&bin_dir, "prettier", "exit 0");
    let svc = Formatters::new(ctx, Some(&FormatterConfig::Toggle(true)));
    assert!(prettier(&svc.status().await));
}
