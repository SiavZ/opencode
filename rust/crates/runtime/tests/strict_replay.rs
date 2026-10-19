//! Purpose: replay every captured HTTP requirement against the production Process composition.
//! Scope: explicitly enabled offline differential, real SQLite/git/domain services and full host.
//! Uses:
//! rust/crates/
//! |-- runtime/src/lib.rs
//! |-- runtime/tests/strict_replay/pagination.rs
//! |-- runtime/tests/strict_replay/requests.rs
//! |-- server/src/app.rs
//! |-- server/src/middleware/auth.rs
//! |-- server/src/middleware/cors.rs
//! |-- server/tests/support/replay/matcher.rs
//! |-- server/tests/support/replay/mod.rs
//! `-- server/tests/support/replay/run.rs
//! External:
//! |-- app::router
//! |-- axum::Router
//! |-- axum::body::Body::empty
//! |-- axum::body::Body::from
//! |-- axum::http::Request::builder
//! |-- http_body_util::BodyExt
//! |-- serde_json::Value
//! |-- serde_json::from_slice
//! |-- serde_json::json
//! |-- serde_json::to_string_pretty
//! |-- tempfile::TempDir
//! |-- tempfile::tempdir
//! |-- tokio::net::TcpSocket
//! |-- tokio::net::TcpSocket::new_v4
//! |-- tokio::test
//! |-- tower::ServiceExt
//! |-- tracing::Level::ERROR
//! `-- tracing_subscriber::fmt
//! Used by:
//! rust/crates/
//! `-- runtime/tests/strict_replay/pagination.rs
//! Invariants: no expected-failure ledger, no fixture mutation, isolated state and loopback provider only.
//! Contract tests: this file, explicitly invoked by xtask fixtures run

#[path = "../../server/tests/support/replay/mod.rs"]
mod replay;

#[path = "strict_replay/pagination.rs"]
mod pagination;
#[path = "strict_replay/requests.rs"]
mod requests;

use std::collections::BTreeMap;

use http_body_util::BodyExt;
use opencode_runtime::{Process, ProcessOptions};
use opencode_server::app::{self, ServerConfig};
use opencode_server::middleware::auth::AuthConfig;
use opencode_server::middleware::cors::CorsConfig;
use replay::matcher::Bindings;
use serde_json::json;
use tower::ServiceExt;

struct Host {
    process: Process,
    router: axum::Router,
    literals: Vec<(String, String)>,
    project_id: String,
    _root: tempfile::TempDir,
    // Bind without listening: the kernel refuses TCP connections, exactly like the capture's
    // dead port, while preventing an unrelated process from claiming the provider destination.
    _provider: tokio::net::TcpSocket,
}

