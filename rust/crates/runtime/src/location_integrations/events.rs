//! Purpose: publish MCP notifications and forward Location bus events to loaded plugins.
//! Scope: bounded, owned Location event pump.
//! Uses:
//! rust/crates/
//! |-- event-api/src/lib.rs
//! |-- integration-adapters/src/mcp/mod.rs
//! `-- integration-api/src/plugin.rs
//! External:
//! |-- futures::StreamExt
//! `-- tokio::sync::mpsc
//! Used by:
//! rust/crates/
//! `-- runtime/src/location_integrations.rs
//! Invariants: bounded overflow is explicit; shutdown joins the pump; events never cross Locations.
//! Contract tests: runtime/tests/location_integrations.rs

use futures::StreamExt;
use opencode_event_api::{Definition, EventStore, LocationRef, PublishOptions};
use opencode_integration_adapters::mcp::{McpEvents, Toast};
use opencode_integration_api::{mcp::McpEvent, plugin::PluginInstance};
use serde_json::{Value, json};
use std::sync::{Arc, RwLock};
use tokio::sync::{Mutex, mpsc};
use tokio_util::sync::CancellationToken;

pub struct IntegrationEvents {
    sender: mpsc::Sender<(&'static str, Value)>,
    plugins: Arc<RwLock<Vec<Arc<dyn PluginInstance>>>>,
    integrations: Arc<RwLock<std::sync::Weak<super::LocationIntegrations>>>,
    stop: CancellationToken,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}
impl IntegrationEvents {
    pub fn start(store: Arc<dyn EventStore>, location: LocationRef) -> Arc<Self> {
        let (sender, mut receiver) = mpsc::channel::<(&'static str, Value)>(256);
        let plugins: Arc<RwLock<Vec<Arc<dyn PluginInstance>>>> = Arc::default();
        let integrations: Arc<RwLock<std::sync::Weak<super::LocationIntegrations>>> =
            Arc::default();
        let catalog = integrations.clone();
        let observed = plugins.clone();
        let stop = CancellationToken::new();
        let cancel = stop.clone();
        let mut events = store.all_bounded(256);
        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    () = cancel.cancelled() => break,
                    Some((name,data)) = receiver.recv() => {
                        if name == "mcp.tools.changed" {
                            let host = catalog.read().unwrap_or_else(|error| error.into_inner()).upgrade();
                            if let Some(host) = host {
                                tokio::select! {
                                    biased;
                                    () = cancel.cancelled() => return,
                                    result = host.refresh_mcp() => { if let Err(error) = result { tracing::error!(%error,"MCP registry refresh failed"); } }
                                }
                            }
                        }
                        if let Err(error) = store.publish(&Definition::ephemeral(name), data, PublishOptions { location:Some(location.clone()), ..Default::default() }).await {
                            tracing::warn!(%error,"integration event publication failed");
                        }
                    }
                    event = events.next() => {
                        let Some(event) = event else { break };
                        let event = match event { Ok(event) => event, Err(error) => { tracing::error!(%error,"integration event stream closed"); break; } };
                        if event.location.as_ref().is_none_or(|source| source.directory != location.directory || source.workspace_id != location.workspace_id) { continue; }
                        let instances = observed.read().unwrap_or_else(|error| error.into_inner()).clone();
                        for plugin in instances {
                            if !plugin.capabilities().event { continue; }
                            let payload = json!({"id":event.id,"type":event.event_type,"properties":event.data});
                            tokio::select! {
                                biased;
                                () = cancel.cancelled() => return,
                                result = plugin.event(&payload) => {
                                    if let Err(error) = result { tracing::warn!(%error,"plugin event callback failed"); }
                                }
                            }
                        }
                    }
                }
            }
        });
        Arc::new(Self {
            sender,
            plugins,
            integrations,
            stop,
            task: Mutex::new(Some(task)),
        })
    }
    pub fn set_integrations(&self, host: &Arc<super::LocationIntegrations>) {
        *self
            .integrations
            .write()
            .unwrap_or_else(|error| error.into_inner()) = Arc::downgrade(host);
    }
    pub fn set_plugins(&self, plugins: Vec<Arc<dyn PluginInstance>>) {
        *self
            .plugins
            .write()
            .unwrap_or_else(|error| error.into_inner()) = plugins;
    }
    fn enqueue(&self, name: &'static str, data: Value) {
        if let Err(error) = self.sender.try_send((name, data)) {
            tracing::error!(%error,"integration event queue overflow or closed");
        }
    }
    pub async fn shutdown(&self) {
        self.stop.cancel();
        if let Some(task) = self.task.lock().await.take() {
            let _ = task.await;
        }
    }
}
impl McpEvents for IntegrationEvents {
    fn publish(&self, event: McpEvent) {
        match event {
            McpEvent::ToolsChanged { properties } => {
                self.enqueue("mcp.tools.changed", json!({"server":properties.server}))
            }
            McpEvent::BrowserOpenFailed { properties } => self.enqueue(
                "mcp.browser.open.failed",
                serde_json::to_value(properties).unwrap_or(Value::Null),
            ),
        }
    }
    fn toast(&self, toast: Toast) {
        self.enqueue("tui.toast.show", json!({"title":toast.title,"message":toast.message,"variant":toast.variant,"duration":toast.duration}));
    }
}
