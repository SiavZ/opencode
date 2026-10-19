//! Purpose: V2 permission service (packages/core/src/permission.ts:93-310): configured agent
//! denial wins over saved grants, ask creates a pending request and publishes `permission.v2.asked`,
//! reply resolves once/always/reject with session-wide reject cascade and always-recheck.
//! Scope: in-memory pending registry per Location over injected policy, saved-grant and event ports.
//! Uses:
//! rust/crates/
//! |-- permissions/src/rule.rs
//! |-- permissions/src/service/pending.rs
//! `-- permissions/src/wildcard.rs
//! External:
//! |-- async_trait::async_trait
//! |-- futures::channel::oneshot
//! |-- oneshot::Receiver
//! |-- oneshot::channel
//! |-- serde::Deserialize
//! |-- serde::Serialize
//! |-- serde_json::Map
//! |-- serde_json::Value
//! `-- wildcard::matches
//! Used by:
//! rust/crates/
//! |-- permissions/src/lib.rs
//! `-- permissions/src/service/pending.rs
//! Invariants: a dropped/cancelled `assert` removes its pending request; `shutdown` declines every
//! pending request; duplicate pending IDs are rejected.
//! Contract tests: permissions/tests/service.rs

use crate::rule::{Effect, Rule, Ruleset, evaluate};
use crate::wildcard;
use async_trait::async_trait;
use futures::channel::oneshot;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::sync::{Arc, Mutex};

mod pending;
use pending::{Pending, PendingGuard, Settler, lock};

/// `PermissionV2.Source`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Source {
    Tool {
        #[serde(rename = "messageID")]
        message_id: String,
        #[serde(rename = "callID")]
        call_id: String,
    },
}

