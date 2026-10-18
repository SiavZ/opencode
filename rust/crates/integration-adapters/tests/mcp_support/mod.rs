//! Purpose: shared fixture peers for MCP integration tests: a tiny stdio MCP server written in
//! Python (spawned for real) and in-process axum peers (streamable HTTP, legacy SSE, OAuth AS).
//! Scope: test support only, included by the mcp_* test files via `mod`.
//! Uses:
//! rust/crates/
//! `-- integration-adapters/src/mcp/mod.rs
//! External:
//! |-- axum::{Router, routing}
//! `-- tempfile::TempDir
//! Used by:
//! rust/crates/
//! |-- integration-adapters/tests/mcp_http.rs
//! |-- integration-adapters/tests/mcp_oauth.rs
//! `-- integration-adapters/tests/mcp_stdio.rs
//! Invariants: every peer binds 127.0.0.1:0; no real user data dirs are touched.
//! Contract tests: integration-adapters/tests/mcp_stdio.rs

#![allow(dead_code)]

pub mod http;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use opencode_integration_adapters::mcp::{McpEvents, McpOptions, McpServiceImpl};
use opencode_integration_api::mcp::{LocalConfig, McpEvent, ServerConfig};

/// Fixture stdio MCP server (Python 3, stdlib only). Behaviour switches via env vars.
pub const STDIO_SERVER: &str = r##"
import json, os, sys, time, subprocess
tools = [
  {"name": "echo", "description": "echo text", "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}}},
  {"name": "fail", "inputSchema": {"type": "object"}},
  {"name": "structured", "inputSchema": {"type": "object"}, "outputSchema": {"type": "object"}},
  {"name": "bad schema", "inputSchema": {"type": "object"}, "outputSchema": {"$ref": "#/missing"}},
  {"name": "env", "inputSchema": {"type": "object"}},
  {"name": "roots", "inputSchema": {"type": "object"}},
  {"name": "slow", "inputSchema": {"type": "object"}},
  {"name": "progress", "inputSchema": {"type": "object"}},
  {"name": "add_tool", "inputSchema": {"type": "object"}},
  {"name": "spawn_child", "inputSchema": {"type": "object"}},
  {"name": "exit", "inputSchema": {"type": "object"}},
]
def send(m):
  sys.stdout.write(json.dumps(m) + "\n"); sys.stdout.flush()
def text(t): return {"content": [{"type": "text", "text": t}]}
def read():
  line = sys.stdin.readline()
  if not line: sys.exit(0)
  return json.loads(line)
log = open(os.environ["MCP_FIXTURE_LOG"], "a") if os.environ.get("MCP_FIXTURE_LOG") else None
while True:
  msg = read()
  if log: log.write(json.dumps(msg) + "\n"); log.flush()
  method, mid, params = msg.get("method"), msg.get("id"), msg.get("params") or {}
  if mid is None: continue
  if method == "initialize":
    send({"jsonrpc": "2.0", "id": mid, "result": {"protocolVersion": params["protocolVersion"],
      "capabilities": {"tools": {"listChanged": True}, "prompts": {}, "resources": {}},
      "serverInfo": {"name": "fixture", "version": "1"}, "instructions": "  use the fixture  "}})
  elif method == "tools/list":
    if os.environ.get("MCP_DUP_CURSOR"):
      send({"jsonrpc": "2.0", "id": mid, "result": {"tools": [], "nextCursor": "same"}}); continue
    half = len(tools) // 2
    if params.get("cursor") == "p2": send({"jsonrpc": "2.0", "id": mid, "result": {"tools": tools[half:]}})
    else: send({"jsonrpc": "2.0", "id": mid, "result": {"tools": tools[:half], "nextCursor": "p2"}})
  elif method == "prompts/list":
    send({"jsonrpc": "2.0", "id": mid, "result": {"prompts": [{"name": "greet me", "description": "d", "arguments": [{"name": "who", "required": True}]}]}})
  elif method == "prompts/get":
    who = (params.get("arguments") or {}).get("who", "")
    send({"jsonrpc": "2.0", "id": mid, "result": {"messages": [{"role": "user", "content": {"type": "text", "text": "hi " + who}}]}})
  elif method == "resources/list":
    send({"jsonrpc": "2.0", "id": mid, "result": {"resources": [{"name": "readme", "uri": "file:///readme", "mimeType": "text/plain"}]}})
  elif method == "resources/templates/list":
    send({"jsonrpc": "2.0", "id": mid, "result": {"resourceTemplates": [{"name": "doc", "uriTemplate": "file:///{path}"}]}})
  elif method == "resources/read":
    send({"jsonrpc": "2.0", "id": mid, "result": {"contents": [{"uri": params["uri"], "text": "body"}]}})
  elif method == "tools/call":
    name, args = params["name"], params.get("arguments") or {}
    if name == "echo": send({"jsonrpc": "2.0", "id": mid, "result": text(args.get("text", ""))})
    elif name == "fail": send({"jsonrpc": "2.0", "id": mid, "result": {"isError": True, "content": [{"type": "text", "text": "boom"}, {"type": "text", "text": "again"}]}})
    elif name == "structured": send({"jsonrpc": "2.0", "id": mid, "result": {"content": [], "structuredContent": {"ok": 1}}})
    elif name == "env": send({"jsonrpc": "2.0", "id": mid, "result": text(os.environ.get("FIXTURE_VAR", "") + "|" + os.getcwd())})
    elif name == "roots":
      send({"jsonrpc": "2.0", "id": "srv-1", "method": "roots/list"})
      while True:
        r = read()
        if r.get("id") == "srv-1": break
      send({"jsonrpc": "2.0", "id": mid, "result": text(json.dumps(r["result"]))})
    elif name == "slow": time.sleep(2); send({"jsonrpc": "2.0", "id": mid, "result": text("late")})
    elif name == "progress":
      tok = params["_meta"]["progressToken"]
      for i in range(8):
        time.sleep(0.1)
        send({"jsonrpc": "2.0", "method": "notifications/progress", "params": {"progressToken": tok, "progress": i}})
      send({"jsonrpc": "2.0", "id": mid, "result": text("done")})
    elif name == "add_tool":
      tools.append({"name": "added", "inputSchema": {"type": "object"}})
      send({"jsonrpc": "2.0", "id": mid, "result": text("ok")})
      send({"jsonrpc": "2.0", "method": "notifications/tools/list_changed"})
    elif name == "spawn_child":
      p = subprocess.Popen(["sleep", "300"])
      send({"jsonrpc": "2.0", "id": mid, "result": text(str(p.pid))})
    elif name == "exit": sys.exit(0)
  else:
    send({"jsonrpc": "2.0", "id": mid, "error": {"code": -32601, "message": "Method not found"}})
"##;

pub struct Fixture {
    pub dir: tempfile::TempDir,
    pub script: PathBuf,
    pub data: PathBuf,
    pub workdir: PathBuf,
}

pub fn fixture() -> Fixture {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = dir.path().join("server.py");
    std::fs::write(&script, STDIO_SERVER).expect("write script");
    let data = dir.path().join("data");
    let workdir = dir.path().join("work");
    std::fs::create_dir_all(workdir.join("sub")).expect("workdir");
    Fixture {
        script,
        data,
        workdir,
        dir,
    }
}

pub fn local(script: &Path, env: &[(&str, &str)]) -> ServerConfig {
    ServerConfig::Local(LocalConfig {
        command: vec!["python3".into(), "-u".into(), script.display().to_string()],
        cwd: None,
        environment: (!env.is_empty()).then(|| {
            env.iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        }),
        enabled: None,
        timeout: Some(10_000),
    })
}

#[derive(Default)]
pub struct Recorder(pub Mutex<Vec<McpEvent>>, pub Mutex<Vec<String>>);

impl McpEvents for Recorder {
    fn publish(&self, event: McpEvent) {
        self.0.lock().unwrap().push(event);
    }
    fn toast(&self, toast: opencode_integration_adapters::mcp::Toast) {
        self.1.lock().unwrap().push(toast.message);
    }
}

pub fn service(
    fx: &Fixture,
    config: BTreeMap<String, ServerConfig>,
    events: Arc<Recorder>,
) -> McpServiceImpl {
    let mut opts = McpOptions::new(fx.workdir.clone(), fx.data.clone(), config);
    opts.events = events;
    McpServiceImpl::new(opts)
}

pub async fn eventually<F: FnMut() -> bool>(mut f: F) -> bool {
    for _ in 0..100 {
        if f() {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    false
}
