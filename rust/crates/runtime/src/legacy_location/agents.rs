//! Purpose: materialize the actual legacy built-in agent registry and configuration overlays.
//! Scope: V1 agent defaults, permissions, model selection and prompt settings, separate from V2.
//! Invariants: user rules append after defaults; disabled agents stay absent; default agents are visible primary agents.
//! Contract tests: runtime/tests/legacy_model.rs, runtime/tests/legacy_compaction.rs

use std::path::Path;

use opencode_session_runtime::legacy::ports::LegacyAgents;
use opencode_session_runtime::legacy::store::object;
use opencode_session_runtime::legacy::types::{AgentInfo, LegacyError, ModelRef, Object};
use serde_json::{Value, json};

pub struct Agents {
    values: Vec<AgentInfo>,
    settings: Object,
    default: Option<String>,
}

impl Agents {
    pub fn new(
        config: &Value,
        data: &Path,
        temp: &Path,
        worktree: &Path,
        references: &[Value],
        skills: &[Value],
    ) -> Self {
        let whitelist = std::iter::once(data.join("tool-output").join("*").display().to_string())
            .chain(std::iter::once(temp.join("*").display().to_string()))
            .chain(
                references
                    .iter()
                    .chain(skills)
                    .filter_map(|entry| entry["path"].as_str())
                    .map(|path| Path::new(path).join("*").display().to_string()),
            )
            .collect::<Vec<_>>();
        let mut external = object(json!({"*":"ask"}));
        for path in &whitelist {
            external.insert(path.clone(), json!("allow"));
        }
        let defaults = rules(
            &json!({"*":"allow","doom_loop":"ask","external_directory":external,"question":"deny","plan_enter":"deny","plan_exit":"deny","read":{"*":"allow","*.env":"ask","*.env.*":"ask","*.env.example":"allow"}}),
        );
        let user = rules(&config["permission"]);
        let mut plan_edit = object(json!({"*":"deny",".opencode/plans/*.md":"allow"}));
        plan_edit.insert(
            opencode_session_runtime::service::relative(
                &worktree.display().to_string(),
                &data.join("plans/*.md").display().to_string(),
            ),
            json!("allow"),
        );
        let builtins = [
            (
                "build",
                "primary",
                false,
                json!({"question":"allow","plan_enter":"allow"}),
            ),
            (
                "plan",
                "primary",
                false,
                json!({"question":"allow","plan_exit":"allow","task":{"general":"deny"},"external_directory":{data.join("plans/*").display().to_string():"allow"},"edit":plan_edit}),
            ),
            ("general", "subagent", false, json!({"todowrite":"deny"})),
            (
                "explore",
                "subagent",
                false,
                json!({"*":"deny","grep":"allow","glob":"allow","list":"allow","bash":"allow","webfetch":"allow","websearch":"allow","read":"allow","external_directory":external}),
            ),
            ("compaction", "primary", true, json!({"*":"deny"})),
            ("title", "primary", true, json!({"*":"deny"})),
            ("summary", "primary", true, json!({"*":"deny"})),
        ];
        let mut values = builtins
            .into_iter()
            .map(|(name, mode, hidden, permission)| AgentInfo {
                name: name.into(),
                mode: mode.into(),
                hidden: Some(hidden),
                permission: defaults
                    .iter()
                    .cloned()
                    .chain(rules(&permission))
                    .chain(user.iter().cloned())
                    .collect(),
                model: None,
                variant: None,
                steps: None,
            })
            .collect::<Vec<_>>();
        let mut settings = object(json!({
            "explore":{"prompt":include_str!("../../../../../packages/opencode/src/agent/prompt/explore.txt")},
            "compaction":{"prompt":include_str!("../../../../../packages/opencode/src/agent/prompt/compaction.txt")},
            "title":{"prompt":include_str!("../../../../../packages/opencode/src/agent/prompt/title.txt"),"temperature":0.5},
            "summary":{"prompt":include_str!("../../../../../packages/opencode/src/agent/prompt/summary.txt")}
        }));
        for (name, value) in config["agent"].as_object().into_iter().flatten() {
            if value["disable"] == true {
                values.retain(|agent| agent.name != *name);
                settings.shift_remove(name);
                continue;
            }
            let index = values
                .iter()
                .position(|agent| agent.name == *name)
                .unwrap_or_else(|| {
                    values.push(AgentInfo {
                        name: name.clone(),
                        mode: "all".into(),
                        hidden: None,
                        permission: defaults
                            .iter()
                            .cloned()
                            .chain(user.iter().cloned())
                            .collect(),
                        model: None,
                        variant: None,
                        steps: None,
                    });
                    values.len() - 1
                });
            let agent = &mut values[index];
            if let Some(model) = value["model"]
                .as_str()
                .and_then(|model| model.split_once('/'))
            {
                agent.model = Some(ModelRef::new(model.0, model.1));
            }
            agent.variant = value["variant"]
                .as_str()
                .map(str::to_owned)
                .or_else(|| agent.variant.clone());
            agent.mode = value["mode"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| agent.mode.clone());
            agent.hidden = value["hidden"].as_bool().or(agent.hidden);
            agent.steps = value["steps"].as_f64().or(agent.steps);
            agent.permission.extend(rules(&value["permission"]));
            let entry = settings.entry(name.clone()).or_insert_with(|| json!({}));
            if let (Some(entry), Some(value)) = (entry.as_object_mut(), value.as_object()) {
                entry.extend(value.clone());
            }
        }
        let output = data.join("tool-output/*").display().to_string();
        for agent in &mut values {
            if !agent.permission.iter().any(|rule| {
                rule["permission"] == "external_directory"
                    && rule["pattern"] == output
                    && rule["action"] == "deny"
            }) {
                agent.permission.push(object(
                    json!({"permission":"external_directory","pattern":output,"action":"allow"}),
                ));
            }
        }
        Self {
            values,
            settings,
            default: config["default_agent"].as_str().map(str::to_owned),
        }
    }
}

