//! Purpose: materialize built-in build agent and ordered configuration agent overlays.
//! Scope: Location agent registry setup from already-decoded config documents.
//! Uses:
//! rust/crates/
//! |-- config/src/lib.rs
//! |-- schema/src/agent.rs
//! |-- schema/src/ids.rs
//! |-- schema/src/permission.rs
//! |-- schema/src/provider.rs
//! `-- session-runtime/src/agents.rs
//! External:
//! |-- serde_json::Value
//! |-- serde_json::from_value
//! |-- serde_json::json
//! `-- serde_json::to_value
//! Used by:
//! rust/crates/
//! |-- runtime/src/location.rs
//! `-- runtime/src/location_tools.rs
//! Invariants: document order is preserved; disabled agents are removed; permissions append.
//! Contract tests: runtime/tests/production.rs

use opencode_config::CurrentConfig;
use opencode_schema::agent::{ID, Mode};
use opencode_schema::provider::ModelRef;
use opencode_session_runtime::agents::Agents;
use serde_json::{Value, json};

pub fn build(
    config: &CurrentConfig,
    home: &std::path::Path,
    data: &std::path::Path,
    temp: &std::path::Path,
) -> Agents {
    let agents = Agents::new();
    let documents: Vec<Value> = config.documents().map(|d| d.info.clone()).collect();
    let home = home.display().to_string();
    let defaults = json!([
        {"action":"*","resource":"*","effect":"allow"},
        {"action":"external_directory","resource":"*","effect":"ask"},
        {"action":"external_directory","resource":format!("{}/*", data.join("tool-output").display()),"effect":"allow"},
        {"action":"external_directory","resource":format!("{}/*", temp.display()),"effect":"allow"},
        {"action":"read","resource":"*.env","effect":"ask"},
        {"action":"read","resource":"*.env.*","effect":"ask"},
        {"action":"read","resource":"*.env.example","effect":"allow"}
    ]);
    agents.transform(move |draft| {
        let global: Vec<opencode_schema::permission::Rule> = documents
            .iter()
            .flat_map(|d| rules(d.get("permissions"), &home))
            .collect();
        draft.update(ID::new("build"), |agent| {
            agent.mode = Mode::Primary;
            agent.permissions = rules(Some(&defaults), &home);
            agent.permissions.extend(global.clone());
        });
        for document in &documents {
            if let Some(default) = document.get("default_agent").and_then(Value::as_str) {
                draft.default(Some(ID::new(default)));
            }
            for (id, item) in document
                .get("agents")
                .and_then(Value::as_object)
                .into_iter()
                .flatten()
            {
                if item.get("disabled").and_then(Value::as_bool) == Some(true) {
                    draft.remove(&ID::new(id));
                    continue;
                }
                let exists = draft.get(&ID::new(id)).is_some();
                draft.update(ID::new(id), |agent| {
                    if !exists {
                        agent.permissions.extend(global.clone());
                    }
                    if let Some(model) = item
                        .get("model")
                        .and_then(Value::as_str)
                        .and_then(|v| v.split_once('/'))
                    {
                        agent.model = Some(ModelRef {
                            provider_id: opencode_schema::ids::ProviderID::new(model.0),
                            id: opencode_schema::ids::ModelID::new(model.1),
                            variant: agent.model.as_ref().and_then(|m| m.variant.clone()),
                        });
                    }
                    if let Some(variant) = item.get("variant").and_then(Value::as_str)
                        && let Some(model) = &mut agent.model
                    {
                        model.variant = Some(opencode_schema::ids::VariantID::new(variant));
                    }
                    if let Some(request) = item.get("request") {
                        if let Some(headers) = request.get("headers").and_then(Value::as_object) {
                            agent
                                .request
                                .headers
                                .extend(headers.iter().filter_map(|(k, v)| {
                                    v.as_str().map(|v| (k.clone(), v.to_owned()))
                                }));
                        }
                        if let Some(body) = request.get("body").and_then(Value::as_object) {
                            agent.request.body.extend(body.clone());
                        }
                    }
                    let mut value = serde_json::to_value(&*agent).expect("agent serializes");
                    for key in ["system", "description", "mode", "hidden", "color", "steps"] {
                        if let Some(field) = item.get(key) {
                            value[key] = field.clone();
                        }
                    }
                    if let Ok(updated) = serde_json::from_value(value) {
                        *agent = updated;
                    }
                    agent
                        .permissions
                        .extend(rules(item.get("permissions"), &home));
                });
            }
        }
    });
    agents
}

fn rules(value: Option<&Value>, home: &str) -> Vec<opencode_schema::permission::Rule> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|value| {
            serde_json::from_value::<opencode_schema::permission::Rule>(value.clone()).ok()
        })
        .map(|mut rule| {
            if ["external_directory", "read", "edit"].contains(&rule.action.as_str()) {
                if rule.resource == "~" || rule.resource == "$HOME" {
                    rule.resource = home.into();
                } else if let Some(tail) = rule.resource.strip_prefix("~/") {
                    rule.resource = format!("{home}/{tail}");
                } else if let Some(tail) = rule.resource.strip_prefix("$HOME/") {
                    rule.resource = format!("{home}/{tail}");
                }
            }
            rule
        })
        .collect()
}
