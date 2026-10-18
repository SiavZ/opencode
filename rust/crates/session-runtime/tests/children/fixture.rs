//! Purpose: shared harness for `tests/children*.rs`: `FixtureOps` (fixture-only stand-in for the
//! AgentV2 lookup, `task` permission ask, tool metadata and legacy SessionPrompt ops) plus a real
//! migrated SQLite database with the Session projectors and a `ComposedChildHost` over it.
//! Scope: test-only.
//! Uses:
//! rust/crates/
//! |-- session-runtime/src/background.rs
//! |-- session-runtime/src/children.rs
//! |-- session-runtime/tests/support/sql.rs
//! `-- storage-api/src/lib.rs
//! External:
//! |-- async_trait::async_trait
//! |-- serde_json::{json, Map, Value}
//! `-- tokio::sync::Semaphore
//! Used by:
//! rust/crates/
//! |-- session-runtime/tests/children.rs
//! `-- session-runtime/tests/children_background.rs
//! Invariants: only the `ChildOps` seams are fixtures. Session rows, child creation and calling
//! message reads go through the production `LegacySessions` adapter on real SQLite.
//! Contract tests: session-runtime/tests/children.rs
#![allow(dead_code)]

use super::sql;

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use opencode_session_runtime::background::BackgroundJobs;
use opencode_session_runtime::children::{
    AgentInfo, ChildError, ChildLocation, ChildOps, ChildReply, Children, ComposedChildHost,
    LegacySessions, ModelSel, PermissionRule, TaskInput, TaskOutput, TaskRun, TaskSettings,
    UNWIRED_PROMPT,
};
use opencode_storage_api::{DatabaseExt, Sql, TxOptions, Value};
use serde_json::{Map, Value as Json, json};
use tokio::sync::Semaphore;

pub const PARENT: &str = "ses_parent";
pub const ASSISTANT: &str = "msg_assistant";

/// `(session, agent, variant, text)` of one synthetic parent prompt.
pub type ParentPrompt = (String, String, Option<String>, String);

/// Fixture for the seams owned outside this node. Records every call.
#[derive(Default)]
pub struct FixtureOps {
    pub agents: HashMap<String, AgentInfo>,
    pub deny_ask: bool,
    pub unwired_prompt: bool,
    /// When set, each child prompt waits for one permit.
    pub gate: Option<Arc<Semaphore>>,
    pub replies: Mutex<VecDeque<ChildReply>>,
    pub asks: Mutex<Vec<(String, String)>>,
    pub metadata: Mutex<Vec<Map<String, Json>>>,
    pub prompts: Mutex<Vec<TaskRun>>,
    pub cancels: Mutex<Vec<String>>,
    pub parents: Mutex<Vec<ParentPrompt>>,
}

#[async_trait]
impl ChildOps for FixtureOps {
    async fn agent(&self, name: &str) -> Result<Option<AgentInfo>, ChildError> {
        Ok(self.agents.get(name).cloned())
    }
    async fn ask_permission(
        &self,
        _: &TaskInput,
        permission: &str,
        pattern: &str,
    ) -> Result<(), ChildError> {
        self.asks
            .lock()
            .unwrap()
            .push((permission.into(), pattern.into()));
        match self.deny_ask {
            true => Err(ChildError::PermissionDenied("rejected".into())),
            false => Ok(()),
        }
    }
    async fn tool_metadata(&self, _: &TaskInput, _: &str, metadata: &Map<String, Json>) {
        self.metadata.lock().unwrap().push(metadata.clone());
    }
    fn prompt_ready(&self) -> Result<(), ChildError> {
        match self.unwired_prompt {
            true => Err(ChildError::NotYetPorted(UNWIRED_PROMPT)),
            false => Ok(()),
        }
    }
    async fn prompt(&self, run: &TaskRun) -> Result<ChildReply, ChildError> {
        self.prompts.lock().unwrap().push(run.clone());
        if let Some(gate) = &self.gate {
            gate.acquire().await.unwrap().forget();
        }
        Ok(self
            .replies
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(ChildReply {
                text: Some(format!("done: {}", run.prompt)),
                ..ChildReply::default()
            }))
    }
    async fn cancel(&self, session_id: &str) {
        self.cancels.lock().unwrap().push(session_id.into());
    }
    async fn prompt_parent(
        &self,
        session_id: &str,
        agent: &str,
        variant: Option<String>,
        text: String,
    ) -> Result<(), ChildError> {
        self.parents
            .lock()
            .unwrap()
            .push((session_id.into(), agent.into(), variant, text));
        Ok(())
    }
}

