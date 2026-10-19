//! Purpose: production-adapter smoke tests: the core and legacy edit/write tools over the real
//! `NativeFileSystem`, `NativeFileMutation` and `NativeLocationMutation` on temp dirs.
//! Scope: live native adapters (no dev fixtures) through `Tool::execute`.
//! Uses:
//! rust/crates/
//! |-- platform-native/src/fs/mod.rs
//! |-- tool-api/src/lib.rs
//! |-- tools/src/files/edit.rs
//! |-- tools/src/files/edit_legacy.rs
//! |-- tools/src/files/mod.rs
//! |-- tools/src/files/write.rs
//! `-- tools/tests/files_mutation/support.rs
//! External:
//! |-- serde_json::json
//! |-- tempfile::TempDir
//! |-- tempfile::tempdir
//! `-- tokio::test
//! Used by:
//! rust/crates/
//! `-- tools/tests/files_write.rs
//! Invariants: BOM/CRLF preservation, overwrite detection and missing-ancestor creation hold on
//! the production adapters too.
//! Contract tests: this suite

use super::support::{Recorder, context};
use opencode_platform_native::fs::{NativeFileMutation, NativeFileSystem, NativeLocationMutation};
use opencode_tool_api::Tool;
use opencode_tools::files::FileServices;
use opencode_tools::files::edit::CoreEditTool;
use opencode_tools::files::edit_legacy::{LegacyEditTool, NoEditHooks};
use opencode_tools::files::write::{CoreWriteTool, LegacyWriteTool};
use serde_json::json;
use std::sync::Arc;

async fn native() -> (tempfile::TempDir, std::path::PathBuf, FileServices) {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    let services = FileServices::new(
        Arc::new(NativeFileSystem::new()),
        Arc::new(NativeFileMutation::new()),
        Arc::new(NativeLocationMutation::new(&root).await.unwrap()),
        root.clone(),
        root.clone(),
    );
    (temp, root, services)
}

#[tokio::test]
async fn core_tools_on_native_adapters() {
    let (_temp, root, services) = native().await;
    let write = CoreWriteTool::new(services.clone());
    let edit = CoreEditTool::new(services.clone());
    let ctx = context(Arc::new(Recorder::default()));
    let created = write
        .execute(
            json!({"path": "a/b/new.txt", "content": "\u{FEFF}x\r\ny\r\n"}),
            &ctx,
        )
        .await
        .unwrap();
    assert_eq!(created.structured["existed"], false);
    let edited = edit
        .execute(
            json!({"path": "a/b/new.txt", "oldString": "x\ny", "newString": "z\ny"}),
            &ctx,
        )
        .await
        .unwrap();
    assert_eq!(edited.structured["replacements"], 1);
    let bytes = std::fs::read(root.join("a/b/new.txt")).unwrap();
    assert_eq!(bytes, "\u{FEFF}z\r\ny\r\n".as_bytes());
    let rewritten = write
        .execute(json!({"path": "a/b/new.txt", "content": "plain"}), &ctx)
        .await
        .unwrap();
    assert_eq!(rewritten.structured["existed"], true);
    assert_eq!(
        std::fs::read(root.join("a/b/new.txt")).unwrap(),
        "\u{FEFF}plain".as_bytes()
    );
}

#[tokio::test]
async fn legacy_tools_on_native_adapters() {
    let (_temp, root, services) = native().await;
    let hooks = Arc::new(NoEditHooks);
    let write = LegacyWriteTool::new(services.clone(), hooks.clone());
    let edit = LegacyEditTool::new(services, hooks);
    let ctx = context(Arc::new(Recorder::default()));
    let file = root.join("deep/x.ts").to_string_lossy().into_owned();
    write
        .execute(json!({"filePath": file, "content": "  foo()\n"}), &ctx)
        .await
        .unwrap();
    edit.execute(
        json!({"filePath": file, "oldString": "foo()", "newString": "bar()"}),
        &ctx,
    )
    .await
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(root.join("deep/x.ts")).unwrap(),
        "  bar()\n"
    );
}