#[async_trait::async_trait]
impl LegacyAgents for Agents {
    async fn get(&self, name: &str) -> Result<Option<AgentInfo>, LegacyError> {
        Ok(self.values.iter().find(|agent| agent.name == name).cloned())
    }
    async fn list(&self) -> Result<Vec<AgentInfo>, LegacyError> {
        let mut values = self.values.clone();
        let default = self.default.as_deref().unwrap_or("build");
        values.sort_by(|a, b| {
            (a.name != default)
                .cmp(&(b.name != default))
                .then_with(|| a.name.cmp(&b.name))
        });
        Ok(values)
    }
    async fn default_info(&self) -> Result<AgentInfo, LegacyError> {
        if let Some(default) = &self.default {
            let agent = self
                .values
                .iter()
                .find(|agent| &agent.name == default)
                .ok_or_else(|| {
                    LegacyError::Unknown(format!("default agent \"{default}\" not found"))
                })?;
            if agent.mode == "subagent" {
                return Err(LegacyError::Unknown(format!(
                    "default agent \"{default}\" is a subagent"
                )));
            }
            if agent.hidden == Some(true) {
                return Err(LegacyError::Unknown(format!(
                    "default agent \"{default}\" is hidden"
                )));
            }
            return Ok(agent.clone());
        }
        self.values
            .iter()
            .find(|agent| agent.mode != "subagent" && agent.hidden != Some(true))
            .cloned()
            .ok_or_else(|| LegacyError::Unknown("no primary visible agent found".into()))
    }
    async fn settings(&self, name: &str) -> Result<Object, LegacyError> {
        Ok(self
            .settings
            .get(name)
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default())
    }
}

fn rules(permission: &Value) -> Vec<Object> {
    opencode_config::migrate::migrate(&json!({"permission":permission}))["permissions"].as_array().into_iter().flatten().map(|rule|object(json!({"permission":rule["action"],"pattern":rule["resource"],"action":rule["effect"]}))).collect()
}
