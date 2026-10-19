//! Purpose: Slack thread, output and full Socket Mode bot acceptance fixtures.
//! Scope: loopback Web API/websocket and fake SessionPort, no real Slack credentials.
//! Uses:
//! rust/crates/
//! |-- ecosystem/src/session.rs
//! |-- ecosystem/src/slack/bot.rs
//! |-- ecosystem/src/slack/mod.rs
//! |-- ecosystem/src/slack/output.rs
//! |-- ecosystem/src/slack/thread.rs
//! |-- ecosystem/tests/slack_session.rs
//! `-- ecosystem/tests/slack_support.rs
//! External:
//! |-- serde_json::json
//! |-- tokio::join
//! |-- tokio::spawn
//! |-- tokio::test
//! |-- tokio::time::timeout
//! `-- tokio_util::sync::CancellationToken
//! Used by:
//! `-- none, entrypoint
//! Invariants: all started bot/server tasks are cancelled and joined.
//! Contract tests: this file.

mod slack_session;
mod slack_support;

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use opencode_ecosystem::session::SessionEvent;
use opencode_ecosystem::slack::SlackError;
use opencode_ecosystem::slack::bot::{BotConfig, SlackBot};
use opencode_ecosystem::slack::output::{
    CREATE_ERROR, EMPTY_RESPONSE, PROMPT_ERROR, TEST_RESPONSE,
};
use opencode_ecosystem::slack::thread::{SlackMessage, ThreadRouter};
use serde_json::json;
use slack_session::Sessions;
use slack_support::{Server, envelope, wait_for};
use tokio_util::sync::CancellationToken;

fn message(channel: &str, thread: &str, text: &str) -> SlackMessage {
    SlackMessage {
        channel: channel.into(),
        thread: thread.into(),
        text: text.into(),
    }
}

fn config() -> BotConfig {
    let mut config = BotConfig::default();
    config.socket.reconnect_base = Duration::from_millis(5);
    config.socket.reconnect_max = Duration::from_millis(20);
    config
}

fn tool(session: &str, status: &str) -> SessionEvent {
    SessionEvent {
        kind: "message.part.updated".into(),
        properties: json!({ "part": {
        "type":"tool", "sessionID":session, "tool":"bash", "state":{"status":status,"title":"list files"}
    } }),
    }
}

#[tokio::test]
async fn fixture_missing_share_url_preserves_omitted_text_post() {
    let server = Server::start().await;
    let sessions = Arc::new(Sessions::default());
    sessions.missing_share_url.store(true, Ordering::SeqCst);
    let router = ThreadRouter::new(sessions.clone(), server.api.clone());
    router.handle(message("C", "T", "first")).await.unwrap();
    assert_eq!(
        server.logs.posts.lock().unwrap()[0],
        json!({"channel":"C","thread_ts":"T"})
    );
    *server.logs.api_error.lock().unwrap() = Some("no_text".into());
    assert!(
        matches!(router.handle(message("C", "U", "second")).await, Err(SlackError::Api { code }) if code == "no_text")
    );
    assert_eq!(router.mapped_sessions(), 2);
    assert_eq!(sessions.prompts.lock().unwrap().len(), 1);
    *server.logs.api_error.lock().unwrap() = None;
    router.handle(message("C", "U", "retry")).await.unwrap();
    assert_eq!(sessions.titles.lock().unwrap().len(), 2);
    assert_eq!(sessions.shares.load(Ordering::SeqCst), 2);
    server.shutdown().await;
}

#[tokio::test]
async fn fixture_thread_serialization_and_channel_isolation() {
    let server = Server::start().await;
    let sessions = Arc::new(Sessions::default());
    let router = ThreadRouter::new(sessions.clone(), server.api.clone());
    let (a, b) = tokio::join!(
        router.handle(message("C1", "1", "first")),
        router.handle(message("C1", "1", "second"))
    );
    a.unwrap();
    b.unwrap();
    assert_eq!(router.mapped_sessions(), 1);
    assert_eq!(*sessions.titles.lock().unwrap(), vec!["Slack thread 1"]);
    assert_eq!(sessions.shares.load(Ordering::SeqCst), 1);
    assert_eq!(sessions.peak.load(Ordering::SeqCst), 1);
    assert!(
        sessions
            .prompts
            .lock()
            .unwrap()
            .iter()
            .all(|(id, _)| id == "s1")
    );
    assert_eq!(
        server.logs.posts.lock().unwrap()[0],
        json!({"channel":"C1","text":"https://fixture.invalid/share/s1","thread_ts":"1"})
    );
    let (a, b) = tokio::join!(
        router.handle(message("C2", "1", "other channel")),
        router.handle(message("C1", "2", "other thread"))
    );
    a.unwrap();
    b.unwrap();
    assert_eq!(router.mapped_sessions(), 3);
    assert!(sessions.peak.load(Ordering::SeqCst) >= 2);
    server.shutdown().await;
}

