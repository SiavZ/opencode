//! Purpose: lifecycle tests for the V2 permission service through its public API, ported from
//! packages/core/test/permission.test.ts (configured denial wins, always persists and rechecks,
//! reject cascades per session, cancellation removes pending, teardown declines).
//! Scope: in-memory test ports (fixture-only, no storage).
//! Uses:
//! rust/crates/
//! `-- permissions/src/service.rs
//! External:
//! `-- tokio
//! Used by:
//! `-- none, entrypoint
//! Invariants: none.
//! Contract tests: permissions/tests/service.rs

use async_trait::async_trait;
use opencode_permissions::*;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Ports {
    rules: Mutex<Option<Ruleset>>,
    saved: Mutex<Vec<SavedGrant>>,
    events: Mutex<Vec<PermissionEvent>>,
}

#[async_trait]
impl PolicySource for Ports {
    async fn configured(
        &self,
        session_id: &str,
        _agent: Option<&str>,
    ) -> Result<Option<Ruleset>, PermissionError> {
        if session_id == "ses_missing" {
            return Err(PermissionError::SessionNotFound {
                session_id: session_id.into(),
            });
        }
        Ok(self.rules.lock().unwrap().clone())
    }
}

#[async_trait]
impl SavedGrants for Ports {
    async fn list(&self, project_id: &str) -> Result<Vec<SavedGrant>, PermissionError> {
        Ok(self
            .saved
            .lock()
            .unwrap()
            .iter()
            .filter(|g| g.project_id == project_id)
            .cloned()
            .collect())
    }
    async fn add(
        &self,
        project_id: &str,
        action: &str,
        resources: &[String],
    ) -> Result<(), PermissionError> {
        let mut saved = self.saved.lock().unwrap();
        for resource in resources {
            if !saved.iter().any(|g| {
                g.project_id == project_id && g.action == action && &g.resource == resource
            }) {
                let id = format!("psv_{}", saved.len());
                saved.push(SavedGrant {
                    id,
                    project_id: project_id.into(),
                    action: action.into(),
                    resource: resource.clone(),
                });
            }
        }
        Ok(())
    }
}

#[async_trait]
impl PermissionEvents for Ports {
    async fn publish(&self, event: PermissionEvent) -> Result<(), PermissionError> {
        self.events.lock().unwrap().push(event);
        Ok(())
    }
}

fn service(rules: Option<Ruleset>) -> (Arc<PermissionService>, Arc<Ports>) {
    let ports = Arc::new(Ports::default());
    *ports.rules.lock().unwrap() = rules;
    let svc = PermissionService::new("prj_1", ports.clone(), ports.clone(), ports.clone());
    (Arc::new(svc), ports)
}

fn input(id: &str, session: &str, action: &str, resource: &str) -> AssertInput {
    let mut input = AssertInput::new(session, action, vec![resource.into()]);
    input.id = Some(id.into());
    input
}

