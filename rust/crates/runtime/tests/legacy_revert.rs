//! Purpose: production legacy revert acceptance using real Git snapshots and native write tools.
//! Scope: controlled HTTP provider, persisted V1 snapshot parts and public revert boundaries.
//! Invariants: invalid boundaries do not mutate files; unrevert restores the pre-revert workspace.
//! Contract tests: this integration target.

#[path = "support/legacy_host.rs"]
mod legacy_host;
#[allow(dead_code)]
#[path = "support/provider.rs"]
mod provider;

use legacy_host::Host;
use provider::{Provider, Reply};
use serde_json::json;
use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_legacy_revert_restores_git_files_and_unrevert_restores_modified_state() {
    let mut provider = Provider::start(vec![
        Reply::Tool {name:"write", arguments:json!({"filePath":"evidence.txt","content":"changed by actual native tool\n"})},
        Reply::Text("file changed"),
    ]).await;
    let host = Host::start(Some(&provider.url)).await;
    for args in [
        vec!["init", "-q"],
        vec!["add", "evidence.txt"],
        vec!["commit", "-q", "-m", "initial evidence"],
    ] {
        let output = std::process::Command::new("git")
            .args([
                "-c",
                "user.name=fixture",
                "-c",
                "user.email=fixture@example.invalid",
            ])
            .args(args)
            .current_dir(&host.directory)
            .env("HOME", host.root.path())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let session = host.create("real revert").await;
    let id = session["id"].as_str().unwrap();
    tokio::time::timeout(
        Duration::from_secs(20),
        host.call(
            "session.prompt",
            Some(id),
            json!({}),
            Some(json!({"parts":[{"type":"text","text":"Write evidence.txt"}]})),
        ),
    )
    .await
    .unwrap();
    let _ = provider.requests.recv().await.unwrap();
    let _ = provider.requests.recv().await.unwrap();
    assert_eq!(
        std::fs::read_to_string(host.directory.join("evidence.txt")).unwrap(),
        "changed by actual native tool\n"
    );
    let invalid = host
        .process
        .legacy_command(
            "session.revert",
            host.directory.clone(),
            serde_json::from_value(json!({"sessionID":id})).unwrap(),
            Default::default(),
            Some(json!({"messageID":"msg_missing"})),
        )
        .await;
    let invalid = invalid.unwrap();
    assert!(invalid.get("revert").is_none());
    assert_eq!(
        std::fs::read_to_string(host.directory.join("evidence.txt")).unwrap(),
        "changed by actual native tool\n"
    );
    let messages = host.messages(id).await;
    let boundary = messages
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["info"]["role"] == "user")
        .unwrap()["info"]["id"]
        .as_str()
        .unwrap();
    let reverted = host
        .call(
            "session.revert",
            Some(id),
            json!({}),
            Some(json!({"messageID":boundary})),
        )
        .await;
    assert_eq!(reverted["revert"]["messageID"], boundary);
    assert_eq!(
        std::fs::read_to_string(host.directory.join("evidence.txt")).unwrap(),
        "native legacy read evidence\n"
    );
    let restored = host
        .call("session.unrevert", Some(id), json!({}), None)
        .await;
    assert!(restored.get("revert").is_none());
    assert_eq!(
        std::fs::read_to_string(host.directory.join("evidence.txt")).unwrap(),
        "changed by actual native tool\n"
    );
    host.stop().await;
    provider.stop().await;
}
