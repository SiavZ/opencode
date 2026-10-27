//! Purpose: adapt actual agent/model/command registries to legacy Session contracts.
//! Scope: wire-shape conversion only, no duplicate catalog or configuration discovery.
//! Invariants: missing models and agents remain typed failures; permissions keep insertion order.
//! Contract tests: runtime/tests/legacy_model.rs

use std::sync::Arc;

use opencode_server::middleware::legacy_routing::RouteContext;
use opencode_server::services::legacy::LegacySessions;
use opencode_session_runtime::legacy::ports::{CommandInfo, LegacyCommands, LegacyProvider};
use opencode_session_runtime::legacy::types::{LegacyError, ModelRef, Object};
use serde_json::Value;

pub use super::agents::Agents;

pub struct Provider(pub Arc<crate::provider_services::ProviderServices>);

#[async_trait::async_trait]
impl LegacyProvider for Provider {
    async fn get_model(&self, provider: &str, model: &str) -> Result<Object, LegacyError> {
        let value = self
            .0
            .handle(
                "provider.list",
                &RouteContext {
                    directory: String::new(),
                    workspace_id: None,
                },
                &Object::new(),
                &Object::new(),
                None,
            )
            .await
            .map_err(|error| LegacyError::Unknown(error.to_string()))?;
        value["all"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|item| item["id"] == provider)
            .and_then(|provider| provider["models"].get(model))
            .and_then(Value::as_object)
            .cloned()
            .ok_or_else(|| LegacyError::ModelNotFound {
                provider_id: provider.into(),
                model_id: model.into(),
                suggestions: Vec::new(),
            })
    }
    async fn default_model(&self) -> Result<ModelRef, LegacyError> {
        let model = self
            .0
            .catalog()
            .await
            .map_err(LegacyError::Unknown)?
            .default_model()
            .ok_or_else(|| LegacyError::Unknown("No default model".into()))?;
        Ok(ModelRef::new(model.provider_id.as_str(), model.id.as_str()))
    }
}

pub struct Commands(
    pub Arc<crate::location_integrations::LocationIntegrations>,
    pub String,
);

#[async_trait::async_trait]
impl LegacyCommands for Commands {
    async fn get(&self, name: &str) -> Result<Option<CommandInfo>, LegacyError> {
        let configured = self
            .list()
            .await?
            .into_iter()
            .find(|command| command.name == name);
        if configured.is_some() {
            return Ok(configured);
        }
        Ok((name == "init").then(|| CommandInfo {
            name: "init".into(),
            description: Some("guided AGENTS.md setup".into()),
            agent: None,
            model: None,
            source: Some("command".into()),
            template: include_str!("init.txt").replace("${path}", &self.1),
            subtask: None,
            hints: vec!["$ARGUMENTS".into()],
        }))
    }
    async fn list(&self) -> Result<Vec<CommandInfo>, LegacyError> {
        Ok(self
            .0
            .resources
            .commands
            .iter()
            .map(|value| CommandInfo {
                name: value["name"].as_str().unwrap_or_default().into(),
                description: value["description"].as_str().map(str::to_owned),
                agent: value["agent"].as_str().map(str::to_owned),
                model: value["model"].as_str().map(str::to_owned),
                source: Some("command".into()),
                template: value["template"].as_str().unwrap_or_default().into(),
                subtask: value["subtask"].as_bool(),
                hints: Vec::new(),
            })
            .collect())
    }
}
