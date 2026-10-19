//! Purpose: materialize legacy command catalog from builtins, configuration, MCP and skills.
//! Scope: one explicit Location lease with no synthetic remote prompt contents.
//! Uses:
//! rust/crates/
//! |-- integration-api/src/mcp.rs
//! `-- runtime/src/global_services.rs
//! External:
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! `-- runtime/src/global_services.rs
//! Invariants: overlays preserve builtin insertion order and skill commands never replace commands.
//! Contract tests: runtime/tests/global_services.rs

use super::{GlobalServices, LegacyError, RouteContext, Value, internal, json};
use opencode_integration_api::mcp::McpService;

impl GlobalServices {
    pub(super) async fn commands(&self, context: &RouteContext) -> Result<Value, LegacyError> {
        let project = self
            .projects
            .from_directory(&context.directory)
            .await
            .map_err(internal)?;
        let mut commands = Vec::new();
        for (name, description, template, subtask) in [
            (
                "init",
                "guided AGENTS.md setup",
                include_str!(
                    "../../../../../packages/opencode/src/command/template/initialize.txt"
                ),
                false,
            ),
            (
                "review",
                "review changes [commit|branch|pr], defaults to uncommitted",
                include_str!("../../../../../packages/opencode/src/command/template/review.txt"),
                true,
            ),
        ] {
            let mut command = json!({"name":name,"description":description,"source":"command","template":template.replacen("${path}",&project.sandbox,1),"hints":hints(template)});
            if subtask {
                command["subtask"] = json!(true);
            }
            commands.push(command);
        }
        let config = self.resolved_config(&context.directory).await?;
        let lease = self
            .locations
            .get(&self.location(context)?)
            .await
            .map_err(internal)?;
        let configured = config
            .get("command")
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|commands| {
                commands.iter().map(|(name, value)| {
                    let mut value = value.clone();
                    value["name"] = json!(name);
                    value
                })
            });
        for mut command in configured.chain(lease.integrations.resources.commands.iter().cloned()) {
            command["source"] = json!("command");
            command["hints"] = json!(hints(
                command
                    .get("template")
                    .and_then(Value::as_str)
                    .unwrap_or("")
            ));
            overlay(&mut commands, command);
        }
        for (name, prompt) in lease.integrations.mcp.prompts().await {
            // Legacy HTTP serializes the lazy Promise template as an empty object.
            let mut command = json!({"name":name,"source":"mcp","template":{},"hints":prompt.arguments.unwrap_or_default().iter().enumerate().map(|(index,_)|format!("${}",index+1)).collect::<Vec<_>>()});
            if let Some(description) = prompt.description {
                command["description"] = json!(description);
            }
            overlay(&mut commands, command);
        }
        for skill in &lease.integrations.resources.legacy_skills() {
            if commands
                .iter()
                .any(|command| command.get("name") == skill.get("name"))
            {
                continue;
            }
            let location = skill
                .get("location")
                .and_then(Value::as_str)
                .unwrap_or("<built-in>");
            let content = skill.get("content").and_then(Value::as_str).unwrap_or("");
            let template = if location == "<built-in>" {
                content.to_owned()
            } else {
                format!(
                    "{content}\n\nBase directory for this skill: {}\nRelative paths in this skill (e.g., scripts/, references/) are relative to this base directory.",
                    std::path::Path::new(location)
                        .parent()
                        .unwrap_or(std::path::Path::new(""))
                        .display()
                )
            };
            let mut command =
                json!({"name":skill["name"],"source":"skill","template":template,"hints":[]});
            if let Some(description) = skill.get("description") {
                command["description"] = description.clone();
            }
            commands.push(command);
        }
        Ok(Value::Array(commands))
    }
}

fn overlay(commands: &mut Vec<Value>, command: Value) {
    if let Some(existing) = commands
        .iter_mut()
        .find(|existing| existing.get("name") == command.get("name"))
    {
        *existing = command;
        return;
    }
    commands.push(command);
}

fn hints(template: &str) -> Vec<String> {
    let mut hints = regex::Regex::new(r"\$\d+")
        .expect("static expression")
        .find_iter(template)
        .map(|item| item.as_str().to_owned())
        .collect::<Vec<_>>();
    hints.sort();
    hints.dedup();
    if template.contains("$ARGUMENTS") {
        hints.push("$ARGUMENTS".into());
    }
    hints
}
