//! Purpose: exercise the real generated plugin bridge and native process RPC with controlled JS fixtures.
//! Scope: local interpreter fixtures only, no external network or package installation.
//! Uses:
//! rust/crates/
//! |-- integration-adapters/src/plugins/mod.rs
//! `-- integration-api/src/plugin.rs
//! External:
//! |-- async_trait::async_trait
//! |-- tempfile::tempdir
//! `-- tokio::test
//! Used by:
//! `-- none, entrypoint
//! Invariants: callback requests can complete while the tool response is pending; shutdown joins children.
//! Contract tests: this file

use async_trait::async_trait;
use opencode_integration_adapters::plugins::{Callbacks, Host, Options};
use opencode_integration_api::{
    IntegrationError,
    plugin::{PluginInstance, ToolContext, ToolResult, TriggerName},
    plugin_abi::PluginInputWire,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Recorder(Mutex<Vec<String>>);
#[async_trait]
impl Callbacks for Recorder {
    async fn request(&self, method: &str, _: Value) -> Result<Value, IntegrationError> {
        self.0.lock().unwrap().push(method.into());
        if method == "ctx.ask" {
            return Ok(Value::Null);
        }
        Ok(json!({"data":"local-sdk"}))
    }
    async fn notification(&self, method: &str, _: Value) {
        self.0.lock().unwrap().push(method.into());
    }
}

#[tokio::test]
async fn generated_bridge_loads_deduplicates_threads_hooks_and_awaits_callbacks() {
    let root = tempfile::tempdir().unwrap();
    let runtime = which::which("node").expect("controlled fixture requires installed node");
    let entry = root.path().join("fixture.mjs");
    std::fs::write(&entry, r#"
import {writeFile} from 'node:fs/promises'
export async function fixture(input) {
  console.log('stray plugin stdout must not become protocol')
  return {
    'chat.params': async (_input, output) => { output.temperature = 0.25 },
    config: async () => { const result = await input.client.config.get(); if (result.data !== 'local-sdk') throw Error('callback lost') },
    tool: { echo: { description:'fixture echo', args:{text:{type:'string'}}, execute: async (args, ctx) => {
      await ctx.ask({permission:'echo', patterns:['*'], always:['*']})
      ctx.metadata({title:'fixture',metadata:{called:true}})
      return `echo:${args.text}:${ctx.sessionID}`
    }}},
    dispose: async () => writeFile(new URL('./disposed', import.meta.url), 'joined')
  }
}
export {fixture as alias}
"#).unwrap();
    let callbacks = Arc::new(Recorder::default());
    let host = Host::start(Options {
        interpreter: runtime,
        bridge_directory: root.path().join("bridge"),
        directory: root.path().into(),
        environment: BTreeMap::from([("HOME".into(), root.path().display().to_string())]),
        zod_fallback: None,
        callbacks: callbacks.clone(),
    })
    .await
    .unwrap();
    let plugins = host
        .load(
            &entry,
            None,
            PluginInputWire {
                directory: root.path().display().to_string(),
                worktree: root.path().display().to_string(),
                project: json!({"id":"fixture"}),
                server_url: "http://127.0.0.1:9".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(plugins.len(), 1);
    assert_eq!(plugins[0].tools[0].id, "echo");
    plugins[0].config(&json!({})).await.unwrap();
    assert_eq!(
        plugins[0]
            .trigger(
                TriggerName::ChatParams,
                &json!({}),
                json!({"temperature":1})
            )
            .await
            .unwrap()["temperature"],
        0.25
    );
    assert_eq!(
        plugins[0]
            .execute_tool(
                "echo",
                json!({"text":"hello"}),
                ToolContext {
                    session_id: "ses_fixture".into(),
                    message_id: "msg_fixture".into(),
                    agent: "build".into(),
                    directory: root.path().display().to_string(),
                    worktree: root.path().display().to_string(),
                }
            )
            .await
            .unwrap(),
        ToolResult::Text("echo:hello:ses_fixture".into())
    );
    let instances: Vec<Arc<dyn PluginInstance>> = plugins
        .into_iter()
        .map(|instance| instance as Arc<dyn PluginInstance>)
        .collect();
    host.dispose(&instances).await;
    assert_eq!(
        std::fs::read_to_string(root.path().join("disposed")).unwrap(),
        "joined"
    );
    assert!(callbacks.0.lock().unwrap().contains(&"client.call".into()));
    assert!(callbacks.0.lock().unwrap().contains(&"ctx.ask".into()));
    assert!(host.request("initialize", json!({})).await.is_err());
}
