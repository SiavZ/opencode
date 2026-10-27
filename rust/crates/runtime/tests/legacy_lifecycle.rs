//! Purpose: typed host lifecycle acceptance over real durable Session projectors and transport.
//! Scope: rename, visible-history fork, fresh admissions, continuation, restart and aggregate deletion.
//! Invariants: pending inputs and source identities are never copied into a fork.
//! Contract tests: this integration target.

#[allow(dead_code)]
#[path = "support/legacy_host.rs"]
mod legacy_host;
#[allow(dead_code)]
#[path = "support/provider.rs"]
mod provider;

use std::time::Duration;

use legacy_host::Host;
use opencode_schema::ids::ModelID;
use opencode_schema::provider::ModelRef;
use opencode_session_api::admission::{Delivery, PromptRequest};
use opencode_session_api::operations::{CreateRequest, SessionService};
use opencode_session_api::{LocationRef, MessageID, PromptInput};
use opencode_storage_api::DatabaseExt;
use provider::{Provider, Reply};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn typed_fork_continues_real_visible_history_without_copying_pending_inputs() {
    let mut provider = Provider::start(vec![
        Reply::Text("source evidence"),
        Reply::Text("fork continuation"),
    ])
    .await;
    let host = Host::start(Some(&provider.url)).await;
    assert!(host.root.path().exists());
    let source = host
        .process
        .session
        .create(CreateRequest {
            id: None,
            agent: None,
            model: Some(ModelRef {
                provider_id: opencode_schema::ProviderID::new("local"),
                id: ModelID::new("local-test"),
                variant: None,
            }),
            location: LocationRef {
                directory: opencode_schema::values::AbsolutePath::new(
                    host.directory.display().to_string(),
                ),
                workspace_id: None,
            },
        })
        .await
        .unwrap();
    host.process
        .session
        .rename(&source.id, "Typed title")
        .await
        .unwrap();
    host.process
        .session
        .prompt(PromptRequest {
            id: None,
            session_id: source.id.clone(),
            prompt: PromptInput::text("Source prompt"),
            delivery: None,
            resume: None,
        })
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(15),async {
        loop {
            let messages=host.process.session.context(&source.id).await.unwrap();
            if messages.iter().any(|message| matches!(message,opencode_session_api::message::Message::Assistant(message) if message.time.completed.is_some())) && !host.process.session.active().await.contains(&source.id) {break;}
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.unwrap();
    let pending = MessageID::create();
    host.process
        .session
        .prompt(PromptRequest {
            id: Some(pending.clone()),
            session_id: source.id.clone(),
            prompt: PromptInput::text("Pending must remain source-only"),
            delivery: Some(Delivery::Queue),
            resume: Some(false),
        })
        .await
        .unwrap();
    let visible = host.process.session.context(&source.id).await.unwrap();
    let fork = host.process.session.fork(&source.id, None).await.unwrap();
    assert_eq!(fork.title.as_str(), "Typed title (fork #1)");
    assert_eq!(fork.location, source.location);
    let forked = host.process.session.context(&fork.id).await.unwrap();
    assert_eq!(forked.len(), visible.len());
    assert!(
        forked
            .iter()
            .zip(&visible)
            .all(|(target, source)| target.id() != source.id())
    );
    assert!(
        !serde_json::to_string(&forked)
            .unwrap()
            .contains(pending.as_str())
    );
    let id = fork.id.as_str().to_owned();
    let pending_count=host.process.database.read(move |sql|sql.query_row("SELECT COUNT(*) AS count FROM session_input WHERE session_id = ? AND promoted_seq IS NULL",&[id.into()])).await.unwrap().unwrap().get::<i64>("count").unwrap();
    assert_eq!(pending_count, 0);
    host.process
        .session
        .prompt(PromptRequest {
            id: None,
            session_id: fork.id.clone(),
            prompt: PromptInput::text("Continue fork"),
            delivery: None,
            resume: None,
        })
        .await
        .unwrap();
    let _ = provider.requests.recv().await.unwrap();
    let next = tokio::time::timeout(Duration::from_secs(15), provider.requests.recv())
        .await
        .unwrap()
        .unwrap()
        .1;
    assert!(next["messages"].to_string().contains("source evidence"));
    assert!(
        !next["messages"]
            .to_string()
            .contains("Pending must remain source-only")
    );
    tokio::time::timeout(Duration::from_secs(15), async {
        while host.process.session.active().await.contains(&fork.id) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let expected = host.process.session.context(&fork.id).await.unwrap();
    let options = host.options.clone();
    host.process.shutdown().await;
    let process = opencode_runtime::Process::start(options).await.unwrap();
    assert_eq!(process.session.context(&fork.id).await.unwrap(), expected);
    process.session.delete(&source.id).await.unwrap();
    assert!(process.session.get(&source.id).await.is_err());
    assert_eq!(process.session.context(&fork.id).await.unwrap(), expected);
    process.session.delete(&fork.id).await.unwrap();
    let id = fork.id.as_str().to_owned();
    let rows = process
        .database
        .read(move |sql| {
            sql.query_row(
                "SELECT COUNT(*) AS count FROM session_message WHERE session_id = ?",
                &[id.into()],
            )
        })
        .await
        .unwrap()
        .unwrap()
        .get::<i64>("count")
        .unwrap();
    assert_eq!(rows, 0);
    process.shutdown().await;
    provider.stop().await;
}
