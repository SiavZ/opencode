//! Purpose: `SessionPrompt.command` (packages/opencode/src/session/prompt.ts:1356-1484): command
//! lookup with the not-found hint, `$N`/`$ARGUMENTS` templating, `` !`cmd` `` shell substitution
//! through the p05 `ProcessRunner`, task/user model and agent selection, subtask part construction,
//! the `command.execute.before` hook, the prompt and the `command.executed` event.
//! Scope: one command invocation.
//! Uses:
//! rust/crates/
//! |-- platform-api/src/process.rs
//! |-- session-runtime/src/legacy/mod.rs
//! |-- session-runtime/src/legacy/prompt.rs
//! |-- session-runtime/src/legacy/store.rs
//! |-- session-runtime/src/legacy/template.rs
//! `-- session-runtime/src/legacy/types.rs
//! External:
//! `-- serde_json::{Value, json}
//! Used by:
//! rust/crates/
//! |-- session-runtime/src/legacy/mod.rs
//! `-- session-runtime/tests/legacy_command.rs
//! Invariants: shell substitutions run concurrently and replace matches in order with stdout
//! (`nothrow`: failing commands still contribute their stdout). Template file parts already present
//! in `input.parts` (by file path) are dropped. Subtask commands send exactly one subtask part.
//! Contract tests: session-runtime/tests/legacy_command.rs

use opencode_platform_api::process::{Command, RunOptions};
use serde_json::{Value, json};

use super::LegacySession;
use super::store::{COMMAND_EXECUTED, object};
use super::template::{expand_command, parse_model, replace_shell, shell_references};
use super::types::{
    CommandInput, LegacyError, ModelRef, PromptInput, WithParts, str_field, unknown_error,
};

impl LegacySession {
    /// `SessionPrompt.command`.
    pub async fn command(&self, input: CommandInput) -> Result<WithParts, LegacyError> {
        tracing::info!(session.id = %input.session_id, command = %input.command, "command");
        let Some(cmd) = self.deps.commands.get(&input.command).await? else {
            let available: Vec<String> = self
                .deps
                .commands
                .list()
                .await?
                .into_iter()
                .map(|command| command.name)
                .collect();
            let hint = if available.is_empty() {
                String::new()
            } else {
                format!(" Available commands: {}", available.join(", "))
            };
            let message = format!("Command not found: \"{}\".{hint}", input.command);
            self.deps
                .store
                .publish_error(&input.session_id, unknown_error(&message))
                .await?;
            return Err(LegacyError::Unknown(message));
        };
        let agent_name = cmd.agent.clone().or_else(|| input.agent.clone());
        let mut template = expand_command(&cmd.template, &input.arguments);
        let shells = shell_references(&template);
        if !shells.is_empty() {
            let runs = shells.iter().map(|(_, command)| {
                let process = self.deps.process.clone();
                let shell = self.deps.config.shell.clone();
                let command = Command::new(shell).arg("-c").arg(command.clone());
                async move {
                    match process.run(&command, RunOptions::default()).await {
                        Ok(result) => String::from_utf8_lossy(&result.stdout).into_owned(),
                        Err(_) => String::new(),
                    }
                }
            });
            let outputs = futures::future::join_all(runs).await;
            template = replace_shell(&template, &outputs);
        }
        let template = template.trim().to_owned();

        let task_model = if let Some(model) = &cmd.model {
            let (provider_id, model_id) = parse_model(model);
            ModelRef::new(provider_id, model_id)
        } else if let Some(own) = match &cmd.agent {
            Some(agent) => self
                .deps
                .agents
                .get(agent)
                .await?
                .and_then(|agent| agent.model),
            None => None,
        } {
            own
        } else if let Some(model) = &input.model {
            let (provider_id, model_id) = parse_model(model);
            ModelRef::new(provider_id, model_id)
        } else {
            self.current_model(&input.session_id).await?
        };
        self.get_model(
            &task_model.provider_id,
            &task_model.model_id,
            &input.session_id,
        )
        .await?;

        let agent = self
            .require_agent(&input.session_id, agent_name.as_deref())
            .await?;
        let template_parts = self.resolve_prompt_parts(&template).await?;
        let input_files: std::collections::HashSet<String> = input
            .parts
            .iter()
            .flatten()
            .filter_map(|part| str_field(part, "url"))
            .filter_map(|url| url::Url::parse(url).ok())
            .filter(|url| url.scheme() == "file")
            .filter_map(|url| url.to_file_path().ok())
            .map(|path| path.to_string_lossy().into_owned())
            .collect();
        let is_subtask =
            (agent.mode == "subagent" && cmd.subtask != Some(false)) || cmd.subtask == Some(true);
        let parts = if is_subtask {
            let prompt = template_parts
                .iter()
                .find(|part| str_field(part, "type") == Some("text"))
                .and_then(|part| str_field(part, "text"))
                .unwrap_or_default();
            vec![object(json!({
                "type": "subtask",
                "agent": agent.name,
                "description": cmd.description.clone().unwrap_or_default(),
                "command": input.command,
                "model": { "providerID": task_model.provider_id, "modelID": task_model.model_id },
                "prompt": prompt,
            }))]
        } else {
            template_parts
                .into_iter()
                .filter(|part| {
                    str_field(part, "type") != Some("file")
                        || !str_field(part, "url")
                            .and_then(|url| url::Url::parse(url).ok())
                            .and_then(|url| url.to_file_path().ok())
                            .is_some_and(|path| input_files.contains(&*path.to_string_lossy()))
                })
                .chain(input.parts.iter().flatten().cloned())
                .collect()
        };
        let user_agent = if is_subtask {
            match &input.agent {
                Some(agent) => agent.clone(),
                None => self.deps.agents.default_info().await?.name,
            }
        } else {
            agent.name.clone()
        };
        let user_model = if is_subtask {
            match &input.model {
                Some(model) => {
                    let (provider_id, model_id) = parse_model(model);
                    ModelRef::new(provider_id, model_id)
                }
                None => self.current_model(&input.session_id).await?,
            }
        } else {
            task_model
        };
        let output = self
            .deps
            .tools
            .trigger(
                "command.execute.before",
                json!({
                    "command": input.command,
                    "sessionID": input.session_id,
                    "arguments": input.arguments,
                }),
                json!({ "parts": parts }),
            )
            .await?;
        let parts = output
            .get("parts")
            .and_then(Value::as_array)
            .map(|parts| parts.iter().filter_map(Value::as_object).cloned().collect())
            .unwrap_or(parts);
        let result = self
            .prompt(PromptInput {
                session_id: input.session_id.clone(),
                message_id: input.message_id.clone(),
                model: Some(user_model),
                agent: Some(user_agent),
                parts,
                variant: input.variant.clone(),
                ..PromptInput::default()
            })
            .await?;
        self.deps
            .store
            .publish(
                &COMMAND_EXECUTED,
                json!({
                    "name": input.command,
                    "sessionID": input.session_id,
                    "arguments": input.arguments,
                    "messageID": result.id(),
                }),
            )
            .await?;
        Ok(result)
    }
}
