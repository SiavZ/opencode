//! Purpose: load agent-filtered skill/reference guidance and live MCP instructions with built-in context.
//! Scope: Location SystemContextLoader implementation.
//! Uses:
//! rust/crates/
//! |-- context/src/source/mod.rs
//! |-- runtime/src/location_integrations.rs
//! `-- session-runtime/src/runner/ports.rs
//! External:
//! |-- async_trait::async_trait
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! `-- runtime/src/location_integrations.rs
//! Invariants: denied skills are absent; reference descriptions and source removal messages match baseline.
//! Contract tests: runtime/tests/location_integrations.rs

use super::LocationIntegrations;
use async_trait::async_trait;
use opencode_context::{
    registry::Registry,
    source::{ContextError, Key, Observation, Source, SystemContext, combine, make},
};
use opencode_integration_api::mcp::McpService;
use opencode_permissions::{Effect, Rule, evaluate};
use opencode_session_runtime::{agents::Selection, runner::ports::SystemContextLoader};
use serde_json::{Value, json};
use std::sync::Arc;

pub struct IntegrationContext {
    pub registry: Registry,
    pub integrations: Arc<LocationIntegrations>,
}

#[async_trait]
impl SystemContextLoader for IntegrationContext {
    async fn load(&self, selection: &Selection) -> Result<SystemContext, ContextError> {
        let mut contexts = vec![self.registry.load().await?];
        if let Some(agent) = &selection.info {
            let rules: Vec<_> = agent
                .permissions
                .iter()
                .map(|rule| {
                    Rule::new(
                        rule.action.clone(),
                        rule.resource.clone(),
                        match rule.effect {
                            opencode_schema::permission::Effect::Allow => Effect::Allow,
                            opencode_schema::permission::Effect::Deny => Effect::Deny,
                            opencode_schema::permission::Effect::Ask => Effect::Ask,
                        },
                    )
                })
                .collect();
            let mut skills: Vec<_> = self
                .integrations
                .resources
                .skills
                .iter()
                .filter(|skill| {
                    evaluate(
                        "skill",
                        skill["name"].as_str().unwrap_or_default(),
                        &[&rules],
                    )
                    .effect
                        != Effect::Deny
                })
                .filter(|skill| skill.get("description").is_some())
                .map(|skill| json!({"name":skill["name"],"description":skill["description"]}))
                .collect();
            skills.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
            if !skills.is_empty() || evaluate("skill", "*", &[&rules]).effect != Effect::Deny {
                contexts.push(source("core/skill-guidance", skills, skills_text,
                    "The available skills have changed. This list supersedes the previous available skills list.",
                    "Skill guidance is no longer available. Do not use any previously listed skill."));
            }
        }
        let mut references: Vec<_> = self.integrations.resources.references.iter().filter(|reference| reference.get("description").is_some()).map(|reference| json!({"name":reference["name"],"path":reference["path"],"description":reference["description"]})).collect();
        references.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
        if !references.is_empty() {
            contexts.push(source("core/reference-guidance", references, references_text,
                "The available project references have changed. This list supersedes the previous reference list.",
                "Project reference guidance is no longer available. Do not use previously listed references."));
        }
        let instructions = self.integrations.mcp.instructions().await;
        for instruction in instructions {
            let encoded: String = instruction
                .name
                .bytes()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            let key = format!("mcp/server-{encoded}/instructions");
            let text = instruction.instructions;
            contexts.push(make(Source::new(
                Key::parse(key).expect("MCP context key"),
                move || {
                    let text = text.clone();
                    async move { Ok(Observation::Available(text)) }
                },
                Clone::clone,
                |_, current| current.clone(),
            )));
        }
        combine(contexts)
    }
}

fn source(
    key: &str,
    value: Vec<Value>,
    render: fn(&[Value]) -> String,
    changed: &'static str,
    removed: &'static str,
) -> SystemContext {
    make(
        Source::new(
            Key::parse(key).expect("static context key"),
            move || {
                let value = value.clone();
                async move { Ok(Observation::Available(value)) }
            },
            move |value: &Vec<Value>| render(value),
            move |_, current| format!("{changed}\n{}", render(current)),
        )
        .with_removed(move |_| removed.into()),
    )
}

fn skills_text(skills: &[Value]) -> String {
    let mut lines = vec![
        "Skills provide specialized instructions and workflows for specific tasks.".into(),
        "Use the skill tool to load a skill when a task matches its description.".into(),
    ];
    if skills.is_empty() {
        lines.push("No skills are currently available.".into());
        return lines.join("\n");
    }
    lines.push("<available_skills>".into());
    for skill in skills {
        lines.extend([
            "  <skill>".into(),
            format!(
                "    <name>{}</name>",
                skill["name"].as_str().unwrap_or_default()
            ),
            format!(
                "    <description>{}</description>",
                skill["description"].as_str().unwrap_or_default()
            ),
            "  </skill>".into(),
        ]);
    }
    lines.push("</available_skills>".into());
    lines.join("\n")
}
fn references_text(references: &[Value]) -> String {
    let mut lines = vec![
        "Project references provide additional directories that can be accessed when relevant."
            .into(),
        "<available_references>".into(),
    ];
    for reference in references {
        lines.extend([
            "  <reference>".into(),
            format!(
                "    <name>{}</name>",
                reference["name"].as_str().unwrap_or_default()
            ),
            format!(
                "    <path>{}</path>",
                reference["path"].as_str().unwrap_or_default()
            ),
            format!(
                "    <description>{}</description>",
                reference["description"].as_str().unwrap_or_default()
            ),
            "  </reference>".into(),
        ]);
    }
    lines.push("</available_references>".into());
    lines.join("\n")
}
