//! Purpose: end-to-end composition test: boot the real process root in a temp dir (real SQLite
//! file, real git, real HTTP listener), resolve a Location, create a Session, admit a prompt and
//! observe the drain, then shut down in order.
//! Scope: integration test over public crate boundaries (no fakes).
//! Uses:
//! rust/crates/
//! |-- runtime/src/lib.rs
//! `-- runtime/src/process.rs
//! External:
//! |-- opencode_server::lifecycle::ListenOptions
//! `-- serde_json::{Value, json}
//! Used by:
//! `-- none, entrypoint
//! Invariants: nothing is written outside the temp root; `git` runs with an isolated global config.
//! Contract tests: this file

use std::path::Path;
use std::time::Duration;

use opencode_runtime::{Process, ProcessOptions};
use opencode_server::lifecycle::ListenOptions;
use serde_json::{Value, json};

async fn git(dir: &Path, args: &[&str], config: &Path) {
    let status = tokio::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", config)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .status()
        .await
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

struct Fixture {
    _root: tempfile::TempDir,
    repo: std::path::PathBuf,
    options: ProcessOptions,
}

async fn fixture() -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let base = root.path().canonicalize().unwrap();
    let repo = base.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let config = base.join("gitconfig");
    std::fs::write(&config, "").unwrap();
    git(&repo, &["init", "-q"], &config).await;
    git(
        &repo,
        &["commit", "-q", "--allow-empty", "-m", "root"],
        &config,
    )
    .await;
    let mut options = ProcessOptions::isolated(&base.join("opencode"));
    options.git_env = vec![
        ("GIT_CONFIG_GLOBAL".into(), config.display().to_string()),
        ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
    ];
    Fixture {
        _root: root,
        repo,
        options,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn boots_serves_location_and_session_then_shuts_down() {
    let fixture = fixture().await;
    let process = Process::start(fixture.options.clone()).await.unwrap();
    assert!(
        fixture
            .options
            .paths
            .data
            .join("opencode-local.db")
            .exists()
    );
    let listener = process
        .serve(
            ListenOptions {
                hostname: "127.0.0.1".into(),
                port: 0,
            },
            Vec::new(),
        )
        .await
        .unwrap();
    let client = reqwest::Client::new();
    let repo = fixture.repo.display().to_string();

    let location: Value = client
        .get(format!("{}/api/location", listener.url()))
        .header("x-opencode-directory", &repo)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(location["directory"], json!(repo));
    let project = location["project"]["id"].as_str().unwrap().to_owned();
    assert_ne!(
        project, "global",
        "a git repo with a root commit has its own id"
    );
    assert_eq!(location["project"]["directory"], json!(repo));

    let response = client
        .post(format!("{}/api/session", listener.url()))
        .json(&json!({ "location": { "directory": repo } }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);
    let created: Value = response.json().await.unwrap();
    let session = created["data"]["id"].as_str().unwrap().to_owned();
    assert!(session.starts_with("ses_"));
    assert_eq!(created["data"]["projectID"], json!(project));

    let response = client
        .post(format!("{}/api/session/{session}/prompt", listener.url()))
        .json(&json!({ "prompt": { "text": "hello" } }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);
    let admitted: Value = response.json().await.unwrap();
    assert_eq!(admitted["data"]["sessionID"], json!(session));

    // This smoke test intentionally has no configured provider. An idle drain alone proves
    // lifecycle cleanup, not model execution. production.rs separately requires a completed
    // assistant response through the actual configured transport and durable projection.
    let active_url = format!("{}/api/session/active", listener.url());
    let mut idle = false;
    for _ in 0..50 {
        let response = client.get(&active_url).send().await.unwrap();
        assert_eq!(response.status().as_u16(), 200);
        let active: Value = response.json().await.unwrap();
        assert!(active.get("data").is_some(), "unexpected body {active}");
        if !active.to_string().contains(&session) {
            idle = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(idle, "drain must finish");

    listener.stop(true).await;
    process.shutdown().await;
}

#[test]
fn process_options_debug_redacts_configuration_secrets() {
    let root = tempfile::tempdir().unwrap();
    let mut options = ProcessOptions::isolated(root.path());
    options
        .environment
        .set_var("OPENAI_API_KEY", "never-print-provider-secret");
    options
        .environment
        .set_var("OPENCODE_SERVER_PASSWORD", "never-print-server-secret");
    options
        .git_env
        .push(("SECRET".into(), "never-print-git-secret".into()));
    let diagnostics = format!("{options:?}");
    assert!(diagnostics.contains("<redacted>"));
    assert!(!diagnostics.contains("never-print"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restart_reopens_the_same_database() {
    let fixture = fixture().await;
    let process = Process::start(fixture.options.clone()).await.unwrap();
    let info = opencode_session_api::operations::SessionService::create(
        process.session.as_ref(),
        opencode_session_api::operations::CreateRequest {
            id: None,
            agent: None,
            model: None,
            location: opencode_session_api::LocationRef {
                directory: opencode_schema::values::AbsolutePath::new(
                    fixture.repo.display().to_string(),
                ),
                workspace_id: None,
            },
        },
    )
    .await
    .unwrap();
    process.shutdown().await;

    let process = Process::start(fixture.options.clone()).await.unwrap();
    let again =
        opencode_session_api::operations::SessionService::get(process.session.as_ref(), &info.id)
            .await
            .unwrap();
    assert_eq!(again.id, info.id);
    process.shutdown().await;
}
