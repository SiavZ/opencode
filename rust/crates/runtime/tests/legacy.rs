//! Real production HTTP coverage for persisted legacy sessions and placed event streams.
use std::time::Duration;

use opencode_runtime::{ListenOptions, Process, ProcessOptions};
use serde_json::{Value, json};

async fn event(response: &mut reqwest::Response, buffer: &mut Vec<u8>) -> Value {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(end) = buffer.windows(2).position(|bytes| bytes == b"\n\n") {
                let frame = String::from_utf8(buffer.drain(..end + 2).collect()).unwrap();
                if let Some(data) = frame.lines().find_map(|line| line.strip_prefix("data: ")) {
                    return serde_json::from_str(data).unwrap();
                }
            }
            buffer.extend(response.chunk().await.unwrap().expect("open event stream"));
        }
    })
    .await
    .expect("real event arrives")
}

#[tokio::test]
async fn legacy_http_persists_projects_sessions_and_publishes_real_placed_events() {
    let root = tempfile::tempdir().unwrap();
    let base = root.path().canonicalize().unwrap();
    let directory = base.join("project");
    std::fs::create_dir_all(&directory).unwrap();
    let mut options = ProcessOptions::isolated(&base.join("state"));
    options.version = "legacy-test".into();
    options.git_env = vec![
        ("HOME".into(), base.display().to_string()),
        (
            "GIT_CONFIG_GLOBAL".into(),
            base.join("absent-git-config").display().to_string(),
        ),
        ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
    ];
    for arguments in [
        vec!["init", "--quiet"],
        vec![
            "-c",
            "user.name=Loopback Test",
            "-c",
            "user.email=loopback@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--allow-empty",
            "--quiet",
            "-m",
            "initial",
        ],
    ] {
        assert!(
            std::process::Command::new("git")
                .args(arguments)
                .current_dir(&directory)
                .envs(options.git_env.clone())
                .status()
                .unwrap()
                .success()
        );
    }
    let process = Process::start(options.clone()).await.unwrap();
    let listener = process
        .serve(
            ListenOptions {
                hostname: "127.0.0.1".into(),
                port: 0,
            },
            vec![],
        )
        .await
        .unwrap();
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let mut global = client
        .get(format!("{}/global/event", listener.url()))
        .send()
        .await
        .unwrap();
    let mut scoped = client
        .get(format!("{}/event", listener.url()))
        .header("x-opencode-directory", directory.to_str().unwrap())
        .send()
        .await
        .unwrap();
    let mut global_buffer = vec![];
    let mut scoped_buffer = vec![];
    assert_eq!(
        event(&mut global, &mut global_buffer).await["payload"]["type"],
        "server.connected"
    );
    assert_eq!(
        event(&mut scoped, &mut scoped_buffer).await["type"],
        "server.connected"
    );
    let response = client.post(format!("{}/session", listener.url())).header("x-opencode-directory",directory.to_str().unwrap()).json(&json!({"title":"Persisted MixedCase", "metadata":{"evidence":"real"}, "model":{"providerID":"local","id":"test"}})).send().await.unwrap();
    assert_eq!(response.status(), 200);
    let created: Value = response.json().await.unwrap();
    assert_eq!(created["version"], "legacy-test");
    assert_eq!(created["cost"], json!(0));
    assert_ne!(
        created["projectID"], "global",
        "real Git identity is persisted before the session foreign key"
    );
    assert_eq!(created["directory"], directory.to_str().unwrap());
    let id = created["id"].as_str().unwrap();
    let published = event(&mut global, &mut global_buffer).await;
    assert_eq!(published["directory"], directory.to_str().unwrap());
    assert_eq!(published["payload"]["type"], "session.created");
    assert_eq!(published["payload"]["properties"]["info"], created);
    let published = event(&mut scoped, &mut scoped_buffer).await;
    assert_eq!(published["type"], "session.created");
    assert_eq!(published["properties"]["sessionID"], id);
    let listed: Value = client
        .get(format!("{}/session?search=mixedcase", listener.url()))
        .header("x-opencode-directory", directory.to_str().unwrap())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(listed, json!([created.clone()]));
    let fetched: Value = client
        .get(format!("{}/session/{id}", listener.url()))
        .header("x-opencode-directory", directory.to_str().unwrap())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(fetched, created);
    drop(global);
    drop(scoped);
    listener.stop(true).await;
    process.shutdown().await;
    let process = Process::start(options).await.unwrap();
    let listener = process
        .serve(
            ListenOptions {
                hostname: "127.0.0.1".into(),
                port: 0,
            },
            vec![],
        )
        .await
        .unwrap();
    let fetched: Value = client
        .get(format!("{}/session/{id}", listener.url()))
        .header("x-opencode-directory", directory.to_str().unwrap())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        fetched, created,
        "SQLite projection survives full process restart"
    );
    listener.stop(true).await;
    process.shutdown().await;
}

#[tokio::test]
async fn legacy_production_authentication_errors_and_empty_history_are_real() {
    let root = tempfile::tempdir().unwrap();
    let base = root.path().canonicalize().unwrap();
    let directory = base.join("project");
    std::fs::create_dir_all(&directory).unwrap();
    let mut options = ProcessOptions::isolated(&base.join("state"));
    options
        .environment
        .vars
        .insert("OPENCODE_SERVER_PASSWORD".into(), "secret".into());
    let process = Process::start(options).await.unwrap();
    let listener = process
        .serve(
            ListenOptions {
                hostname: "127.0.0.1".into(),
                port: 0,
            },
            vec!["https://example.test".into()],
        )
        .await
        .unwrap();
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    for path in ["/global/health", "/global/event", "/event", "/session"] {
        assert_eq!(
            client
                .get(format!("{}{path}", listener.url()))
                .send()
                .await
                .unwrap()
                .status(),
            401
        );
    }
    let health = client
        .get(format!("{}/global/health", listener.url()))
        .basic_auth("opencode", Some("secret"))
        .header("Origin", "https://example.test")
        .send()
        .await
        .unwrap();
    assert_eq!(health.status(), 200);
    assert_eq!(
        health.headers()["access-control-allow-origin"],
        "https://example.test"
    );
    assert_eq!(health.json::<Value>().await.unwrap()["healthy"], true);
    let malformed = client
        .post(format!("{}/session", listener.url()))
        .basic_auth("opencode", Some("secret"))
        .header("x-opencode-directory", directory.to_str().unwrap())
        .body("{")
        .send()
        .await
        .unwrap();
    assert_eq!(malformed.status(), 400);
    let created: Value = client
        .post(format!("{}/session", listener.url()))
        .basic_auth("opencode", Some("secret"))
        .header("x-opencode-directory", directory.to_str().unwrap())
        .json(&json!({}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = created["id"].as_str().unwrap();
    let history: Value = client
        .get(format!("{}/session/{id}/message", listener.url()))
        .basic_auth("opencode", Some("secret"))
        .header("x-opencode-directory", directory.to_str().unwrap())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(history, json!([]));
    let missing = client
        .get(format!("{}/session/ses_missing", listener.url()))
        .basic_auth("opencode", Some("secret"))
        .header("x-opencode-directory", directory.to_str().unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), 404);
    assert_eq!(
        missing.json::<Value>().await.unwrap()["name"],
        "NotFoundError"
    );
    listener.stop(true).await;
    process.shutdown().await;
}
