//! Purpose: verify canonical current catalogs and byte-preserving native filesystem reads.
//! Scope: public production service adapter with isolated SQLite and checkout.
//! Uses:
//! rust/crates/
//! `-- runtime/tests/global_services.rs
//! External:
//! `-- serde_json::json
//! Used by:
//! rust/crates/
//! `-- runtime/tests/global_services.rs
//! Invariants: canonical schemas do not inherit legacy hints, sources or base64 wrappers.
//! Contract tests: runtime/tests/global_services.rs

use super::{Process, ProcessOptions, RouteContext, call, json};

#[tokio::test]
async fn current_catalogs_and_raw_files_keep_canonical_values() {
    let root = tempfile::tempdir().unwrap();
    let checkout = root.path().canonicalize().unwrap().join("checkout");
    std::fs::create_dir_all(checkout.join("folder")).unwrap();
    std::fs::write(checkout.join("bytes.bin"), [0, 255, 128, 10]).unwrap();
    std::fs::write(checkout.join("plain.txt"), b" \xef\xbb\xbfuntouched \n").unwrap();
    std::fs::write(checkout.join("opencode.json"), serde_json::to_vec(&json!({
        "commands":{"review":{"template":"configured","model":"fixture/deep/model","variant":"high"},"fixture":{"template":"fixture body"}}
    })).unwrap()).unwrap();
    let process = Process::start(ProcessOptions::isolated(root.path()))
        .await
        .unwrap();
    let agents = call(
        &process,
        &checkout,
        "v2.agent.list",
        json!({}),
        json!({}),
        None,
    )
    .await
    .unwrap();
    assert!(
        agents
            .as_array()
            .unwrap()
            .iter()
            .any(|agent| agent["id"] == "build")
    );
    assert!(
        agents
            .as_array()
            .unwrap()
            .iter()
            .all(|agent| agent.get("name").is_none())
    );
    let commands = call(
        &process,
        &checkout,
        "v2.command.list",
        json!({}),
        json!({}),
        None,
    )
    .await
    .unwrap();
    let commands = commands.as_array().unwrap();
    assert_eq!(commands[0]["name"], "init");
    assert_eq!(commands[1]["name"], "review");
    assert_eq!(commands[1]["template"], "configured");
    assert_eq!(
        commands[1]["model"],
        json!({"id":"deep/model","providerID":"fixture","variant":"high"})
    );
    assert_eq!(commands[1]["subtask"], true);
    assert!(
        commands
            .iter()
            .all(|command| command.get("hints").is_none() && command.get("source").is_none())
    );
    let entries = call(
        &process,
        &checkout,
        "v2.fs.list",
        json!({}),
        json!({}),
        None,
    )
    .await
    .unwrap();
    assert_eq!(entries[0], json!({"path":"folder/","type":"directory"}));
    assert!(
        entries
            .as_array()
            .unwrap()
            .iter()
            .all(|entry| entry.as_object().unwrap().len() == 2)
    );
    let service = process.services().legacy_global.unwrap();
    let context = RouteContext {
        directory: checkout.display().to_string(),
        workspace_id: None,
    };
    for (path, bytes, mime) in [
        (
            "bytes.bin",
            vec![0, 255, 128, 10],
            "application/octet-stream",
        ),
        (
            "plain.txt",
            b" \xef\xbb\xbfuntouched \n".to_vec(),
            "text/plain",
        ),
    ] {
        let response = service
            .handle_bytes(
                "v2.fs.read",
                &context,
                json!({"wildcard":path}).as_object().unwrap(),
                &serde_json::Map::new(),
                None,
            )
            .await
            .unwrap();
        assert_eq!(response.body, bytes);
        assert_eq!(response.status, 200);
        assert!(
            response
                .headers
                .iter()
                .any(|(key, value)| key == "content-type" && value == mime)
        );
    }
    assert!(
        service
            .handle_bytes(
                "v2.fs.read",
                &context,
                json!({"wildcard":"../escape"}).as_object().unwrap(),
                &serde_json::Map::new(),
                None
            )
            .await
            .is_err()
    );
    assert!(
        service
            .handle_bytes(
                "v2.fs.read",
                &context,
                json!({"wildcard":"folder"}).as_object().unwrap(),
                &serde_json::Map::new(),
                None
            )
            .await
            .is_err()
    );
    assert!(
        service
            .handle_bytes(
                "v2.fs.read",
                &context,
                json!({"wildcard":"absent"}).as_object().unwrap(),
                &serde_json::Map::new(),
                None
            )
            .await
            .is_err()
    );
    process.shutdown().await;
}