/// `PermissionV2.Request` wire shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PermissionRequest {
    pub id: String,
    #[serde(rename = "sessionID")]
    pub session_id: String,
    pub action: String,
    pub resources: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub save: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub metadata: Option<Map<String, Value>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub source: Option<Source>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Reply {
    Once,
    Always,
    Reject,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AssertInput {
    pub id: Option<String>,
    pub session_id: String,
    pub action: String,
    pub resources: Vec<String>,
    pub save: Option<Vec<String>>,
    pub metadata: Option<Map<String, Value>>,
    pub source: Option<Source>,
    pub agent: Option<String>,
}

impl AssertInput {
    pub fn new(
        session_id: impl Into<String>,
        action: impl Into<String>,
        resources: Vec<String>,
    ) -> Self {
        Self {
            id: None,
            session_id: session_id.into(),
            action: action.into(),
            resources,
            save: None,
            metadata: None,
            source: None,
            agent: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplyInput {
    #[serde(rename = "requestID")]
    pub request_id: String,
    pub reply: Reply,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskResult {
    pub id: String,
    pub effect: Effect,
}

/// `PermissionSaved.Info`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedGrant {
    pub id: String,
    #[serde(rename = "projectID")]
    pub project_id: String,
    pub action: String,
    pub resource: String,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum PermissionError {
    /// `PermissionV2.BlockedError`: denied by policy, carries the relevant rules.
    #[error("permission blocked by rules")]
    Blocked { rules: Ruleset },
    /// `PermissionV2.CorrectedError`: rejected with feedback.
    #[error("{feedback}")]
    Corrected { feedback: String },
    /// `PermissionV2.DeclinedError`: rejected without feedback, cascaded reject, or teardown.
    #[error("permission declined")]
    Declined,
    #[error("permission request not found: {request_id}")]
    NotFound { request_id: String },
    #[error("session not found: {session_id}")]
    SessionNotFound { session_id: String },
    #[error("duplicate pending permission ID: {id}")]
    Duplicate { id: String },
    #[error("permission port failed: {0}")]
    Port(String),
}

/// Events published by the service.
#[derive(Debug, Clone, PartialEq)]
pub enum PermissionEvent {
    /// `permission.v2.asked`.
    Asked(PermissionRequest),
    /// `permission.v2.replied`.
    Replied {
        session_id: String,
        request_id: String,
        reply: Reply,
    },
}

/// Configured (agent) rules for a session. `Ok(None)` means the agent could not be resolved, which
/// the baseline treats as deny-all (`missingAgentPermissions`).
#[async_trait]
pub trait PolicySource: Send + Sync {
    async fn configured(
        &self,
        session_id: &str,
        agent: Option<&str>,
    ) -> Result<Option<Ruleset>, PermissionError>;
}

/// Saved `always` grants, project scoped (packages/core/src/permission/saved.ts).
#[async_trait]
pub trait SavedGrants: Send + Sync {
    async fn list(&self, project_id: &str) -> Result<Vec<SavedGrant>, PermissionError>;
    /// Insert one row per resource, ignoring conflicts. Empty input is a no-op.
    async fn add(
        &self,
        project_id: &str,
        action: &str,
        resources: &[String],
    ) -> Result<(), PermissionError>;
}

#[async_trait]
pub trait PermissionEvents: Send + Sync {
    async fn publish(&self, event: PermissionEvent) -> Result<(), PermissionError>;
}

/// Location-scoped permission service.
pub struct PermissionService {
    project_id: String,
    policy: Arc<dyn PolicySource>,
    saved: Arc<dyn SavedGrants>,
    events: Arc<dyn PermissionEvents>,
    pending: Arc<Mutex<Vec<Pending>>>,
}

impl std::fmt::Debug for PermissionService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PermissionService")
            .field("project_id", &self.project_id)
            .finish_non_exhaustive()
    }
}

fn missing_agent_permissions() -> Ruleset {
    vec![Rule::new("*", "*", Effect::Deny)]
}

fn denied(action: &str, resources: &[String], rules: &[Rule]) -> bool {
    resources
        .iter()
        .any(|r| evaluate(action, r, &[rules]).effect == Effect::Deny)
}

impl PermissionService {
    pub fn new(
        project_id: impl Into<String>,
        policy: Arc<dyn PolicySource>,
        saved: Arc<dyn SavedGrants>,
        events: Arc<dyn PermissionEvents>,
    ) -> Self {
        Self {
            project_id: project_id.into(),
            policy,
            saved,
            events,
            pending: Arc::new(Mutex::new(Vec::new())),
        }
    }

    async fn configured(
        &self,
        session_id: &str,
        agent: Option<&str>,
    ) -> Result<Ruleset, PermissionError> {
        Ok(self
            .policy
            .configured(session_id, agent)
            .await?
            .unwrap_or_else(missing_agent_permissions))
    }

    async fn saved_rules(&self) -> Result<Ruleset, PermissionError> {
        Ok(self
            .saved
            .list(&self.project_id)
            .await?
            .into_iter()
            .map(|item| Rule::new(item.action, item.resource, Effect::Allow))
            .collect())
    }

    /// Effective decision: configured deny wins, otherwise configured followed by saved grants.
    pub async fn evaluate_input(
        &self,
        input: &AssertInput,
    ) -> Result<(Effect, Ruleset), PermissionError> {
        let rules = self
            .configured(&input.session_id, input.agent.as_deref())
            .await?;
        if denied(&input.action, &input.resources, &rules) {
            return Ok((Effect::Deny, rules));
        }
        let mut all = rules;
        all.extend(self.saved_rules().await?);
        let effects: Vec<Effect> = input
            .resources
            .iter()
            .map(|r| evaluate(&input.action, r, &[&all]).effect)
            .collect();
        let effect = if effects.contains(&Effect::Deny) {
            Effect::Deny
        } else if effects.contains(&Effect::Ask) {
            Effect::Ask
        } else {
            Effect::Allow
        };
        Ok((effect, all))
    }

    fn request(input: &AssertInput) -> PermissionRequest {
        PermissionRequest {
            id: input
                .id
                .clone()
                .unwrap_or_else(|| format!("per_{}", opencode_schema::identifier::ascending())),
            session_id: input.session_id.clone(),
            action: input.action.clone(),
            resources: input.resources.clone(),
            save: input.save.clone(),
            metadata: input.metadata.clone(),
            source: input.source.clone(),
        }
    }

    async fn create(
        &self,
        request: PermissionRequest,
        agent: Option<String>,
    ) -> Result<(oneshot::Receiver<Result<(), PermissionError>>, PendingGuard), PermissionError>
    {
        let (sender, receiver) = oneshot::channel();
        let owner = Arc::new(());
        {
            let mut pending = lock(&self.pending);
            if pending.iter().any(|item| item.request.id == request.id) {
                return Err(PermissionError::Duplicate { id: request.id });
            }
            pending.push(Pending {
                request: request.clone(),
                agent,
                sender: Some(sender),
                owner: owner.clone(),
            });
        }
        // Install cleanup only after insertion, but before the first cancellable await.
        // Identity rather than ID prevents an old assertion from removing a reused ID.
        let guard = PendingGuard {
            pending: self.pending.clone(),
            owner,
            armed: true,
        };
        self.events.publish(PermissionEvent::Asked(request)).await?;
        Ok((receiver, guard))
    }

    /// `ask`: evaluate, and when the effect is `ask` register a pending request without waiting.
    pub async fn ask(&self, input: AssertInput) -> Result<AskResult, PermissionError> {
        let (effect, _) = self.evaluate_input(&input).await?;
        let request = Self::request(&input);
        let id = request.id.clone();
        if effect == Effect::Ask {
            let (_, mut guard) = self.create(request, input.agent.clone()).await?;
            guard.armed = false;
        }
        Ok(AskResult { id, effect })
    }

    /// `assert`: allow returns, deny fails with relevant rules, ask waits for a reply.
    pub async fn assert(&self, input: AssertInput) -> Result<(), PermissionError> {
        let (effect, rules) = self.evaluate_input(&input).await?;
        match effect {
            Effect::Allow => Ok(()),
            Effect::Deny => Err(PermissionError::Blocked {
                rules: rules
                    .into_iter()
                    .filter(|rule| wildcard::matches(&input.action, &rule.action))
                    .collect(),
            }),
            Effect::Ask => {
                let request = Self::request(&input);
                let (receiver, guard) = self.create(request, input.agent.clone()).await?;
                let result = receiver.await.unwrap_or(Err(PermissionError::Declined));
                drop(guard);
                result
            }
        }
    }

    fn take(&self, id: &str) -> Option<(PermissionRequest, Option<Settler>)> {
        let mut pending = lock(&self.pending);
        let index = pending.iter().position(|item| item.request.id == id)?;
        let mut item = pending.remove(index);
        Some((item.request, item.sender.take()))
    }

    fn settle(sender: Option<Settler>, value: Result<(), PermissionError>) {
        if let Some(sender) = sender {
            drop(sender.send(value));
        }
    }

    pub async fn reply(&self, input: ReplyInput) -> Result<(), PermissionError> {
        let existing = lock(&self.pending)
            .iter()
            .find(|item| item.request.id == input.request_id)
            .map(|item| item.request.clone());
        let Some(existing) = existing else {
            return Err(PermissionError::NotFound {
                request_id: input.request_id,
            });
        };
        self.events
            .publish(PermissionEvent::Replied {
                session_id: existing.session_id.clone(),
                request_id: existing.id.clone(),
                reply: input.reply,
            })
            .await?;

        if input.reply == Reply::Reject {
            let (_, sender) = self.take(&existing.id).unzip();
            let error = match input.message {
                Some(feedback) => PermissionError::Corrected { feedback },
                None => PermissionError::Declined,
            };
            Self::settle(sender.flatten(), Err(error));
            let others: Vec<String> = lock(&self.pending)
                .iter()
                .filter(|item| item.request.session_id == existing.session_id)
                .map(|item| item.request.id.clone())
                .collect();
            for id in others {
                self.events
                    .publish(PermissionEvent::Replied {
                        session_id: existing.session_id.clone(),
                        request_id: id.clone(),
                        reply: Reply::Reject,
                    })
                    .await?;
                if let Some((_, sender)) = self.take(&id) {
                    Self::settle(sender, Err(PermissionError::Declined));
                }
            }
            return Ok(());
        }

        let save = existing.save.clone().unwrap_or_default();
        if input.reply == Reply::Always && !save.is_empty() {
            self.saved
                .add(&self.project_id, &existing.action, &save)
                .await?;
        }
        if let Some((_, sender)) = self.take(&existing.id) {
            Self::settle(sender, Ok(()));
        }
        if input.reply != Reply::Always || save.is_empty() {
            return Ok(());
        }

        let remembered = self.saved_rules().await?;
        let candidates: Vec<(PermissionRequest, Option<String>)> = lock(&self.pending)
            .iter()
            .map(|item| (item.request.clone(), item.agent.clone()))
            .collect();
        for (request, agent) in candidates {
            let rules = match self
                .policy
                .configured(&request.session_id, agent.as_deref())
                .await
            {
                Ok(rules) => rules.unwrap_or_else(missing_agent_permissions),
                Err(PermissionError::SessionNotFound { .. }) => continue,
                Err(error) => return Err(error),
            };
            if denied(&request.action, &request.resources, &rules) {
                continue;
            }
            let effective = [rules, remembered.clone()].concat();
            if !request
                .resources
                .iter()
                .all(|r| evaluate(&request.action, r, &[&effective]).effect == Effect::Allow)
            {
                continue;
            }
            self.events
                .publish(PermissionEvent::Replied {
                    session_id: request.session_id.clone(),
                    request_id: request.id.clone(),
                    reply: Reply::Always,
                })
                .await?;
            if let Some((_, sender)) = self.take(&request.id) {
                Self::settle(sender, Ok(()));
            }
        }
        Ok(())
    }

    pub fn get(&self, id: &str) -> Option<PermissionRequest> {
        lock(&self.pending)
            .iter()
            .find(|item| item.request.id == id)
            .map(|item| item.request.clone())
    }

    pub fn list(&self) -> Vec<PermissionRequest> {
        lock(&self.pending)
            .iter()
            .map(|item| item.request.clone())
            .collect()
    }

    pub fn for_session(&self, session_id: &str) -> Vec<PermissionRequest> {
        lock(&self.pending)
            .iter()
            .filter(|item| item.request.session_id == session_id)
            .map(|item| item.request.clone())
            .collect()
    }

    /// Location teardown: decline every pending request and clear the registry.
    pub fn shutdown(&self) {
        let drained: Vec<Pending> = lock(&self.pending).drain(..).collect();
        for mut item in drained {
            Self::settle(item.sender.take(), Err(PermissionError::Declined));
        }
    }
}
