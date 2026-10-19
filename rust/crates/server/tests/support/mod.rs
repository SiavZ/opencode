//! Purpose: shared harness for server socket tests: in-memory port fakes (test fixtures, not
//! adapters) and a real listener bound on 127.0.0.1:0.
//! Scope: test-only.
//! Uses:
//! rust/crates/
//! |-- server/src/app.rs
//! |-- server/src/lifecycle.rs
//! |-- server/src/services/mod.rs
//! |-- server/tests/support/replay/mod.rs
//! |-- session-api/src/admission.rs
//! |-- session-api/src/error.rs
//! |-- session-api/src/ids.rs
//! |-- session-api/src/info.rs
//! |-- session-api/src/message.rs
//! `-- session-api/src/operations.rs
//! External:
//! |-- async_trait::async_trait
//! |-- axum::Router
//! |-- lifecycle::serve
//! |-- reqwest::Client
//! |-- reqwest::Client::builder
//! |-- serde_json::from_value
//! |-- serde_json::json
//! `-- tokio::net::TcpListener::bind
//! Used by:
//! rust/crates/
//! |-- server/tests/http_auth.rs
//! |-- server/tests/http_cors.rs
//! |-- server/tests/http_errors.rs
//! |-- server/tests/http_lifecycle.rs
//! |-- server/tests/http_proxy.rs
//! |-- server/tests/http_session.rs
//! |-- server/tests/replay_baseline.rs
//! |-- server/tests/sse_events.rs
//! `-- server/tests/ws_pty.rs
//! Invariants: fakes record calls and return canned values; they never stand in for production.
//! Contract tests: this harness.
#![allow(dead_code)]

pub mod replay;

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::Router;
use opencode_server::lifecycle::{self, Listener};
use opencode_server::services::{
    LocationInfo, LocationRef, LocationResolver, PortError, SessionPlacement,
};
use opencode_session_api::admission::{Admitted, PromptRequest};
use opencode_session_api::error::{SessionError, SessionResult, UnavailableOperation};
use opencode_session_api::ids::{MessageID, ModelRef, SessionID};
use opencode_session_api::info::{RevertState, SessionInfo};
use opencode_session_api::message::Message;
use opencode_session_api::operations::{
    CreateRequest, HistoryPage, ListRequest, MessagesRequest, RevertStageRequest, SessionService,
};
use serde_json::json;

pub async fn start(router: Router) -> Listener {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    lifecycle::serve(listener, address, "127.0.0.1".to_string(), router)
}

pub fn client() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}

/// Location fixture: echoes the reference with a fixed project.
pub struct EchoLocations;

#[async_trait]
impl LocationResolver for EchoLocations {
    async fn resolve(&self, location: &LocationRef) -> Result<LocationInfo, PortError> {
        Ok(serde_json::from_value(json!({
            "directory": location.directory,
            "workspaceID": location.workspace_id,
            "project": { "id": "global", "directory": "/" },
        }))
        .unwrap_or_else(|_| {
            serde_json::from_value(json!({
                "directory": location.directory,
                "project": { "id": "global", "directory": "/" },
            }))
            .unwrap()
        }))
    }
}

/// Placement fixture: only `ses_known` exists, at `/work`.
pub struct OnePlacement;

#[async_trait]
impl SessionPlacement for OnePlacement {
    async fn placement(
        &self,
        session_id: &str,
    ) -> Result<Option<(String, Option<String>)>, PortError> {
        Ok((session_id == "ses_known").then(|| ("/work".to_string(), None)))
    }
}

pub fn session_info(id: &str, created: i64) -> SessionInfo {
    serde_json::from_value(json!({
        "id": id,
        "projectID": "global",
        "cost": 0,
        "tokens": { "input": 0, "output": 0, "reasoning": 0, "cache": { "read": 0, "write": 0 } },
        "time": { "created": created, "updated": created },
        "title": "t",
        "location": { "directory": "/work" },
    }))
    .unwrap()
}

pub fn user_message(id: &str) -> Message {
    serde_json::from_value(
        json!({ "type": "user", "id": id, "time": { "created": 5 }, "text": "hi" }),
    )
    .unwrap()
}

/// SessionService fixture recording requests.
#[derive(Default)]
pub struct FakeSessions {
    pub lists: Mutex<Vec<ListRequest>>,
    pub prompts: Mutex<Vec<PromptRequest>>,
    pub messages: Mutex<Vec<MessagesRequest>>,
}

fn missing(id: &SessionID) -> SessionError {
    SessionError::NotFound {
        session_id: id.clone(),
    }
}

