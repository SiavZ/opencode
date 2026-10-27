//! Purpose: prove the production Location composes real plugin, MCP, LSP, skills and references.
//! Scope: isolated Process, native interpreter subprocesses and actual shared tool registry.
//! Uses:
//! rust/crates/
//! |-- integration-adapters/tests/fixtures/lsp/fake_lsp.py
//! |-- runtime/src/location_integrations.rs
//! `-- runtime/src/process.rs
//! External:
//! |-- serde_json::json
//! `-- tempfile::tempdir
//! Used by:
//! `-- none, entrypoint
//! Invariants: no external providers, package installs, user config or injected builtin-only hosts.
//! Contract tests: this file

use opencode_context::source::initialize;
use opencode_integration_api::{language::LanguageService, mcp::McpService};
use opencode_runtime::{
    Process, ProcessOptions,
    location_integrations::{context::IntegrationContext, hooks::IntegrationMaterializer},
};
use opencode_schema::{location::Ref, values::AbsolutePath};
use opencode_session_api::{
    MessageID, SessionID,
    operations::{CreateRequest, SessionService},
};
use opencode_session_runtime::runner::ports::{SettleInput, SystemContextLoader, ToolMaterializer};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

fn executable(name: &str) -> PathBuf {
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|path| path.join(name))
        .find(|path| path.is_file())
        .expect("installed local fixture interpreter")
}
fn call(name: &str, input: Value) -> SettleInput {
    SettleInput {
        session_id: SessionID::new("ses_integration").unwrap(),
        assistant_message_id: MessageID::create(),
        agent: "build".into(),
        call_id: "fixture-call".into(),
        name: name.into(),
        input,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_location_composes_registered_process_tools_bootstrap_callbacks_and_guidance() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().canonicalize().unwrap().join("project");
    std::fs::create_dir_all(directory.join("skills/fixture")).unwrap();
    std::fs::create_dir_all(directory.join("reference")).unwrap();
    std::fs::write(
        directory.join("skills/fixture/SKILL.md"),
        "---\nname: fixture\ndescription: Native fixture skill\n---\nUse real integration evidence",
    )
    .unwrap();
    std::fs::write(directory.join("sample.ts"), "const sample = 1").unwrap();
    let plugin = directory.join("plugin.mjs");
    std::fs::write(&plugin,r#"
import {writeFile} from 'node:fs/promises'
export default async function(input) {
  const config = await input.client.config.get()
  if (config.data.default_agent !== 'build') throw Error('bootstrap real config missing')
  return {
    config: async () => { await writeFile(new URL('./configured',import.meta.url),'real callback') },
    'tool.execute.before': async (_input, output) => { if (output.args.text) output.args.text += ':before' },
    'tool.execute.after': async (_input, output) => { output.output += ':after'; if (output.output.startsWith('grow')) output.output += ' retained-plugin-output'.repeat(6000) },
    tool: {fixture_echo:{description:'Real plugin echo',args:{text:{type:'string'}},execute: async (args,ctx) => {
      await ctx.ask({permission:'fixture_echo',patterns:['*'],always:['*']})
      const config = await input.client.config.get()
      return `${args.text}:${config.data.default_agent}`
    }}},
    dispose: async () => { await writeFile(new URL('./disposed',import.meta.url),'joined') }
  }
}
"#).unwrap();
    let mcp = directory.join("mcp.mjs");
    std::fs::write(&mcp,r#"
import {createInterface} from 'node:readline'
for await (const line of createInterface({input:process.stdin})) {
  const request=JSON.parse(line)
  if (request.id === undefined) continue
  const result = request.method === 'initialize' ? {protocolVersion:'2024-11-05',capabilities:{tools:{},resources:{},prompts:{}},serverInfo:{name:'fixture',version:'1'},instructions:'MCP native fixture instruction'}
    : request.method === 'tools/list' ? {tools:[{name:'echo',description:'MCP echo',inputSchema:{type:'object',properties:{text:{type:'string'}},required:['text']}}]}
    : request.method === 'tools/call' ? {content:[{type:'text',text:'mcp:'+request.params.arguments.text}]}
    : request.method === 'resources/list' ? {resources:[{name:'fixture',uri:'fixture://read',mimeType:'text/plain'}]}
    : request.method === 'resources/read' ? {contents:[{uri:request.params.uri,mimeType:'text/plain',text:'native resource evidence'}]}
    : request.method === 'prompts/list' ? {prompts:[]} : {}
  process.stdout.write(JSON.stringify({jsonrpc:'2.0',id:request.id,result})+'\n')
}
"#).unwrap();
    let mut options = ProcessOptions::isolated(&root.path().join("host"));
    options.environment.vars.insert(
        "OPENCODE_PLUGIN_RUNTIME".into(),
        executable("node").display().to_string(),
    );
    std::fs::create_dir_all(&options.paths.config).unwrap();
    let lsp = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../integration-adapters/tests/fixtures/lsp/fake_lsp.py");
    std::fs::write(options.paths.config.join("opencode.json"),json!({
        "default_agent":"build","plugins":[plugin],"skills":[directory.join("skills")],
        "references":{"fixture":{"path":directory.join("reference"),"description":"Native reference fixture"}},
        "mcp":{"timeout":{"startup":5000,"request":1000},"servers":{"Fixture Server":{"type":"local","command":[executable("node"),mcp],"disabled":false}}},
        "lsp":{"fixture":{"extensions":[".ts"],"command":[executable("python3"),"-u",lsp],"env":{"FAKE_LSP_LOG":directory.join("lsp.log")}},"eslint":{"disabled":true}}
    }).to_string()).unwrap();
    let process = Process::start(options).await.unwrap();
    process
        .session
        .create(CreateRequest {
            id: Some(SessionID::new("ses_integration").unwrap()),
            agent: None,
            model: None,
            location: Ref {
                directory: AbsolutePath::new(directory.display().to_string()),
                workspace_id: None,
            },
        })
        .await
        .unwrap();
    let location = tokio::time::timeout(
        Duration::from_secs(20),
        process.locations.get(&Ref {
            directory: AbsolutePath::new(directory.display().to_string()),
            workspace_id: None,
        }),
    )
    .await
    .expect("bootstrap callback cannot recursively acquire Location")
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(directory.join("configured")).unwrap(),
        "real callback"
    );
    assert_eq!(location.integrations.plugins.len(), 1);
    let tools = IntegrationMaterializer {
        source: location.materializer.clone(),
        inner: location.materializer.clone(),
        integrations: location.integrations.clone(),
    };
    let selected = location.agents.select(None);
    let permissions = &selected.info.as_ref().unwrap().permissions;
    let denied = tools.materialize(&[opencode_schema::permission::Rule {
        action: "fixture_echo".into(),
        resource: "*".into(),
        effect: opencode_schema::permission::Effect::Deny,
    }]);
    assert!(
        !denied
            .definitions()
            .iter()
            .any(|tool| tool.name == "fixture_echo")
    );
    drop(denied);
    let state = process
        .legacy_command(
            "mcp.cli.list",
            directory.clone(),
            Default::default(),
            Default::default(),
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        state["statuses"]["Fixture Server"]["status"], "connected",
        "{state}"
    );
    let legacy_tools = IntegrationMaterializer {
        source: location.legacy_materializer.clone(),
        inner: location.legacy_materializer.clone(),
        integrations: location.integrations.clone(),
    };
    assert!(
        legacy_tools
            .materialize(permissions)
            .definitions()
            .iter()
            .any(|tool| tool.name == "fixture_echo")
    );
    let materialized = tools.materialize(permissions);
    assert!(
        materialized
            .definitions()
            .iter()
            .any(|tool| tool.name == "fixture_echo")
    );
    let echo = materialized
        .settle(
            call("fixture_echo", json!({"text":"plugin"})),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(
        serde_json::to_string(&echo.result)
            .unwrap()
            .contains("plugin:before:build:after"),
        "{echo:?}"
    );
    let grown = materialized
        .settle(
            call("fixture_echo", json!({"text":"grow"})),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(
        !grown.output_paths.is_empty(),
        "after-hook growth must use actual retained output store"
    );
    let retained = std::fs::read_to_string(&grown.output_paths[0]).unwrap();
    assert!(retained.contains(&" retained-plugin-output".repeat(6000)));
    assert!(serde_json::to_string(&grown.result).unwrap().len() < retained.len());
    let mcp_name = materialized
        .definitions()
        .iter()
        .find(|tool| tool.name.contains("echo") && tool.name != "fixture_echo")
        .unwrap()
        .name
        .clone();
    let echo = materialized
        .settle(
            call(&mcp_name, json!({"text":"native"})),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(
        serde_json::to_string(&echo.result)
            .unwrap()
            .contains("mcp:native:before:after")
    );
    let resource = materialized
        .settle(
            call(
                "read_mcp_resource",
                json!({"server":"Fixture Server","uri":"fixture://read"}),
            ),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(
        serde_json::to_string(&resource.result)
            .unwrap()
            .contains("native resource evidence")
    );
    let skill = materialized
        .settle(
            call("skill", json!({"name":"fixture"})),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(
        serde_json::to_string(&skill.result)
            .unwrap()
            .contains("Use real integration evidence")
    );
    let context = IntegrationContext {
        registry: location.context.clone(),
        integrations: location.integrations.clone(),
    }
    .load(&selected)
    .await
    .unwrap();
    let context = initialize(&context).await.unwrap();
    assert!(context.baseline.contains("Native fixture skill"));
    assert!(context.baseline.contains("Native reference fixture"));
    assert!(context.baseline.contains("MCP native fixture instruction"));
    location
        .integrations
        .lsp
        .touch_file(directory.join("sample.ts").to_str().unwrap(), None)
        .await
        .unwrap();
    assert!(
        location
            .integrations
            .lsp
            .status()
            .await
            .iter()
            .any(|status| status.id == "fixture")
    );
    assert!(!location.integrations.mcp.status().await.is_empty());
    let mut params = serde_json::Map::new();
    params.insert("name".into(), json!("Fixture Server"));
    process
        .legacy_command(
            "mcp.disconnect",
            directory.clone(),
            params,
            Default::default(),
            None,
        )
        .await
        .unwrap();
    let disconnected = tools.materialize(permissions);
    assert!(
        !disconnected
            .definitions()
            .iter()
            .any(|tool| tool.name == mcp_name || tool.name == "read_mcp_resource")
    );
    drop(disconnected);
    let disconnected = legacy_tools.materialize(permissions);
    assert!(
        !disconnected
            .definitions()
            .iter()
            .any(|tool| tool.name == mcp_name || tool.name == "read_mcp_resource")
    );
    drop(disconnected);
    drop(legacy_tools);
    drop(materialized);
    drop(tools);
    drop(location);
    process.shutdown().await;
    assert_eq!(
        std::fs::read_to_string(directory.join("disposed")).unwrap(),
        "joined"
    );
}
