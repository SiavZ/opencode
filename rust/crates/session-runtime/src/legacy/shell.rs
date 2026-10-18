//! Purpose: `SessionPrompt.shell/shellImpl` (packages/opencode/src/session/prompt.ts:451-592,
//! 1349-1354) and `Shell.args` (packages/core/src/shell.ts:166-203): durable user/assistant/tool
//! transcript for a user shell command, process execution through the p05 `ProcessRunner`
//! (TERM=dumb, `shell.env` hook, stdin closed, 3 s force-kill grace), abort marker and completion.
//! Scope: one shell command under `RunState::start_shell`.
//! Uses:
//! rust/crates/
//! |-- platform-api/src/process.rs
//! |-- session-runtime/src/legacy/mod.rs
//! |-- session-runtime/src/legacy/run_state.rs
//! |-- session-runtime/src/legacy/store.rs
//! `-- session-runtime/src/legacy/types.rs
//! External:
//! |-- futures::FutureExt
//! |-- serde_json::{Value, json}
//! `-- tokio_util::sync::CancellationToken
//! Used by:
//! rust/crates/
//! |-- session-runtime/src/legacy/mod.rs
//! `-- session-runtime/tests/legacy_shell.rs
//! Invariants: the user message, synthetic text part, assistant message and running tool part are
//! durable before the ready latch opens (so cancel never races the transcript). Completion always
//! writes `time.completed` and the completed tool state, also after cancellation, with the
//! `<metadata>User aborted the command</metadata>` suffix. A busy Session rejects with `Busy`.
//! Contract tests: session-runtime/tests/legacy_shell.rs

use std::time::Duration;

use futures::FutureExt;
use opencode_platform_api::process::{Command, ProcessError, RunOptions};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::LegacySession;
use super::run_state::Ready;
use super::store::object;
use super::types::{LegacyError, Object, ShellInput, WithParts, message_id, now_millis, part_id};

/// `ShellID.ToolID`.
pub const SHELL_TOOL_ID: &str = "bash";