#[tokio::test]
async fn fixture_session_failure_fallbacks_and_reply_precedence() {
    let server = Server::start().await;
    let sessions = Arc::new(Sessions::default());
    let router = ThreadRouter::new(sessions.clone(), server.api.clone());
    sessions.fail_create.store(true, Ordering::SeqCst);
    router.handle(message("C", "1", "first")).await.unwrap();
    assert_eq!(server.logs.posts.lock().unwrap()[0]["text"], CREATE_ERROR);
    assert_eq!(router.mapped_sessions(), 0);
    sessions.fail_create.store(false, Ordering::SeqCst);
    sessions.fail_share.store(true, Ordering::SeqCst);
    for text in ["error", "empty", "legacy", "normal"] {
        router.handle(message("C", "1", text)).await.unwrap();
    }
    let texts = server
        .logs
        .posts
        .lock()
        .unwrap()
        .iter()
        .map(|v| v["text"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert_eq!(
        texts,
        vec![
            CREATE_ERROR,
            PROMPT_ERROR,
            EMPTY_RESPONSE,
            "legacy content",
            "reply normal\nsecond"
        ]
    );
    assert_eq!(sessions.titles.lock().unwrap().len(), 1);
    assert_eq!(sessions.shares.load(Ordering::SeqCst), 1);
    server.shutdown().await;
}

#[tokio::test]
async fn fixture_tool_updates_filter_session_and_completed_state() {
    let server = Server::start().await;
    let sessions = Arc::new(Sessions::default());
    let router = ThreadRouter::new(sessions, server.api.clone().with_retries(0));
    router.handle(message("C", "1", "normal")).await.unwrap();
    let before = server.logs.posts.lock().unwrap().len();
    router.tool_event(&tool("s1", "running")).await.unwrap();
    router
        .tool_event(&tool("missing", "completed"))
        .await
        .unwrap();
    assert_eq!(server.logs.posts.lock().unwrap().len(), before);
    router.tool_event(&tool("s1", "completed")).await.unwrap();
    assert_eq!(
        server.logs.posts.lock().unwrap().last().unwrap(),
        &json!({"channel":"C","text":"*bash* - list files","thread_ts":"1"})
    );
    server.logs.http_status.store(500, Ordering::SeqCst);
    router.tool_event(&tool("s1", "completed")).await.unwrap();
    server.shutdown().await;
}

#[test]
fn fixture_message_filters_and_thread_selection() {
    let mut event = json!({"type":"message","channel":"C","ts":"1","text":"hi"});
    assert_eq!(
        SlackMessage::from_event(&event, None).unwrap(),
        message("C", "1", "hi")
    );
    event["thread_ts"] = json!("2");
    assert_eq!(SlackMessage::from_event(&event, None).unwrap().thread, "2");
    event["user"] = json!("U_BOT");
    assert!(SlackMessage::from_event(&event, Some("U_BOT")).is_none());
    event["subtype"] = json!("bot_message");
    assert!(SlackMessage::from_event(&event, None).is_none());
    event.as_object_mut().unwrap().remove("subtype");
    event["text"] = json!("");
    assert!(SlackMessage::from_event(&event, None).is_none());
    event.as_object_mut().unwrap().remove("text");
    assert!(SlackMessage::from_event(&event, None).is_none());
}

#[tokio::test]
async fn fixture_full_bot_ack_dedup_test_command_tools_and_reconnect() {
    let server = Server::start().await;
    let sessions = Arc::new(Sessions::default());
    let bot = Arc::new(SlackBot::new(
        server.api.clone(),
        sessions.clone(),
        config(),
    ));
    let cancel = CancellationToken::new();
    let running = bot.clone();
    let stop = cancel.clone();
    let task = tokio::spawn(async move { running.run(stop).await });
    wait_for(|| server.logs.sockets.load(Ordering::SeqCst) == 1).await;
    let first = envelope("e1", "C", "1", "normal");
    server.send(first.clone()).await;
    server.send(first.clone()).await;
    let mut redelivery = first.clone();
    redelivery["envelope_id"] = json!("retry-e1");
    redelivery["retry_attempt"] = json!(1);
    redelivery["retry_reason"] = json!("timeout");
    server.send(redelivery).await;
    server.send(json!({"envelope_id":"cmd","type":"slash_commands","payload":{"command":"/test","channel_id":"C"}})).await;
    let mut skipped = envelope("skip", "C", "1", "subtype");
    skipped["payload"]["event"]["subtype"] = json!("message_changed");
    server.send(skipped).await;
    let mut self_message = envelope("self", "C", "1", "self");
    self_message["payload"]["event"]["user"] = json!("U_BOT");
    server.send(self_message).await;
    wait_for(|| {
        server.logs.acks.lock().unwrap().len() == 6 && server.logs.posts.lock().unwrap().len() == 3
    })
    .await;
    assert_eq!(sessions.prompts.lock().unwrap().len(), 1);
    assert_eq!(bot.threads.mapped_sessions(), 1);
    assert!(
        server
            .logs
            .posts
            .lock()
            .unwrap()
            .contains(&json!({"channel":"C","text":TEST_RESPONSE}))
    );
    assert!(
        server
            .logs
            .acks
            .lock()
            .unwrap()
            .iter()
            .all(|a| a["payload"] == json!({}))
    );
    sessions.events.send(tool("s1", "completed")).await.unwrap();
    wait_for(|| server.logs.posts.lock().unwrap().len() == 4).await;
    server
        .send(json!({"type":"disconnect","reason":"warning"}))
        .await;
    wait_for(|| server.logs.sockets.load(Ordering::SeqCst) == 2).await;
    server.send(first).await;
    server.send(envelope("e2", "C", "1", "next")).await;
    wait_for(|| {
        server.logs.acks.lock().unwrap().len() == 8 && server.logs.posts.lock().unwrap().len() == 5
    })
    .await;
    assert_eq!(sessions.prompts.lock().unwrap().len(), 2);
    assert_eq!(sessions.titles.lock().unwrap().len(), 1);
    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(
        sessions
            .subscription_cancel
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .is_cancelled()
    );
    server.shutdown().await;
}

#[tokio::test]
async fn fixture_shutdown_joins_blocked_prompt_and_ack_is_immediate() {
    let server = Server::start().await;
    let sessions = Arc::new(Sessions::default());
    let bot = SlackBot::new(server.api.clone(), sessions.clone(), config());
    let cancel = CancellationToken::new();
    let stop = cancel.clone();
    let task = tokio::spawn(async move { bot.run(stop).await });
    server.send(envelope("blocked", "C", "1", "blocked")).await;
    wait_for(|| {
        sessions.active.load(Ordering::SeqCst) == 1 && server.logs.acks.lock().unwrap().len() == 1
    })
    .await;
    assert_eq!(
        server.logs.acks.lock().unwrap()[0],
        json!({"envelope_id":"blocked","payload":{}})
    );
    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(sessions.active.load(Ordering::SeqCst), 0);
    server.shutdown().await;
}

#[tokio::test]
async fn fixture_thread_limits_and_operation_timeout_are_typed() {
    let server = Server::start().await;
    let sessions = Arc::new(Sessions::default());
    let router =
        ThreadRouter::new(sessions, server.api.clone()).with_limits(1, Duration::from_millis(50));
    assert!(matches!(
        router.handle(message("C", "1", "blocked")).await,
        Err(SlackError::Timeout)
    ));
    assert!(matches!(
        router.handle(message("C", "2", "new")).await,
        Err(SlackError::ThreadLimit)
    ));
    router.handle(message("C", "1", "retry")).await.unwrap();
    server.shutdown().await;
}
