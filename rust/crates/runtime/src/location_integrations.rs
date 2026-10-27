//! Purpose: compose native MCP, LSP and plugin processes for one Location.
//! Scope: Location-owned integrations, registration guards and joined shutdown.
//! Uses:
//! rust/crates/
//! |-- integration-adapters/src/lsp/mod.rs
//! |-- integration-adapters/src/mcp/mod.rs
//! |-- integration-adapters/src/plugins/mod.rs
//! `-- runtime/src/location_integrations/tools.rs
//! External:
//! |-- serde_json::Value
//! `-- tokio::sync::Mutex
//! Used by:
//! rust/crates/
//! `-- runtime/src/location.rs
//! Invariants: tools use the same permission context as native tools; all interpreter processes are joined.
//! Contract tests: runtime/tests/location_integrations.rs

#[path = "location_integrations/callbacks.rs"]
pub mod callbacks;
#[path = "location_integrations/context.rs"]
pub mod context;
#[path = "location_integrations/events.rs"]
pub mod events;
#[path = "location_integrations/hooks.rs"]
pub mod hooks;
#[path = "location_integrations/mcp_config.rs"]
mod mcp_config;
#[path = "location_integrations/mcp_resources.rs"]
mod mcp_resources;
#[path = "location_integrations/resources.rs"]
pub mod resources;
#[path = "location_integrations/tools.rs"]
mod tools;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use opencode_config::{CurrentConfig, resources as configured};
use opencode_integration_adapters::{lsp, mcp, plugins};
use opencode_integration_api::{
    IntegrationError,
    language::LanguageService,
    plugin::{PluginInstance, TriggerName},
};
use opencode_tool_api::{Registration, ToolRegistry};
use serde_json::Value;

pub struct Options<'a> {
    pub directory: &'a Path,
    pub project: &'a Path,
    pub data: &'a Path,
    pub home: &'a Path,
    pub config: &'a CurrentConfig,
    pub environment: &'a BTreeMap<String, String>,
    pub events: Arc<dyn mcp::McpEvents>,
    pub callbacks: Arc<callbacks::LocationCallbacks>,
}

pub struct LocationIntegrations {
    pub mcp: Arc<mcp::McpServiceImpl>,
    pub lsp: Arc<lsp::LspService>,
    pub plugins: Vec<Arc<plugins::Instance>>,
    pub resources: resources::Resources,
    pub callbacks: Arc<callbacks::LocationCallbacks>,
    pub directory: PathBuf,
    pub project: PathBuf,
    host: Option<Arc<plugins::Host>>,
    catalog: tokio::sync::Mutex<Vec<(ToolRegistry, Registration, Value)>>,
}

