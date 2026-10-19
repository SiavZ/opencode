//! Purpose: shared Location question admission, user replies and cancellation-safe settlement.
//! Scope: question domain independent of HTTP, TUI and model execution.
//! Uses: schema/question, tool-api; async-trait, futures, serde_json, tokio.
//! Used by: runtime/location_questions and native HTTP adapters.
//! Invariants: publication precedes settlement; canceled waiters remove only their own request.
//! Contract tests: tools/tests/questions.rs and runtime/tests/permissions.rs.

use async_trait::async_trait;
use futures::channel::oneshot;
use opencode_schema::ids::{QuestionID, SessionID};
use opencode_schema::question::{Answer, Info, Request, Tool};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

#[derive(Debug, thiserror::Error)]
pub enum QuestionError {
    #[error("Question not found: {0}")]
    NotFound(String),
    #[error("The user dismissed this question")]
    Rejected,
    #[error("{0}")]
    Publication(String),
}

#[async_trait]
pub trait QuestionEvents: Send + Sync {
    async fn publish(&self, name: &'static str, value: serde_json::Value) -> Result<(), String>;
}

struct Pending {
    request: Request,
    sender: oneshot::Sender<Result<Vec<Answer>, QuestionError>>,
}

#[derive(Default)]
struct State {
    closed: bool,
    pending: BTreeMap<String, Pending>,
}

pub struct Questions {
    state: Arc<Mutex<State>>,
    events: Arc<dyn QuestionEvents>,
    replies: futures::lock::Mutex<()>,
}

impl Questions {
    pub fn new(events: Arc<dyn QuestionEvents>) -> Self {
        Self {
            state: Arc::new(Mutex::new(State::default())),
            events,
            replies: futures::lock::Mutex::new(()),
        }
    }

    pub fn list(&self) -> Vec<Request> {
        lock(&self.state)
            .pending
            .values()
            .map(|entry| entry.request.clone())
            .collect()
    }

    pub async fn ask(
        &self,
        session_id: SessionID,
        questions: Vec<Info>,
        tool: Option<Tool>,
    ) -> Result<Vec<Answer>, QuestionError> {
        let request = Request {
            id: QuestionID::new(format!("que_{}", opencode_schema::identifier::ascending()))
                .map_err(|error| QuestionError::Publication(error.to_string()))?,
            session_id,
            questions,
            tool,
        };
        let id = request.id.to_string();
        let (sender, receiver) = oneshot::channel();
        {
            let mut state = lock(&self.state);
            if state.closed {
                return Err(QuestionError::Rejected);
            }
            state.pending.insert(
                id.clone(),
                Pending {
                    request: request.clone(),
                    sender,
                },
            );
        }
        let _guard = Guard {
            state: self.state.clone(),
            id,
        };
        self.events
            .publish(
                "question.v2.asked",
                serde_json::to_value(request)
                    .map_err(|error| QuestionError::Publication(error.to_string()))?,
            )
            .await
            .map_err(QuestionError::Publication)?;
        receiver.await.unwrap_or(Err(QuestionError::Rejected))
    }

    pub async fn reply(&self, id: &str, answers: Vec<Answer>) -> Result<(), QuestionError> {
        self.settle(id, Some(answers)).await
    }

    pub async fn reject(&self, id: &str) -> Result<(), QuestionError> {
        self.settle(id, None).await
    }

    async fn settle(&self, id: &str, answers: Option<Vec<Answer>>) -> Result<(), QuestionError> {
        let _reply = self.replies.lock().await;
        let request = lock(&self.state)
            .pending
            .get(id)
            .map(|pending| pending.request.clone())
            .ok_or_else(|| QuestionError::NotFound(id.into()))?;
        let name = if answers.is_some() {
            "question.v2.replied"
        } else {
            "question.v2.rejected"
        };
        let mut value =
            serde_json::json!({"sessionID": request.session_id, "requestID": request.id});
        if let Some(answers) = &answers {
            value["answers"] = serde_json::json!(answers);
        }
        self.events
            .publish(name, value)
            .await
            .map_err(QuestionError::Publication)?;
        let pending = lock(&self.state)
            .pending
            .remove(id)
            .ok_or_else(|| QuestionError::NotFound(id.into()))?;
        let _ = pending.sender.send(answers.ok_or(QuestionError::Rejected));
        Ok(())
    }

    pub fn shutdown(&self) {
        let mut state = lock(&self.state);
        state.closed = true;
        for (_, pending) in std::mem::take(&mut state.pending) {
            let _ = pending.sender.send(Err(QuestionError::Rejected));
        }
    }
}

struct Guard {
    state: Arc<Mutex<State>>,
    id: String,
}
impl Drop for Guard {
    fn drop(&mut self) {
        lock(&self.state).pending.remove(&self.id);
    }
}

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
