//! Purpose: real TUI event publication and FIFO control request/response queues.
//! Scope: one process-global adapter with explicitly exposed host control methods.
//! Uses:
//! rust/crates/
//! |-- session-runtime/src/legacy/store.rs
//! `-- runtime/src/global_services.rs
//! External:
//! `-- tokio::sync
//! Used by:
//! rust/crates/
//! `-- runtime/src/global_services.rs
//! Invariants: control next waits for an actual request; no fabricated queue responses.
//! Contract tests: runtime/tests/global_services.rs

use super::{GlobalServices, LegacyError, RouteContext, Value, internal, json, required_payload};
use tokio::sync::{Mutex, mpsc};

pub struct TuiControl {
    request: mpsc::UnboundedSender<Value>,
    requests: Mutex<mpsc::UnboundedReceiver<Value>>,
    response: mpsc::UnboundedSender<Value>,
    responses: Mutex<mpsc::UnboundedReceiver<Value>>,
}

impl Default for TuiControl {
    fn default() -> Self {
        let (request, requests) = mpsc::unbounded_channel();
        let (response, responses) = mpsc::unbounded_channel();
        Self {
            request,
            requests: Mutex::new(requests),
            response,
            responses: Mutex::new(responses),
        }
    }
}

impl TuiControl {
    pub fn submit_request(&self, path: String, body: Value) -> Result<(), LegacyError> {
        self.request
            .send(json!({"path":path,"body":body}))
            .map_err(internal)
    }

    pub async fn next_response(&self) -> Result<Value, LegacyError> {
        self.responses
            .lock()
            .await
            .recv()
            .await
            .ok_or_else(|| LegacyError::Unavailable("TUI response queue closed".into()))
    }
}

impl GlobalServices {
    pub fn tui_control(&self) -> &TuiControl {
        &self.tui
    }

    pub(super) async fn tui(
        &self,
        operation: &str,
        context: &RouteContext,
        payload: Option<Value>,
    ) -> Result<Value, LegacyError> {
        if operation == "tui.control.next" {
            return self
                .tui
                .requests
                .lock()
                .await
                .recv()
                .await
                .ok_or_else(|| LegacyError::Unavailable("TUI request queue closed".into()));
        }
        if operation == "tui.control.response" {
            self.tui
                .response
                .send(payload.unwrap_or(Value::Null))
                .map_err(internal)?;
            return Ok(json!(true));
        }
        let (event, data) = match operation {
            "tui.appendPrompt" => ("tui.prompt.append", required_payload(payload)?),
            "tui.showToast" => ("tui.toast.show", required_payload(payload)?),
            "tui.selectSession" => {
                let payload = required_payload(payload)?;
                let session = payload
                    .get("sessionID")
                    .and_then(Value::as_str)
                    .filter(|id| id.starts_with("ses"))
                    .ok_or_else(|| LegacyError::BadRequest("Invalid Session ID".into()))?;
                let events = self
                    .events
                    .as_ref()
                    .ok_or_else(|| LegacyError::Unavailable("Event store not composed".into()))?;
                let store =
                    opencode_session_runtime::legacy::store::LegacyStore::new(std::sync::Arc::new(
                        opencode_session_runtime::projection::LocalSessionEvents::new(
                            self.database.clone(),
                            events.clone(),
                        ),
                    ));
                store
                    .get(session)
                    .await
                    .map_err(|error| LegacyError::Response {
                        status: 404,
                        body: json!({"name":"NotFoundError","data":{"message":error.to_string()}}),
                    })?;
                ("tui.session.select", payload)
            }
            "tui.publish" => {
                let payload = required_payload(payload)?;
                let event = match payload.get("type").and_then(Value::as_str) {
                    Some("tui.prompt.append") => "tui.prompt.append",
                    Some("tui.command.execute") => "tui.command.execute",
                    Some("tui.toast.show") => "tui.toast.show",
                    Some("tui.session.select") => "tui.session.select",
                    _ => return Err(LegacyError::BadRequest("Invalid TUI event type".into())),
                };
                (
                    event,
                    payload.get("properties").cloned().unwrap_or(Value::Null),
                )
            }
            _ => {
                let command = match operation {
                    "tui.openHelp" => Some("help.show"),
                    "tui.openSessions" | "tui.openThemes" => Some("session.list"),
                    "tui.openModels" => Some("model.list"),
                    "tui.submitPrompt" => Some("prompt.submit"),
                    "tui.clearPrompt" => Some("prompt.clear"),
                    "tui.executeCommand" => match required_payload(payload)?
                        .get("command")
                        .and_then(Value::as_str)
                    {
                        Some("session_new") => Some("session.new"),
                        Some("session_share") => Some("session.share"),
                        Some("session_interrupt") => Some("session.interrupt"),
                        Some("session_compact") => Some("session.compact"),
                        Some("messages_page_up") => Some("session.page.up"),
                        Some("messages_page_down") => Some("session.page.down"),
                        Some("messages_line_up") => Some("session.line.up"),
                        Some("messages_line_down") => Some("session.line.down"),
                        Some("messages_half_page_up") => Some("session.half.page.up"),
                        Some("messages_half_page_down") => Some("session.half.page.down"),
                        Some("messages_first") => Some("session.first"),
                        Some("messages_last") => Some("session.last"),
                        Some("agent_cycle") => Some("agent.cycle"),
                        _ => None,
                    },
                    _ => {
                        return Err(LegacyError::BadRequest(format!(
                            "Unknown TUI operation: {operation}"
                        )));
                    }
                };
                (
                    "tui.command.execute",
                    command
                        .map(|command| json!({"command":command}))
                        .unwrap_or(json!({})),
                )
            }
        };
        self.publish(event, &context.directory, data).await?;
        Ok(json!(true))
    }
}
