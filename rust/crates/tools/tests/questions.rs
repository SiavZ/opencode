//! Purpose: exercise actual shared question pending/reply and ownership cleanup.
//! Scope: public question domain with a recording event sink, no duplicate domain logic.
//! Uses: tools/interaction, schema/question; async-trait, tokio.
//! Used by: cargo test --test questions.
//! Invariants: cancellation, rejection and failed publication leave no orphan pending requests.
//! Contract tests: this file.

use async_trait::async_trait;
use opencode_schema::{SessionID, question::Info};
use opencode_tools::interaction::{QuestionError, QuestionEvents, Questions};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Events {
    values: Mutex<Vec<(String, serde_json::Value)>>,
    fail: bool,
}
#[async_trait]
impl QuestionEvents for Events {
    async fn publish(&self, name: &'static str, value: serde_json::Value) -> Result<(), String> {
        if self.fail {
            return Err("publication failed".into());
        }
        self.values.lock().unwrap().push((name.into(), value));
        Ok(())
    }
}

fn prompt() -> Vec<Info> {
    vec![Info {
        question: "Which target?".into(),
        header: "Target".into(),
        options: vec![],
        multiple: None,
        custom: None,
    }]
}

async fn pending(service: &Questions) -> String {
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if let Some(request) = service.list().first() {
                break request.id.to_string();
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn actual_pending_reply_reject_cancel_and_closed_admission() {
    let events = Arc::new(Events::default());
    let service = Arc::new(Questions::new(events.clone()));
    let session = SessionID::new("ses_question_test").unwrap();
    let task = {
        let service = service.clone();
        let session = session.clone();
        tokio::spawn(async move { service.ask(session, prompt(), None).await })
    };
    let id = pending(&service).await;
    assert_eq!(service.list()[0].questions[0].header, "Target");
    service
        .reply(&id, vec![vec!["Linux".into()]])
        .await
        .unwrap();
    assert_eq!(task.await.unwrap().unwrap(), vec![vec!["Linux"]]);
    assert!(service.list().is_empty());
    assert!(matches!(
        service.reply(&id, vec![]).await,
        Err(QuestionError::NotFound(_))
    ));
    let task = {
        let service = service.clone();
        let session = session.clone();
        tokio::spawn(async move { service.ask(session, prompt(), None).await })
    };
    let id = pending(&service).await;
    service.reject(&id).await.unwrap();
    assert!(matches!(task.await.unwrap(), Err(QuestionError::Rejected)));
    let task = {
        let service = service.clone();
        let session = session.clone();
        tokio::spawn(async move { service.ask(session, prompt(), None).await })
    };
    pending(&service).await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(service.list().is_empty());
    let task = {
        let service = service.clone();
        let session = session.clone();
        tokio::spawn(async move { service.ask(session, prompt(), None).await })
    };
    pending(&service).await;
    service.shutdown();
    assert!(matches!(task.await.unwrap(), Err(QuestionError::Rejected)));
    assert!(matches!(
        service.ask(session, prompt(), None).await,
        Err(QuestionError::Rejected)
    ));
    let events = events.values.lock().unwrap();
    assert_eq!(events[0].0, "question.v2.asked");
    assert_eq!(events[1].0, "question.v2.replied");
    assert_eq!(events[3].0, "question.v2.rejected");
}

#[tokio::test]
async fn failed_question_publication_cleans_admission() {
    let service = Questions::new(Arc::new(Events {
        fail: true,
        ..Events::default()
    }));
    assert!(matches!(
        service
            .ask(
                SessionID::new("ses_question_failure").unwrap(),
                prompt(),
                None
            )
            .await,
        Err(QuestionError::Publication(_))
    ));
    assert!(service.list().is_empty());
}