impl LocationIntegrations {
    pub async fn build(options: Options<'_>) -> Result<Arc<Self>, IntegrationError> {
        let mut mcp_options = mcp_config::options(options.config, options.directory, options.data)?;
        mcp_options.environment = options.environment.clone();
        mcp_options.events = options.events;
        let mut launch = lsp::LaunchEnv::new(
            options.data.join("bin"),
            options.data.join("packages"),
            options.home.into(),
        );
        launch.env = options.environment.clone();
        launch.path = options.environment.get("PATH").map(Into::into);
        // Resolution must never install packages as a side effect of composing a Location.
        launch.disable_download = true;
        let lsp = Arc::new(lsp::LspService::new(lsp::LspOptions {
            config: options
                .config
                .latest("lsp")
                .map(|value| serde_json::from_value(value.clone()))
                .transpose()
                .map_err(validation)?,
            context: lsp::RootContext {
                directory: options.directory.into(),
                worktree: options.project.into(),
            },
            launch,
            timeouts: lsp::Timeouts::default(),
        }));
        let config =
            options
                .config
                .documents()
                .fold(serde_json::Map::new(), |mut merged, document| {
                    if let Some(value) = document.info.as_object() {
                        merged.extend(value.clone());
                    }
                    merged
                });
        options
            .callbacks
            .bootstrap_config(Value::Object(config.clone()));
        let resources = resources::Resources::load(
            options.config,
            options.home,
            options.directory,
            &options.data.join("cache"),
            options.environment,
        )
        .await?;
        let sources = configured::plugins(options.config.entries(), options.directory);
        let host = if sources.is_empty() {
            None
        } else {
            let interpreter = options
                .environment
                .get("OPENCODE_PLUGIN_RUNTIME")
                .map(PathBuf::from)
                .or_else(|| {
                    options.environment.get("PATH").and_then(|path| {
                        std::env::split_paths(path)
                            .map(|directory| directory.join("bun"))
                            .find(|candidate| candidate.is_file())
                    })
                })
                .ok_or_else(|| IntegrationError::NotFound {
                    kind: "plugin interpreter",
                    name: "bun (or OPENCODE_PLUGIN_RUNTIME)".into(),
                })?;
            Some(
                plugins::Host::start(plugins::Options {
                    interpreter,
                    bridge_directory: options.data.join("plugin-host"),
                    directory: options.directory.into(),
                    environment: options.environment.clone(),
                    zod_fallback: options
                        .environment
                        .get("OPENCODE_PLUGIN_ZOD")
                        .map(PathBuf::from),
                    callbacks: options.callbacks.clone(),
                })
                .await?,
            )
        };
        let mut plugins = Vec::new();
        if let Some(host) = &host {
            for (entry, config) in sources {
                let path = PathBuf::from(&entry);
                if !path.is_absolute() {
                    host.close().await;
                    return Err(IntegrationError::NotYetPorted(
                        "external plugin package resolution/install",
                    ));
                }
                match host
                    .load(
                        &path,
                        config,
                        opencode_integration_api::plugin_abi::PluginInputWire {
                            directory: options.directory.display().to_string(),
                            worktree: options.project.display().to_string(),
                            project: serde_json::json!({"worktree":options.project}),
                            server_url: "http://localhost:4096".into(),
                        },
                    )
                    .await
                {
                    Ok(loaded) => plugins.extend(loaded),
                    Err(error) => {
                        host.close().await;
                        return Err(error);
                    }
                }
            }
            for plugin in &plugins {
                if let Err(error) = plugin.config(&Value::Object(config.clone())).await {
                    host.close().await;
                    return Err(error);
                }
            }
        }
        Ok(Arc::new(Self {
            mcp: Arc::new(mcp::McpServiceImpl::new(mcp_options)),
            lsp,
            plugins,
            host,
            catalog: tokio::sync::Mutex::new(Vec::new()),
            resources,
            callbacks: options.callbacks,
            directory: options.directory.into(),
            project: options.project.into(),
        }))
    }

    pub async fn register_tools(
        self: &Arc<Self>,
        registry: &ToolRegistry,
    ) -> Result<Registration, IntegrationError> {
        let mcp = tools::mcp_tools(self.clone()).await;
        let signature = tools::signature(&mcp);
        let registration = registry
            .register(mcp)
            .map_err(|error| IntegrationError::Validation(error.to_string()))?;
        self.catalog
            .lock()
            .await
            .push((registry.clone(), registration, signature));
        tools::register(self.clone(), registry).await
    }

    pub async fn refresh_mcp(self: &Arc<Self>) -> Result<(), IntegrationError> {
        let mut state = self.catalog.lock().await;
        let tools = tools::mcp_tools(self.clone()).await;
        let signature = tools::signature(&tools);
        for (registry, registration, previous) in state.iter_mut() {
            if *previous != signature {
                *registration = registry
                    .register(tools.clone())
                    .map_err(|error| IntegrationError::Validation(error.to_string()))?;
                *previous = signature.clone();
            }
        }
        Ok(())
    }

    pub async fn trigger(
        &self,
        name: TriggerName,
        input: &Value,
        output: Value,
    ) -> Result<Value, IntegrationError> {
        let plugins: Vec<Arc<dyn PluginInstance>> = self
            .plugins
            .iter()
            .map(|plugin| plugin.clone() as Arc<dyn PluginInstance>)
            .collect();
        opencode_integration_api::plugin::trigger(&plugins, name, input, output).await
    }

    pub async fn shutdown(&self) {
        self.resources.shutdown().await;
        self.catalog.lock().await.clear();
        if let Some(host) = &self.host {
            let plugins: Vec<Arc<dyn PluginInstance>> = self
                .plugins
                .iter()
                .map(|plugin| plugin.clone() as Arc<dyn PluginInstance>)
                .collect();
            host.dispose(&plugins).await;
        }
        self.mcp.shutdown().await;
        self.lsp.shutdown().await;
    }
}

fn validation(error: serde_json::Error) -> IntegrationError {
    IntegrationError::Validation(error.to_string())
}