/// `Shell.name(file)` on unix: lowercased basename.
fn shell_name(file: &str) -> String {
    std::path::Path::new(file)
        .file_name()
        .map(|name| name.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

/// `Shell.args(file, command, cwd)`.
pub fn shell_args(file: &str, command: &str, cwd: &str) -> Vec<String> {
    let name = shell_name(file);
    let quoted = serde_json::to_string(command).unwrap_or_default();
    match name.as_str() {
        "nu" | "fish" => vec!["-c".into(), command.into()],
        "zsh" => vec![
            "-l".into(),
            "-c".into(),
            format!(
                "\n        [[ -f ~/.zshenv ]] && source ~/.zshenv >/dev/null 2>&1 || true\n        [[ -f \"${{ZDOTDIR:-$HOME}}/.zshrc\" ]] && source \"${{ZDOTDIR:-$HOME}}/.zshrc\" >/dev/null 2>&1 || true\n        cd -- \"$1\"\n        eval {quoted}\n      "
            ),
            "opencode".into(),
            cwd.into(),
        ],
        "bash" => vec![
            "-l".into(),
            "-c".into(),
            format!(
                "\n        shopt -s expand_aliases\n        [[ -f ~/.bashrc ]] && source ~/.bashrc >/dev/null 2>&1 || true\n        cd -- \"$1\"\n        eval {quoted}\n      "
            ),
            "opencode".into(),
            cwd.into(),
        ],
        "cmd" => vec!["/c".into(), command.into()],
        "powershell" | "pwsh" => vec!["-NoProfile".into(), "-Command".into(), command.into()],
        _ => vec!["-c".into(), command.into()],
    }
}

impl LegacySession {
    /// `SessionPrompt.shell`: fails with `Busy` while a run or shell is active.
    pub async fn shell(&self, input: ShellInput) -> Result<WithParts, LegacyError> {
        let this = self.clone();
        let session = input.session_id.clone();
        self.deps
            .run_state
            .start_shell(
                &session,
                self.last_assistant_fallback(&session),
                Box::new(move |cancel, ready| this.shell_impl(input, cancel, ready).boxed()),
            )
            .await
    }

    async fn shell_transcript(&self, input: &ShellInput) -> Result<(Object, Object), LegacyError> {
        let session = self.deps.store.get(&input.session_id).await?;
        if session
            .get("revert")
            .is_some_and(|revert| !revert.is_null())
        {
            self.cleanup_revert(&session).await?;
        }
        let agent = self
            .require_agent(&input.session_id, Some(&input.agent))
            .await?;
        let model = match (&input.model, &agent.model) {
            (Some(model), _) | (None, Some(model)) => model.clone(),
            (None, None) => self.current_model(&input.session_id).await?,
        };
        let user_id = input.message_id.clone().unwrap_or_else(message_id);
        let user = object(json!({
            "id": user_id,
            "sessionID": input.session_id,
            "time": { "created": now_millis() },
            "role": "user",
            "agent": input.agent,
            "model": { "providerID": model.provider_id, "modelID": model.model_id },
        }));
        self.deps.store.update_message(&user).await?;
        self.deps
            .store
            .update_part(&object(json!({
                "type": "text",
                "id": part_id(),
                "messageID": user_id,
                "sessionID": input.session_id,
                "text": "The following tool was executed by the user",
                "synthetic": true,
            })))
            .await?;
        let instance = &self.deps.instance;
        let message = object(json!({
            "id": message_id(),
            "sessionID": input.session_id,
            "parentID": user_id,
            "mode": input.agent,
            "agent": input.agent,
            "cost": 0,
            "path": {
                "cwd": instance.directory.to_string_lossy(),
                "root": instance.worktree.to_string_lossy(),
            },
            "time": { "created": now_millis() },
            "role": "assistant",
            "tokens": { "input": 0, "output": 0, "reasoning": 0, "cache": { "read": 0, "write": 0 } },
            "modelID": model.model_id,
            "providerID": model.provider_id,
        }));
        self.deps.store.update_message(&message).await?;
        let part = object(json!({
            "type": "tool",
            "id": part_id(),
            "messageID": message["id"],
            "sessionID": input.session_id,
            "tool": SHELL_TOOL_ID,
            "callID": opencode_schema::identifier::create("call", opencode_schema::identifier::Direction::Ascending, None),
            "state": {
                "status": "running",
                "time": { "start": now_millis() },
                "input": { "command": input.command },
            },
        }));
        self.deps.store.update_part(&part).await?;
        Ok((message, part))
    }

    async fn shell_impl(
        self,
        input: ShellInput,
        cancel: CancellationToken,
        ready: Ready,
    ) -> Result<WithParts, LegacyError> {
        let transcript = self.shell_transcript(&input).await;
        ready.open();
        let (mut message, mut part) = transcript?;
        let cwd = self.deps.instance.directory.to_string_lossy().into_owned();
        let sh = self.deps.config.shell.clone();
        let env = self
            .deps
            .tools
            .trigger(
                "shell.env",
                json!({ "cwd": cwd, "sessionID": input.session_id, "callID": part["callID"] }),
                json!({ "env": {} }),
            )
            .await
            .map(|output| output.get("env").cloned().unwrap_or(Value::Null))
            .unwrap_or(Value::Null);
        let mut command = Command::new(sh.clone())
            .args(shell_args(&sh, &input.command, &cwd))
            .cwd(&cwd);
        for (key, value) in env.as_object().into_iter().flatten() {
            if let Some(value) = value.as_str() {
                command = command.env(key, value);
            }
        }
        command = command.env("TERM", "dumb");
        let result = self
            .deps
            .process
            .run(
                &command,
                RunOptions {
                    combine_output: true,
                    cancel: Some(cancel.clone()),
                    stdin: Some(Vec::new()),
                    kill_grace: Some(Duration::from_secs(3)),
                    ..RunOptions::default()
                },
            )
            .await;
        let (mut output, aborted, failure) = match result {
            Ok(result) => (
                String::from_utf8_lossy(&result.output.unwrap_or(result.stdout)).into_owned(),
                cancel.is_cancelled(),
                None,
            ),
            Err(ProcessError::Cancelled { .. }) => (String::new(), true, None),
            Err(error) if cancel.is_cancelled() => (String::new(), true, Some(error)),
            Err(error) => (String::new(), false, Some(error)),
        };
        if aborted {
            output.push_str("\n\n<metadata>\nUser aborted the command\n</metadata>");
        }
        let completed = now_millis();
        if message["time"].get("completed").is_none() {
            message["time"]["completed"] = json!(completed);
            self.deps.store.update_message(&message).await?;
        }
        if part["state"]["status"] == "running" {
            let start = part["state"]["time"]["start"].clone();
            let input_value = part["state"]["input"].clone();
            part["state"] = json!({
                "status": "completed",
                "time": { "start": start, "end": completed },
                "input": input_value,
                "title": "",
                "metadata": { "output": output },
                "output": output,
            });
            self.deps.store.update_part(&part).await?;
        }
        if let (Some(error), false) = (failure, aborted) {
            return Err(LegacyError::Process(error.to_string()));
        }
        Ok(WithParts {
            info: message,
            parts: vec![part],
        })
    }
}