impl Host {
    async fn start(password: bool) -> Self {
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::ERROR)
            .with_test_writer()
            .try_init();
        let root = tempfile::tempdir().unwrap();
        let base = root.path().canonicalize().unwrap();
        let project = base.join("proj");
        std::fs::create_dir_all(project.join("src")).unwrap();
        std::fs::write(base.join("gitconfig"), "").unwrap();
        std::fs::write(
            project.join("README.md"),
            "# fixture project\n\nhello fixture\n",
        )
        .unwrap();
        std::fs::write(
            project.join("src/main.ts"),
            "export function hello() {\n  return 'fixture'\n}\n",
        )
        .unwrap();
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec!["add", "README.md", "src/main.ts"],
            vec!["commit", "-q", "-m", "init"],
        ] {
            let output = std::process::Command::new("git")
                .args([
                    "-c",
                    "user.name=fixture",
                    "-c",
                    "user.email=fixture@example.com",
                ])
                .args(args)
                .current_dir(&project)
                .env("HOME", base.join("home"))
                .env("GIT_CONFIG_GLOBAL", base.join("gitconfig"))
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "isolated git setup: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let commit = std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&project)
            .output()
            .unwrap();
        assert!(commit.status.success());
        let project_id = String::from_utf8(commit.stdout).unwrap().trim().to_owned();
        std::fs::write(
            project.join("README.md"),
            "# fixture project\n\nhello fixture\nmodified\n",
        )
        .unwrap();
        let provider = tokio::net::TcpSocket::new_v4().unwrap();
        provider
            .bind(std::net::SocketAddr::from(([127, 0, 0, 1], 0)))
            .unwrap();
        let endpoint = format!("http://127.0.0.1:{}", provider.local_addr().unwrap().port());
        let mut options = ProcessOptions::isolated(&base);
        // capture.py sets TMPDIR to XDG_CACHE_HOME, not the isolated helper's default tmp.
        options.paths.tmp = base.join("cache/opencode");
        for (key, path) in [
            ("XDG_DATA_HOME", "data"),
            ("XDG_CONFIG_HOME", "config"),
            ("XDG_STATE_HOME", "state"),
            ("XDG_CACHE_HOME", "cache"),
            ("TMPDIR", "cache"),
        ] {
            options
                .environment
                .set_var(key, base.join(path).display().to_string());
        }
        options.environment.username = Some("unknown".into());
        options.environment.set_var(
            "OPENCODE_TEST_MANAGED_CONFIG_DIR",
            base.join("managed").display().to_string(),
        );
        if let Some(path) = std::env::var_os("PATH") {
            options
                .environment
                .set_var("PATH", path.to_string_lossy().into_owned());
        }
        options.git_env = vec![
            (
                "GIT_CONFIG_GLOBAL".into(),
                base.join("gitconfig").display().to_string(),
            ),
            ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
        ];
        for (key, value) in [
            ("OPENCODE_DISABLE_MODELS_FETCH", "1"),
            ("OPENCODE_DISABLE_AUTOUPDATE", "1"),
            ("OPENCODE_DISABLE_LSP_DOWNLOAD", "1"),
            ("HTTP_PROXY", endpoint.as_str()),
            ("HTTPS_PROXY", endpoint.as_str()),
            ("http_proxy", endpoint.as_str()),
            ("https_proxy", endpoint.as_str()),
            ("NO_PROXY", "127.0.0.1,localhost"),
            ("no_proxy", "127.0.0.1,localhost"),
            ("SHELL", "/bin/sh"),
            ("TERM", "dumb"),
        ] {
            options.environment.set_var(key, value);
        }
        options.environment.set_var(
            "OPENCODE_CONFIG_CONTENT",
            json!({
                "share":"disabled", "autoupdate":false, "model":"fixture/fixture-model",
                "provider":{"fixture":{
                    "name":"Fixture", "npm":"@ai-sdk/openai-compatible",
                    "options":{"baseURL":format!("{endpoint}/v1"), "apiKey":"fixture-placeholder"},
                    "models":{"fixture-model":{"name":"Fixture Model"}}
                }}
            })
            .to_string(),
        );
        let process = Process::start(options).await.unwrap();
        let router = app::router(
            process.services(),
            ServerConfig::full_host(
                AuthConfig {
                    username: "opencode".into(),
                    password: password.then(|| replay::run::PASSWORD.into()),
                },
                CorsConfig::default(),
            ),
        );
        Self {
            process,
            router,
            project_id,
            literals: vec![
                ("{PROJ}".into(), project.display().to_string()),
                ("{TMP}".into(), base.display().to_string()),
                (
                    "{CWD}".into(),
                    std::env::current_dir().unwrap().display().to_string(),
                ),
                (
                    "{DEAD_PORT}".into(),
                    provider.local_addr().unwrap().port().to_string(),
                ),
            ],
            _root: root,
            _provider: provider,
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn replay_host_persists_real_session_across_process_restart() {
    let host = Host::start(false).await;
    let project = &host
        .literals
        .iter()
        .find(|(token, _)| token == "{PROJ}")
        .unwrap()
        .1;
    let response = host
        .router
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/api/session")
                .header("content-type", "application/json")
                .body(axum::body::Body::from(
                    json!({"location":{"directory":project}}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let created: serde_json::Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let session = created["data"]["id"]
        .as_str()
        .expect("production session ID");
    assert!(session.starts_with("ses_"));
    let options = host.process.options.clone();
    host.process.shutdown().await;
    let process = Process::start(options).await.unwrap();
    let router = app::router(
        process.services(),
        ServerConfig::full_host(AuthConfig::default(), CorsConfig::default()),
    );
    let response = router
        .oneshot(
            axum::http::Request::builder()
                .uri(format!("/api/session/{session}"))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let restored: serde_json::Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    process.shutdown().await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(restored["data"]["id"], session);
    assert_eq!(restored["data"]["location"]["directory"], project.as_str());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "explicit offline product differential; xtask strict gate always executes this test"]
async fn production_process_matches_every_recorded_requirement() {
    let cases = replay::load();
    assert!(
        !cases.is_empty(),
        "strict replay cannot approve an empty corpus"
    );
    let nopass = Host::start(false).await;
    let pass = Host::start(true).await;
    let mut bindings = Bindings::default();
    let mut categories = BTreeMap::<String, usize>::new();
    let mut results = Vec::new();
    let started = std::time::Instant::now();
    for case in &cases {
        let host = match case.server.as_str() {
            "nopass" => &nopass,
            "pass" => &pass,
            other => panic!("{}: unknown server variant {other}", case.id),
        };
        bindings.literals.clone_from(&host.literals);
        let execution = requests::bind(case, &bindings, &host.project_id);
        // Keep cascading setup failures visible instead of misdiagnosing missing fixture IDs
        // as independent production regressions. The request is still executed and must pass.
        let request = bindings.fill_value(&json!({
            "path":execution.request.path, "query":execution.request.query,
            "body":execution.request.body, "raw_body":execution.request.raw_body,
        }));
        let unbound = request.to_string().contains("_unbound");
        let elapsed = started.elapsed().as_millis();
        let outcome = replay::run::replay(&host.router, &execution, &mut bindings).await;
        let category = outcome.category.as_deref().unwrap_or("match");
        *categories.entry(category.to_owned()).or_default() += 1;
        let detail = host
            .literals
            .iter()
            .fold(outcome.detail, |text, (token, value)| {
                text.replace(value, token)
            });
        if outcome.category.is_some() {
            println!(
                "STRICT {} {} {} [{category}] dependency_unbound={unbound}: {detail}",
                case.id, case.request.method, case.request.path
            );
        }
        results.push(json!({
            "id":case.id, "seq":case.seq, "family":case.family, "operationId":case.operation_id,
            "method":case.request.method, "path":case.request.path, "category":category,
            "status":outcome.status, "dependency_unbound":unbound, "detail":detail,
            "resolved_request":request, "elapsed_ms":elapsed
        }));
    }
    let passed = *categories.get("match").unwrap_or(&0);
    let report = json!({"host":"production Process::start / Process::services", "strict":true,
        "fixture_provenance":"rust/contracts/phase-06/capture/capture.py", "original_ledger_used":false,
        "os":std::env::consts::OS, "architecture":std::env::consts::ARCH,
        "cases":cases.len(), "passed":passed,
        "failed":cases.len()-passed, "categories":categories, "results":results});
    if let Some(path) = std::env::var_os("OPENCODE_PARITY_REPORT") {
        std::fs::write(path, serde_json::to_string_pretty(&report).unwrap() + "\n").unwrap();
    }
    println!(
        "strict production replay: {} cases, {passed} passed, {} failed, {categories:?}",
        cases.len(),
        cases.len() - passed
    );
    // Shut down all production-owned tasks even when a wire mismatch will fail the assertion.
    nopass.process.shutdown().await;
    pass.process.shutdown().await;
    assert_eq!(
        passed,
        cases.len(),
        "strict production replay has incompatible requirements; no expected-failure exceptions: {categories:?}"
    );
}