async fn wait_pending(svc: &PermissionService, n: usize) {
    for _ in 0..200 {
        if svc.list().len() >= n {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    }
    panic!("pending never reached {n}");
}

#[tokio::test]
async fn configured_denial_wins_over_saved_grant() {
    let (svc, ports) = service(Some(vec![Rule::new("edit", "secret.txt", Effect::Deny)]));
    ports.add("prj_1", "edit", &["*".into()]).await.unwrap();
    let error = svc
        .assert(input("per_1", "ses_a", "edit", "secret.txt"))
        .await
        .unwrap_err();
    assert_eq!(
        error,
        PermissionError::Blocked {
            rules: vec![Rule::new("edit", "secret.txt", Effect::Deny)]
        }
    );
    assert!(ports.events.lock().unwrap().is_empty());
}

#[tokio::test]
async fn missing_agent_denies_everything() {
    let (svc, _) = service(None);
    let result = svc.ask(input("per_1", "ses_a", "read", "a")).await.unwrap();
    assert_eq!(result.effect, Effect::Deny);
    assert!(svc.list().is_empty());
}

#[tokio::test]
async fn ask_once_then_always_saves_and_resolves_matching_pending() {
    let (svc, ports) = service(Some(vec![]));
    let a = {
        let svc = svc.clone();
        tokio::spawn(async move {
            let mut i = input("per_a", "ses_a", "edit", "x.txt");
            i.save = Some(vec!["*".into()]);
            svc.assert(i).await
        })
    };
    let b = {
        let svc = svc.clone();
        tokio::spawn(async move { svc.assert(input("per_b", "ses_b", "edit", "y.txt")).await })
    };
    wait_pending(&svc, 2).await;
    svc.reply(ReplyInput {
        request_id: "per_a".into(),
        reply: Reply::Always,
        message: None,
    })
    .await
    .unwrap();
    assert_eq!(a.await.unwrap(), Ok(()));
    assert_eq!(b.await.unwrap(), Ok(()));
    assert_eq!(ports.saved.lock().unwrap().len(), 1);
    assert!(svc.list().is_empty());
    // Later asks are allowed by the saved grant.
    assert_eq!(
        svc.ask(input("per_c", "ses_c", "edit", "z"))
            .await
            .unwrap()
            .effect,
        Effect::Allow
    );
}

#[tokio::test]
async fn reject_with_message_corrects_and_cascades_same_session_only() {
    let (svc, ports) = service(Some(vec![]));
    let spawn = |id: &'static str, session: &'static str| {
        let svc = svc.clone();
        tokio::spawn(async move { svc.assert(input(id, session, "bash", "ls")).await })
    };
    let a = spawn("per_a", "ses_a");
    let b = spawn("per_b", "ses_a");
    let c = spawn("per_c", "ses_other");
    wait_pending(&svc, 3).await;
    svc.reply(ReplyInput {
        request_id: "per_a".into(),
        reply: Reply::Reject,
        message: Some("use rg".into()),
    })
    .await
    .unwrap();
    assert_eq!(
        a.await.unwrap(),
        Err(PermissionError::Corrected {
            feedback: "use rg".into()
        })
    );
    assert_eq!(b.await.unwrap(), Err(PermissionError::Declined));
    assert_eq!(svc.list().len(), 1);
    let replies = ports
        .events
        .lock()
        .unwrap()
        .iter()
        .filter(|e| matches!(e, PermissionEvent::Replied { .. }))
        .count();
    assert_eq!(replies, 2);
    svc.shutdown();
    assert_eq!(c.await.unwrap(), Err(PermissionError::Declined));
}

#[tokio::test]
async fn cancelled_assert_removes_pending_and_unknown_reply_fails() {
    let (svc, _) = service(Some(vec![]));
    let task = {
        let svc = svc.clone();
        tokio::spawn(async move { svc.assert(input("per_a", "ses_a", "bash", "ls")).await })
    };
    wait_pending(&svc, 1).await;
    task.abort();
    let _ = task.await;
    assert!(svc.list().is_empty());
    let error = svc
        .reply(ReplyInput {
            request_id: "per_a".into(),
            reply: Reply::Once,
            message: None,
        })
        .await;
    assert_eq!(
        error,
        Err(PermissionError::NotFound {
            request_id: "per_a".into()
        })
    );
}

#[tokio::test]
async fn request_wire_shape_omits_absent_optionals() {
    let request = PermissionRequest {
        id: "per_1".into(),
        session_id: "ses_1".into(),
        action: "read".into(),
        resources: vec!["a".into()],
        save: None,
        metadata: None,
        source: Some(Source::Tool {
            message_id: "msg_1".into(),
            call_id: "c1".into(),
        }),
    };
    assert_eq!(
        serde_json::to_string(&request).unwrap(),
        r#"{"id":"per_1","sessionID":"ses_1","action":"read","resources":["a"],"source":{"type":"tool","messageID":"msg_1","callID":"c1"}}"#
    );
}
