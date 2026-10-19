//! Purpose: fixture tests for the MCP service over a real spawned stdio server (Python script in
//! mcp_support): handshake, instructions, paginated catalog, tolerant outputSchema, call_tool
//! translation, roots, env/cwd, timeouts with progress reset, list_changed refresh, close
//! transitions, connect/disconnect/add, descendant cleanup and pagination guards.
//! Scope: integration tests through the public `McpService` boundary. Label: fixture (real
//! process, local script), not live.
//! Uses:
//! rust/crates/
//! |-- integration-adapters/src/mcp/catalog.rs
//! |-- integration-adapters/src/mcp/jsonrpc.rs
//! |-- integration-adapters/tests/mcp_support/mod.rs
//! |-- integration-api/src/error.rs
//! `-- integration-api/src/mcp.rs
//! External:
//! |-- nix::sys::signal::kill
//! |-- nix::unistd::Pid::from_raw
//! |-- serde_json::Map
//! |-- serde_json::Value
//! |-- serde_json::from_str
//! |-- serde_json::json
//! |-- tokio::test
//! |-- tokio::time::sleep
//! `-- url::Url::from_file_path
//! Used by:
//! `-- none, entrypoint
//! Invariants: every service is shut down at the end of its test.
//! Contract tests: integration-adapters/tests/mcp_stdio.rs

mod mcp_support;

use std::collections::BTreeMap;
use std::sync::Arc;

use mcp_support::{Recorder, eventually, fixture, local, service};
use opencode_integration_api::error::IntegrationError;
use opencode_integration_api::mcp::{LocalConfig, McpEvent, McpService, ServerConfig, Status};
use serde_json::{Map, json};

