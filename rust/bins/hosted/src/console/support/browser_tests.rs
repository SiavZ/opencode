//! Purpose: accept the unchanged support lookup form through built Node SSR and Chromium.
//! Scope: opt-in local browser, native bearer API and disposable migrated MySQL.
//! Uses:
//! rust/bins/
//! |-- hosted/src/console/state.rs
//! `-- hosted/src/console/support.rs
//! rust/crates/
//! `-- hosted-adapters/src/console/db.rs
//! External:
//! |-- mysql_async::prelude::Queryable
//! |-- reqwest::Client
//! `-- tokio::net::TcpListener
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/support.rs
//! Invariants: all listeners bind literal loopback; support bearer exists only in server environment.
//! Contract tests: fixture_support_built_ssr_lookup_popup_and_operator_boundary

use super::*;
use mysql_async::prelude::Queryable;
use opencode_hosted_adapters::console::db::TestDatabase;
use serde_json::{Value, json};
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

const CANARY: &str = "support-browser-fixture-canary-NEVER-IN-CLIENT";
const EMAIL: &str = "support-browser@example.test";
const WORKSPACE: &str = "wrk_support_browser";
const NAME: &str = "Native support browser fixture";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires built rust/scripts/console-support artifacts, local MySQL and installed Chromium; run explicitly"]
async fn fixture_support_built_ssr_lookup_popup_and_operator_boundary() {
    let fixture = TestDatabase::create("support_browser").await.unwrap();
    let mut conn = fixture.db.conn().await.unwrap();
    conn.query_drop("INSERT INTO account(id) VALUES ('acc_support_browser')")
        .await
        .unwrap();
    conn.exec_drop(
        "INSERT INTO workspace(id,name) VALUES (?,?)",
        (WORKSPACE, NAME),
    )
    .await
    .unwrap();
    conn.exec_drop("INSERT INTO auth(id,provider,subject,account_id) VALUES ('aut_support_browser','email',?,'acc_support_browser')", (EMAIL,)).await.unwrap();
    conn.exec_drop("INSERT INTO `user`(id,workspace_id,account_id,name,role,email) VALUES ('usr_support_browser',?,'acc_support_browser','Support browser fixture','admin',?)", (WORKSPACE, EMAIL)).await.unwrap();
    conn.exec_drop(
        "INSERT INTO billing(id,workspace_id,balance) VALUES ('bil_support_browser',?,1230000000)",
        (WORKSPACE,),
    )
    .await
    .unwrap();
    drop(conn);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let native = format!("http://{}/", listener.local_addr().unwrap());
    let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = probe.local_addr().unwrap().port();
    drop(probe);
    let limits = json!({"lite":{"rollingLimit":10,"rollingWindow":5,"weeklyLimit":50,"monthlyLimit":100},"black":{"20":{"fixedLimit":20,"rollingLimit":5,"rollingWindow":5},"100":{"fixedLimit":100,"rollingLimit":25,"rollingWindow":5},"200":{"fixedLimit":200,"rollingLimit":50,"rollingWindow":5}}});
    let router = routes().with_state(super::super::state::ConsoleState {
        db: fixture.db.clone(),
        config: Arc::new(super::super::state::ConsoleConfig {
            support: Config {
                api_key: Some(CANARY.into()),
                limits: Some(limits),
                ..Default::default()
            },
            stage: "production".into(),
            ..Default::default()
        }),
    });
    let shutdown = tokio_util::sync::CancellationToken::new();
    let stop = shutdown.clone();
    let mut server = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(stop.cancelled_owned())
            .await
    });
    // Defer all assertions until child/server/database cleanup completes.
    let checked = async {
        let http = reqwest::Client::builder().timeout(Duration::from_secs(15)).build()?;
        let mut endpoint = reqwest::Url::parse(&format!("{native}api/support/lookup"))?;
        endpoint.query_pairs_mut().append_pair("identifier", EMAIL);
        for bearer in [None, Some("wrong-support-fixture-bearer")] {
            let request = http.get(endpoint.clone());
            let request = if let Some(bearer) = bearer { request.bearer_auth(bearer) } else { request };
            anyhow::ensure!(request.send().await?.status() == reqwest::StatusCode::UNAUTHORIZED, "Native support lookup accepted absent/wrong bearer");
        }
        let response = http.get(endpoint).bearer_auth(CANARY).send().await?;
        anyhow::ensure!(response.status().is_success(), "Native fixture lookup failed: {}", response.status());
        let value: Value = response.json().await?;
        anyhow::ensure!(value["identifier"] == EMAIL, "Lookup identifier mismatch");
        anyhow::ensure!(value["workspaces"][0]["workspaceID"] == WORKSPACE, "Lookup workspace mismatch");
        anyhow::ensure!(value["workspaces"][0]["title"].as_str().is_some_and(|title| title.contains(NAME)), "Persisted workspace name missing");
        anyhow::ensure!(!value.to_string().contains(CANARY), "Native response exposed bearer canary");
        let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/console-support/acceptance.ts");
        let output = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
            let mut child = Command::new("bun").arg(script)
                .env("OPENCODE_SUPPORT_RUST_URL", native)
                .env("OPENCODE_SUPPORT_PORT", port.to_string())
                .env("SUPPORT_API_KEY", CANARY)
                .env("OPENCODE_SUPPORT_TEST_IDENTIFIER", EMAIL)
                .env("OPENCODE_SUPPORT_TEST_EMAIL", EMAIL)
                .env("OPENCODE_SUPPORT_TEST_WORKSPACE", WORKSPACE)
                .env("OPENCODE_SUPPORT_TEST_NAME", NAME)
                .env("OPENCODE_SUPPORT_TEST_TEXT", NAME)
                .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
            let stdout = child.stdout.take().unwrap();
            let stderr = child.stderr.take().unwrap();
            let read = |mut stream: Box<dyn Read + Send>| {
                std::thread::spawn(move || { let mut bytes = Vec::new(); stream.read_to_end(&mut bytes).map(|_| bytes) })
            };
            let stdout = read(Box::new(stdout));
            let stderr = read(Box::new(stderr));
            let deadline = Instant::now() + Duration::from_secs(180);
            let status = loop {
                if let Some(status) = child.try_wait()? { break status; }
                if Instant::now() >= deadline {
                    // Driver handles SIGTERM by closing Chromium and its Node SSR child.
                    let _ = Command::new("kill").args(["-TERM", &child.id().to_string()]).status();
                    let grace = Instant::now() + Duration::from_secs(10);
                    while child.try_wait()?.is_none() && Instant::now() < grace { std::thread::sleep(Duration::from_millis(25)); }
                    if child.try_wait()?.is_none() { child.kill()?; }
                    break child.wait()?;
                }
                std::thread::sleep(Duration::from_millis(25));
            };
            let stdout = stdout.join().map_err(|_| anyhow::anyhow!("Support stdout reader failed"))??;
            let stderr = stderr.join().map_err(|_| anyhow::anyhow!("Support stderr reader failed"))??;
            Ok((status, stdout, stderr))
        }).await??;
        anyhow::ensure!(!output.1.windows(CANARY.len()).any(|bytes| bytes == CANARY.as_bytes()) && !output.2.windows(CANARY.len()).any(|bytes| bytes == CANARY.as_bytes()), "Support child output exposed bearer canary");
        println!("{}", String::from_utf8_lossy(&output.1));
        eprintln!("{}", String::from_utf8_lossy(&output.2));
        anyhow::ensure!(output.0.success(), "Built Node SSR / Chromium support acceptance failed: {}", output.0);
        let mut conn = fixture.db.conn().await?;
        let rows: Option<(String, i64)> = conn.exec_first("SELECT w.name,b.balance FROM workspace w JOIN billing b ON b.workspace_id=w.id WHERE w.id=?", (WORKSPACE,)).await?;
        anyhow::ensure!(rows == Some((NAME.into(), 1230000000)), "Read-only browser lookup changed fixture state");
        Ok::<_, anyhow::Error>(())
    }.await;
    shutdown.cancel();
    let served = tokio::time::timeout(Duration::from_secs(15), &mut server).await;
    if served.is_err() {
        server.abort();
        let _ = server.await;
    }
    fixture.drop_database().await.unwrap();
    checked.unwrap();
    served
        .expect("Native support fixture shutdown timed out")
        .unwrap()
        .unwrap();
}