pub struct Ctx {
    pub h: sql::Harness,
    pub ops: Arc<FixtureOps>,
    pub children: Children,
}

pub fn rule(permission: &str, pattern: &str, action: &str) -> PermissionRule {
    PermissionRule {
        permission: permission.into(),
        pattern: pattern.into(),
        action: action.into(),
    }
}

pub fn agent(name: &str, permission: Vec<PermissionRule>, model: Option<ModelSel>) -> AgentInfo {
    AgentInfo {
        name: name.into(),
        permission,
        model,
    }
}

pub fn model(provider: &str, id: &str) -> ModelSel {
    ModelSel {
        model_id: id.into(),
        provider_id: provider.into(),
    }
}

pub fn default_ops() -> FixtureOps {
    FixtureOps {
        agents: HashMap::from([
            ("general".to_owned(), agent("general", vec![], None)),
            (
                "pinned".to_owned(),
                agent(
                    "pinned",
                    vec![rule("todowrite", "*", "allow")],
                    Some(model("anthropic", "claude-pinned")),
                ),
            ),
        ]),
        ..FixtureOps::default()
    }
}

pub async fn setup(ops: FixtureOps) -> Ctx {
    let h = sql::open().await;
    h.seed_session(PARENT).await;
    let ops = Arc::new(ops);
    let sessions = LegacySessions::new(
        h.events.clone(),
        ChildLocation {
            project_id: "global".into(),
            directory: "/project".into(),
            path: Some("".into()),
            workspace_id: None,
            version: "test".into(),
        },
    );
    let host = Arc::new(ComposedChildHost::new(sessions, ops.clone()));
    let children = Children::new(host, BackgroundJobs::new());
    let ctx = Ctx { h, ops, children };
    ctx.exec(
        "UPDATE session SET agent = 'build', permission = ? WHERE id = ?",
        vec![
            json!([
                {"permission": "bash", "pattern": "*", "action": "allow"},
                {"permission": "edit", "pattern": "secret/*", "action": "deny"},
                {"permission": "external_directory", "pattern": "/tmp/*", "action": "allow"},
                {"permission": "todowrite", "pattern": "*", "action": "deny"}
            ])
            .to_string()
            .into(),
            PARENT.into(),
        ],
    )
    .await;
    ctx.assistant(PARENT, ASSISTANT, Some("high")).await;
    ctx
}

impl Ctx {
    pub async fn exec(&self, statement: &str, params: Vec<Value>) {
        let statement = statement.to_owned();
        self.h
            .db
            .transaction(TxOptions::default(), move |tx: &dyn Sql| {
                tx.execute(&statement, &params)
            })
            .await
            .unwrap();
    }

    pub async fn assistant(&self, session: &str, id: &str, variant: Option<&str>) {
        let mut data = json!({"role": "assistant", "modelID": "gpt-call", "providerID": "openai"});
        if let Some(variant) = variant {
            data["variant"] = variant.into();
        }
        self.exec(
            "INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES (?, ?, 1, 1, ?)",
            vec![id.into(), session.into(), data.to_string().into()],
        )
        .await;
    }

    pub async fn row(&self, id: &str) -> (Option<String>, Option<String>, String, Option<String>) {
        let row = self
            .h
            .query(
                "SELECT parent_id, agent, title, permission FROM session WHERE id = ?",
                vec![id.into()],
            )
            .await
            .remove(0);
        (
            row.get("parent_id").unwrap(),
            row.get("agent").unwrap(),
            row.get("title").unwrap(),
            row.get("permission").unwrap(),
        )
    }

    pub async fn sessions(&self) -> i64 {
        self.h.count("SELECT count(*) FROM session", vec![]).await
    }

    pub async fn parents(&self, count: usize) -> Vec<ParentPrompt> {
        for _ in 0..500 {
            let parents = self.ops.parents.lock().unwrap().clone();
            if parents.len() >= count {
                return parents;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        self.ops.parents.lock().unwrap().clone()
    }
}

pub fn input(subagent: &str) -> TaskInput {
    TaskInput {
        session_id: PARENT.into(),
        message_id: ASSISTANT.into(),
        agent: "build".into(),
        description: "Find files".into(),
        prompt: "look around".into(),
        subagent_type: subagent.into(),
        ..TaskInput::default()
    }
}

pub fn settings() -> TaskSettings {
    TaskSettings {
        subagent_depth: None,
        primary_tools: vec![],
        background_subagents: true,
    }
}

pub fn child_of(output: &TaskOutput) -> String {
    output.metadata["sessionId"].as_str().unwrap().to_owned()
}