fn args(v: serde_json::Value) -> Map<String, serde_json::Value> {
    v.as_object().cloned().unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread")]
async fn fixture_stdio_catalog_and_calls() {
    let fx = fixture();
    let events = Arc::new(Recorder::default());
    let mut cfg = BTreeMap::new();
    cfg.insert(
        "fix.ture".to_string(),
        local(&fx.script, &[("FIXTURE_VAR", "hello")]),
    );
    let svc = service(&fx, cfg, events.clone());

    assert_eq!(svc.status().await["fix.ture"], Status::Connected);
    let tools = svc.tools().await;
    // Two pages were followed; names are sanitized and keyed `<client>_<tool>`.
    assert!(tools.contains_key("fix_ture_echo"));
    assert!(tools.contains_key("fix_ture_bad_schema"));
    assert_eq!(tools.len(), 11);
    // Page 1 holds an unresolvable outputSchema, so the tolerant re-read strips outputSchema
    // from every tool on that page (baseline TolerantListToolsResultSchema).
    assert!(tools["fix_ture_bad_schema"].def.output_schema.is_none());
    assert!(tools["fix_ture_structured"].def.output_schema.is_none());
    assert_eq!(tools["fix_ture_echo"].timeout_ms, Some(10_000));

    let instr = svc.instructions().await;
    assert_eq!(instr[0].instructions, "use the fixture");
    assert!(instr[0].tools.contains(&"fix_ture_echo".to_string()));

    let r = svc
        .call_tool("fix.ture", "echo", args(json!({"text": "hi"})), None)
        .await
        .unwrap();
    assert_eq!(r["content"][0]["text"], "hi");
    let e = svc
        .call_tool("fix.ture", "fail", Map::new(), None)
        .await
        .unwrap_err();
    assert_eq!(
        e,
        IntegrationError::Failed {
            name: "fix.ture".into(),
            message: "boom\n\nagain".into()
        }
    );
    let s = svc
        .call_tool("fix.ture", "structured", Map::new(), None)
        .await
        .unwrap();
    assert_eq!(s["content"][0]["text"], "{\"ok\":1}");

    let env = svc
        .call_tool("fix.ture", "env", Map::new(), None)
        .await
        .unwrap();
    let text = env["content"][0]["text"].as_str().unwrap().to_string();
    let cwd = std::fs::canonicalize(&fx.workdir).unwrap();
    assert_eq!(text, format!("hello|{}", cwd.display()));

    let roots = svc
        .call_tool("fix.ture", "roots", Map::new(), None)
        .await
        .unwrap();
    let roots: serde_json::Value =
        serde_json::from_str(roots["content"][0]["text"].as_str().unwrap()).unwrap();
    let expected = url::Url::from_file_path(&fx.workdir).unwrap().to_string();
    assert_eq!(roots, json!({"roots": [{"uri": expected}]}));

    let prompts = svc.prompts().await;
    assert_eq!(prompts["fix_ture:greet_me"].client, "fix.ture");
    let res = svc.resources(None).await;
    assert_eq!(
        res["fix.ture:file:///readme"].mime_type.as_deref(),
        Some("text/plain")
    );
    let tpl = svc.resource_templates(Some("fix.ture")).await;
    assert!(tpl.contains_key("fix.ture:file:///{path}"));
    assert!(svc.resource_templates(Some("other")).await.is_empty());

    let mut pargs = BTreeMap::new();
    pargs.insert("who".to_string(), "bob".to_string());
    let p = svc
        .get_prompt("fix.ture", "greet me", Some(pargs))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(p["messages"][0]["content"]["text"], "hi bob");
    let rr = svc
        .read_resource("fix.ture", "file:///readme")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(rr["contents"][0]["text"], "body");
    assert!(svc.read_resource("missing", "x").await.unwrap().is_none());

    // tools/list_changed -> refresh + ToolsChanged event.
    svc.call_tool("fix.ture", "add_tool", Map::new(), None)
        .await
        .unwrap();
    let ev = events.clone();
    assert!(
        eventually(|| ev
            .0
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e, McpEvent::ToolsChanged { .. })))
        .await
    );
    assert!(svc.tools().await.contains_key("fix_ture_added"));
    svc.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn fixture_stdio_timeouts_progress_and_close() {
    let fx = fixture();
    let log = fx.dir.path().join("server.log");
    let events = Arc::new(Recorder::default());
    let mut cfg = BTreeMap::new();
    cfg.insert(
        "s".to_string(),
        local(&fx.script, &[("MCP_FIXTURE_LOG", log.to_str().unwrap())]),
    );
    let svc = service(&fx, cfg, events.clone());

    let e = svc
        .call_tool("s", "slow", Map::new(), Some(300))
        .await
        .unwrap_err();
    assert_eq!(e, IntegrationError::Timeout { ms: 300 });
    // The fixture serves requests sequentially; let the abandoned slow call drain first.
    tokio::time::sleep(std::time::Duration::from_millis(2200)).await;
    let ok = svc
        .call_tool("s", "progress", Map::new(), Some(400))
        .await
        .unwrap();
    assert_eq!(
        ok["content"][0]["text"], "done",
        "progress notifications reset the timeout"
    );
    let sent = std::fs::read_to_string(&log).unwrap();
    assert!(
        sent.contains("notifications/cancelled"),
        "timeouts send cancellation"
    );
    assert!(sent.contains("notifications/initialized"));

    // Server exit -> failed "Connection closed" + ToolsChanged.
    let _ = svc.call_tool("s", "exit", Map::new(), Some(500)).await;
    let s2 = svc.clone();
    let mut ok = false;
    for _ in 0..100 {
        if s2.status().await["s"]
            == (Status::Failed {
                error: "Connection closed".into(),
            })
        {
            ok = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(ok);
    assert!(svc.tools().await.is_empty());

    // Reconnect restores the server.
    svc.connect("s").await.unwrap();
    assert_eq!(svc.status().await["s"], Status::Connected);
    svc.disconnect("s").await.unwrap();
    assert_eq!(svc.status().await["s"], Status::Disabled);
    assert!(matches!(
        svc.connect("nope").await,
        Err(IntegrationError::NotFound { .. })
    ));
    svc.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn fixture_stdio_add_failures_and_guards() {
    let fx = fixture();
    let svc = service(&fx, BTreeMap::new(), Arc::new(Recorder::default()));

    let missing = ServerConfig::Local(LocalConfig {
        command: vec!["definitely-not-a-binary-xyz".into()],
        cwd: None,
        environment: None,
        enabled: None,
        timeout: Some(2000),
    });
    let st = svc.add("bad", missing).await.unwrap();
    assert_eq!(
        st["bad"],
        Status::Failed {
            error: "spawn definitely-not-a-binary-xyz ENOENT".into()
        }
    );

    // Duplicate cursor -> tool listing fails -> "Failed to get tools".
    let st = svc
        .add("dup", local(&fx.script, &[("MCP_DUP_CURSOR", "1")]))
        .await
        .unwrap();
    assert_eq!(
        st["dup"],
        Status::Failed {
            error: "Failed to get tools".into()
        }
    );

    let mut disabled = local(&fx.script, &[]);
    if let ServerConfig::Local(l) = &mut disabled {
        l.enabled = Some(false);
        l.cwd = Some("sub".into());
    }
    let st = svc.add("off", disabled).await.unwrap();
    assert_eq!(st["off"], Status::Disabled);
    // connect() forces enabled and resolves cwd relative to the Location directory.
    svc.connect("off").await.unwrap();
    let env = svc.call_tool("off", "env", Map::new(), None).await.unwrap();
    let sub = std::fs::canonicalize(fx.workdir.join("sub")).unwrap();
    assert!(
        env["content"][0]["text"]
            .as_str()
            .unwrap()
            .ends_with(&sub.display().to_string())
    );

    let status = svc.status().await;
    assert_eq!(
        status.keys().cloned().collect::<Vec<_>>(),
        vec!["bad", "dup", "off"]
    );
    svc.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn fixture_stdio_shutdown_kills_descendants() {
    let fx = fixture();
    let mut cfg = BTreeMap::new();
    cfg.insert("p".to_string(), local(&fx.script, &[]));
    let svc = service(&fx, cfg, Arc::new(Recorder::default()));
    let r = svc
        .call_tool("p", "spawn_child", Map::new(), None)
        .await
        .unwrap();
    let pid: i32 = r["content"][0]["text"].as_str().unwrap().parse().unwrap();
    let alive = |pid: i32| nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok();
    assert!(alive(pid));
    svc.shutdown().await;
    let mut gone = false;
    for _ in 0..60 {
        // The grandchild is reparented; a zombie still answers kill(0), so check `ps` state too.
        let out = std::process::Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .unwrap();
        let stat = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !alive(pid) || stat.is_empty() || stat.starts_with('Z') {
            gone = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(gone, "descendant process was SIGTERMed on shutdown");
}

#[tokio::test(flavor = "multi_thread")]
async fn fixture_pagination_guards() {
    use opencode_integration_adapters::mcp::catalog::paginate;
    use opencode_integration_adapters::mcp::jsonrpc::RpcError;
    let mut n = 0u32;
    let r = paginate(
        |_| {
            n += 1;
            let page = json!({"items": [n], "nextCursor": format!("c{n}")});
            async move { Ok::<_, RpcError>(page) }
        },
        "items",
    )
    .await;
    assert_eq!(r.unwrap_err().to_string(), "MCP list exceeded 1000 pages");
    assert_eq!(n, 1000);
}
