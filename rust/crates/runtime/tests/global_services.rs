//! Purpose: exercise concrete global services through the public legacy HTTP service boundary.
//! Scope: isolated SQLite process, temporary checkout and controlled loopback HTTP only.
//! Uses:
//! rust/crates/
//! |-- runtime/src/global_services.rs
//! |-- runtime/src/process.rs
//! `-- server/src/services/legacy.rs
//! External:
//! |-- serde_json::json
//! `-- tempfile::tempdir
//! Used by:
//! `-- none, entrypoint
//! Invariants: never touches user config, credentials or external network.
//! Contract tests: runtime/tests/global_services.rs
#[path = "../src/global_services/catalog_acceptance.rs"]
mod catalog_acceptance;
#[path = "../src/global_services/move_acceptance.rs"]
mod move_acceptance;
#[path = "../src/global_services/tui_acceptance.rs"]
mod tui_acceptance;
#[path = "../src/global_services/warp_acceptance.rs"]
mod warp_acceptance;

use opencode_runtime::process::{Process, ProcessOptions};
use opencode_server::middleware::legacy_routing::RouteContext;
use opencode_server::services::legacy::LegacyError;
use serde_json::{Value, json};
use std::path::Path;
async fn call(
    process: &Process,
    directory: &Path,
    operation: &str,
    params: Value,
    query: Value,
    payload: Option<Value>,
) -> Result<Value, LegacyError> {
    process
        .services()
        .legacy_global
        .unwrap()
        .handle(
            operation,
            &RouteContext {
                directory: directory.display().to_string(),
                workspace_id: None,
            },
            params.as_object().unwrap(),
            query.as_object().unwrap(),
            payload,
        )
        .await
}
#[tokio::test]
async fn config_writes_are_real_and_keep_secrets_unexpanded() {
    let root = tempfile::tempdir().unwrap();
    let checkout = root.path().join("checkout");
    std::fs::create_dir_all(&checkout).unwrap();
    let mut options = ProcessOptions::isolated(root.path());
    options
        .environment
        .vars
        .insert("FIXTURE_SECRET".into(), "fixture-only".into());
    let process = Process::start(options).await.unwrap();
    std::fs::write(
        process.options.paths.config.join("opencode.jsonc"),
        "{\n // keep this\n \"username\": \"{env:FIXTURE_SECRET}\"\n}\n",
    )
    .unwrap();
    let loaded = call(
        &process,
        &checkout,
        "global.config.get",
        json!({}),
        json!({}),
        None,
    )
    .await
    .unwrap();
    assert_eq!(loaded["username"], "fixture-only");
    let updated = call(
        &process,
        &checkout,
        "global.config.update",
        json!({}),
        json!({}),
        Some(json!({"model":"fixture/model"})),
    )
    .await
    .unwrap();
    assert_eq!(updated["model"], "fixture/model");
    let raw = std::fs::read_to_string(process.options.paths.config.join("opencode.jsonc")).unwrap();
    assert!(raw.contains("keep this"));
    assert!(raw.contains("{env:FIXTURE_SECRET}"));
    assert!(!raw.contains("fixture-only"));
    let patch = json!({"username":"project-user"});
    assert_eq!(
        call(
            &process,
            &checkout,
            "config.update",
            json!({}),
            json!({}),
            Some(patch.clone())
        )
        .await
        .unwrap(),
        patch
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&std::fs::read(checkout.join("config.json")).unwrap())
            .unwrap(),
        patch
    );
    assert_eq!(
        call(
            &process,
            &checkout,
            "global.dispose",
            json!({}),
            json!({}),
            None
        )
        .await
        .unwrap(),
        true
    );
    assert_eq!(
        call(
            &process,
            &checkout,
            "global.config.get",
            json!({}),
            json!({}),
            None
        )
        .await
        .unwrap()["theme"],
        "dark"
    );
    process.shutdown().await;
}
#[tokio::test]
async fn project_metadata_roundtrips_in_sqlite() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().canonicalize().unwrap();
    let process = Process::start(ProcessOptions::isolated(&directory))
        .await
        .unwrap();
    let current = call(
        &process,
        &directory,
        "project.current",
        json!({}),
        json!({}),
        None,
    )
    .await
    .unwrap();
    assert_eq!(current["id"], "global");
    let updated = call(
        &process,
        &directory,
        "project.update",
        json!({"projectID":"global"}),
        json!({}),
        Some(json!({"name":"fixture","icon":{"color":"red"},"commands":{"start":"run fixture"}})),
    )
    .await
    .unwrap();
    assert_eq!(updated["name"], "fixture");
    assert_eq!(updated["icon"]["color"], "red");
    assert_eq!(updated["commands"]["start"], "run fixture");
    let preserved = call(
        &process,
        &directory,
        "project.update",
        json!({"projectID":"global"}),
        json!({}),
        Some(json!({"icon":{"url":"fixture-url"}})),
    )
    .await
    .unwrap();
    assert_eq!(preserved["name"], "fixture");
    assert_eq!(preserved["icon"]["color"], "red");
    assert_eq!(
        call(
            &process,
            &directory,
            "project.list",
            json!({}),
            json!({}),
            None
        )
        .await
        .unwrap()[0]["name"],
        "fixture"
    );
    assert_eq!(
        call(
            &process,
            &directory,
            "project.directories",
            json!({"projectID":"global"}),
            json!({}),
            None
        )
        .await
        .unwrap(),
        json!([])
    );
    assert!(matches!(
        call(
            &process,
            &directory,
            "project.update",
            json!({"projectID":"missing"}),
            json!({}),
            Some(json!({"name":"x"}))
        )
        .await,
        Err(LegacyError::Response { status: 404, .. })
    ));
    process.shutdown().await;
}
#[tokio::test]
async fn file_reads_listing_and_escape_checks_use_native_files() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().canonicalize().unwrap().join("checkout");
    std::fs::create_dir_all(directory.join("z-dir")).unwrap();
    std::fs::write(directory.join("text.txt"), " \n hello fixture \n ").unwrap();
    std::fs::write(directory.join("bytes.bin"), [0, 1, 255]).unwrap();
    let process = Process::start(ProcessOptions::isolated(root.path()))
        .await
        .unwrap();
    let read = |path: &str| json!({"path":path});
    assert_eq!(
        call(
            &process,
            &directory,
            "file.read",
            json!({}),
            read("text.txt"),
            None
        )
        .await
        .unwrap(),
        json!({"type":"text","content":"hello fixture"})
    );
    let binary = call(
        &process,
        &directory,
        "file.read",
        json!({}),
        read("bytes.bin"),
        None,
    )
    .await
    .unwrap();
    assert_eq!(binary["content"], "AAH/");
    assert_eq!(binary["encoding"], "base64");
    assert_eq!(
        call(
            &process,
            &directory,
            "file.read",
            json!({}),
            read("missing.txt"),
            None
        )
        .await
        .unwrap(),
        json!({"type":"text","content":""})
    );
    assert!(
        call(
            &process,
            &directory,
            "file.read",
            json!({}),
            read("../secret"),
            None
        )
        .await
        .is_err()
    );
    let listed = call(
        &process,
        &directory,
        "file.list",
        json!({}),
        read("."),
        None,
    )
    .await
    .unwrap();
    assert_eq!(listed[0]["path"], "z-dir/");
    assert_eq!(listed[0]["type"], "directory");
    assert_eq!(
        call(
            &process,
            &directory,
            "file.status",
            json!({}),
            json!({}),
            None
        )
        .await
        .unwrap(),
        json!([])
    );
    assert_eq!(
        call(
            &process,
            &directory,
            "find.symbols",
            json!({}),
            json!({"query":"anything"}),
            None
        )
        .await
        .unwrap(),
        json!([])
    );
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(root.path(), directory.join("outside")).unwrap();
        assert!(
            call(
                &process,
                &directory,
                "file.read",
                json!({}),
                read("outside/secret"),
                None
            )
            .await
            .is_err()
        );
    }
    process.shutdown().await;
}
#[tokio::test]
async fn git_status_diff_and_apply_are_real() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().canonicalize().unwrap().join("checkout");
    std::fs::create_dir_all(&directory).unwrap();
    let process = Process::start(ProcessOptions::isolated(root.path()))
        .await
        .unwrap();
    assert!(matches!(
        call(
            &process,
            &directory,
            "vcs.apply",
            json!({}),
            json!({}),
            Some(json!({"patch":""}))
        )
        .await,
        Err(LegacyError::Response { status: 400, .. })
    ));
    call(
        &process,
        &directory,
        "project.initGit",
        json!({}),
        json!({}),
        None,
    )
    .await
    .unwrap();
    let git = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .current_dir(&directory)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", root.path().join("empty"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    std::fs::write(directory.join("tracked.txt"), "before\n").unwrap();
    git(&["add", "tracked.txt"]);
    git(&[
        "-c",
        "user.name=Fixture",
        "-c",
        "user.email=fixture@example.invalid",
        "commit",
        "-qm",
        "fixture",
    ]);
    std::fs::write(directory.join("tracked.txt"), "after\n").unwrap();
    std::fs::write(directory.join("new.txt"), "new\n").unwrap();
    let status = call(
        &process,
        &directory,
        "vcs.status",
        json!({}),
        json!({}),
        None,
    )
    .await
    .unwrap();
    assert_eq!(status.as_array().unwrap().len(), 2);
    assert_eq!(
        status[0],
        json!({"file":"new.txt","additions":1,"deletions":0,"status":"added"})
    );
    assert_eq!(
        status[1],
        json!({"file":"tracked.txt","additions":1,"deletions":1,"status":"modified"})
    );
    let diff = call(
        &process,
        &directory,
        "vcs.diff",
        json!({}),
        json!({"mode":"git","context":0}),
        None,
    )
    .await
    .unwrap();
    assert!(diff[1]["patch"].as_str().unwrap().contains("+after"));
    let raw = process
        .services()
        .legacy_global
        .unwrap()
        .handle_response(
            "vcs.diff.raw",
            &RouteContext {
                directory: directory.display().to_string(),
                workspace_id: None,
            },
            json!({}).as_object().unwrap(),
            json!({}).as_object().unwrap(),
            None,
        )
        .await
        .unwrap();
    assert_eq!(raw.headers[0].0, "content-type");
    assert!(raw.body.as_str().unwrap().contains("+after"));
    git(&["checkout", "--", "tracked.txt"]);
    let patch = "diff --git a/tracked.txt b/tracked.txt\n--- a/tracked.txt\n+++ b/tracked.txt\n@@ -1 +1 @@\n-before\n+applied\n";
    assert_eq!(
        call(
            &process,
            &directory,
            "vcs.apply",
            json!({}),
            json!({}),
            Some(json!({"patch":patch}))
        )
        .await
        .unwrap(),
        json!({"applied":true})
    );
    assert_eq!(
        std::fs::read_to_string(directory.join("tracked.txt")).unwrap(),
        "applied\n"
    );
    assert!(
        call(
            &process,
            &directory,
            "vcs.apply",
            json!({}),
            json!({}),
            Some(json!({"patch":patch}))
        )
        .await
        .is_err()
    );
    process.shutdown().await;
}
#[path = "../src/global_services/remote_acceptance.rs"]
mod remote_acceptance;