#[async_trait]
impl SessionService for FakeSessions {
    async fn list(&self, request: ListRequest) -> SessionResult<Vec<SessionInfo>> {
        self.lists.lock().unwrap().push(request);
        Ok(vec![session_info("ses_b", 20), session_info("ses_a", 10)])
    }
    async fn create(&self, request: CreateRequest) -> SessionResult<SessionInfo> {
        Ok(session_info(
            request.id.as_ref().map(|i| i.as_str()).unwrap_or("ses_new"),
            1,
        ))
    }
    async fn get(&self, session_id: &SessionID) -> SessionResult<SessionInfo> {
        if session_id.as_str() == "ses_known" {
            return Ok(session_info("ses_known", 1));
        }
        Err(missing(session_id))
    }
    async fn messages(&self, request: MessagesRequest) -> SessionResult<Vec<Message>> {
        let known = request.session_id.as_str() == "ses_known";
        self.messages.lock().unwrap().push(request.clone());
        if !known {
            return Err(missing(&request.session_id));
        }
        Ok(vec![user_message("msg_2"), user_message("msg_1")])
    }
    async fn message(
        &self,
        _: &SessionID,
        message_id: &MessageID,
    ) -> SessionResult<Option<Message>> {
        Ok((message_id.as_str() == "msg_1").then(|| user_message("msg_1")))
    }
    async fn context(&self, _: &SessionID) -> SessionResult<Vec<Message>> {
        Ok(Vec::new())
    }
    async fn history(&self, s: &SessionID, _: Option<i64>, _: usize) -> SessionResult<HistoryPage> {
        Err(missing(s))
    }
    async fn prompt(&self, request: PromptRequest) -> SessionResult<Admitted> {
        self.prompts.lock().unwrap().push(request.clone());
        if request
            .id
            .as_ref()
            .is_some_and(|i| i.as_str() == "msg_conflict")
        {
            return Err(SessionError::PromptConflict {
                session_id: request.session_id.clone(),
                message_id: request.id.clone().unwrap(),
            });
        }
        Ok(serde_json::from_value(json!({
            "admittedSeq": 1,
            "id": request.id.as_ref().map(|i| i.as_str()).unwrap_or("msg_gen"),
            "sessionID": request.session_id,
            "prompt": { "text": request.prompt.text },
            "delivery": "steer",
            "timeCreated": 7,
        }))
        .unwrap())
    }
    async fn switch_agent(&self, s: &SessionID, _: &str) -> SessionResult<()> {
        Err(missing(s))
    }
    async fn switch_model(&self, s: &SessionID, _: ModelRef) -> SessionResult<()> {
        Err(missing(s))
    }
    async fn shell(&self, _: &SessionID, _: &str) -> SessionResult<()> {
        Err(SessionError::OperationUnavailable {
            operation: UnavailableOperation::Shell,
        })
    }
    async fn skill(&self, _: &SessionID, _: &str) -> SessionResult<()> {
        Err(SessionError::OperationUnavailable {
            operation: UnavailableOperation::Skill,
        })
    }
    async fn compact(&self, _: &SessionID) -> SessionResult<()> {
        Err(SessionError::OperationUnavailable {
            operation: UnavailableOperation::Compact,
        })
    }
    async fn wait(&self, _: &SessionID) -> SessionResult<()> {
        Ok(())
    }
    async fn active(&self) -> BTreeSet<SessionID> {
        BTreeSet::from([SessionID::new("ses_run").unwrap()])
    }
    async fn resume(&self, _: &SessionID) -> SessionResult<()> {
        Ok(())
    }
    async fn interrupt(&self, _: &SessionID) {}
    async fn revert_stage(&self, r: RevertStageRequest) -> SessionResult<RevertState> {
        Err(missing(&r.session_id))
    }
    async fn revert_clear(&self, _: &SessionID) -> SessionResult<()> {
        Ok(())
    }
    async fn revert_commit(&self, _: &SessionID) -> SessionResult<()> {
        Ok(())
    }
}

pub fn services(sessions: Arc<FakeSessions>) -> opencode_server::services::Services {
    opencode_server::services::Services {
        legacy_sessions: None,
        legacy_global: None,
        legacy_native: None,
        legacy_integrations: None,
        legacy_providers: None,
        version: "test".into(),
        session: Some(sessions),
        locations: Some(Arc::new(EchoLocations)),
        placement: Some(Arc::new(OnePlacement)),
        workspaces: None,
        ui: None,
        streams: Default::default(),
    }
}

pub fn session_router(services: opencode_server::services::Services) -> Router {
    opencode_server::app::embedded_router(services)
}
