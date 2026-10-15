//! Purpose: contract tests for bridge subscription registry.
//! Scope: verifies replacement, idempotent disposal, sender teardown, and overflow behavior.
//! Uses:
//! rust/crates/
//! `-- desktop-host/src/bridge/subscriptions.rs
//! External:
//! `-- tokio::test
//! Used by:
//! `-- none, entrypoint
//! Invariants: tests cover all documented overflow behaviors and lifecycle edge cases.
//! Contract tests: this file

use opencode_desktop_host::bridge::subscriptions::{
    OverflowBehavior, SendResult, SubscriptionConfig, SubscriptionRegistry,
};

#[tokio::test]
async fn test_replaces_previous_subscription() {
    let registry = SubscriptionRegistry::<String, String>::default();

    let (mut rx1, _handle1) = registry.subscribe(1, "topic".to_string());
    let _ = registry
        .send(1, &"topic".to_string(), "first".to_string())
        .await;
    assert_eq!(rx1.recv().await, Some("first".to_string()));

    let (mut rx2, _handle2) = registry.subscribe(1, "topic".to_string());

    assert_eq!(rx1.recv().await, None);

    let _ = registry
        .send(1, &"topic".to_string(), "second".to_string())
        .await;
    assert_eq!(rx2.recv().await, Some("second".to_string()));
}

#[tokio::test]
async fn test_idempotent_dispose() {
    let registry = SubscriptionRegistry::<String, String>::default();

    let (_rx, handle) = registry.subscribe(1, "topic".to_string());

    handle.dispose();
    handle.dispose();

    let result = registry
        .send(1, &"topic".to_string(), "msg".to_string())
        .await;
    assert_eq!(result, SendResult::NoSuchSubscription);
}

#[tokio::test]
async fn test_sender_teardown() {
    let registry = SubscriptionRegistry::<String, String>::default();

    let (_rx1, _h1) = registry.subscribe(1, "topic1".to_string());
    let (_rx2, _h2) = registry.subscribe(1, "topic2".to_string());
    let (_rx3, _h3) = registry.subscribe(2, "topic1".to_string());

    registry.teardown_sender(1);

    assert_eq!(
        registry
            .send(1, &"topic1".to_string(), "msg".to_string())
            .await,
        SendResult::NoSuchSubscription
    );
    assert_eq!(
        registry
            .send(1, &"topic2".to_string(), "msg".to_string())
            .await,
        SendResult::NoSuchSubscription
    );

    assert_eq!(
        registry
            .send(2, &"topic1".to_string(), "msg".to_string())
            .await,
        SendResult::Sent
    );
}

#[tokio::test]
async fn test_overflow_drop_newest() {
    let config = SubscriptionConfig {
        capacity: 1,
        overflow: OverflowBehavior::DropNewest,
    };
    let registry = SubscriptionRegistry::<String, String>::new(config);

    let (mut rx, _handle) = registry.subscribe(1, "topic".to_string());

    let _ = registry
        .send(1, &"topic".to_string(), "first".to_string())
        .await;
    let result = registry
        .send(1, &"topic".to_string(), "second".to_string())
        .await;

    assert_eq!(result, SendResult::Dropped);
    assert_eq!(rx.recv().await, Some("first".to_string()));
}

#[tokio::test]
async fn test_overflow_drop_oldest() {
    let config = SubscriptionConfig {
        capacity: 1,
        overflow: OverflowBehavior::DropOldest,
    };
    let registry = SubscriptionRegistry::<String, String>::new(config);

    let (mut rx, _handle) = registry.subscribe(1, "topic".to_string());

    let _ = registry
        .send(1, &"topic".to_string(), "first".to_string())
        .await;
    let result = registry
        .send(1, &"topic".to_string(), "second".to_string())
        .await;

    assert_eq!(result, SendResult::Dropped);
    assert_eq!(rx.recv().await, Some("first".to_string()));
}

#[tokio::test]
async fn test_overflow_block() {
    let config = SubscriptionConfig {
        capacity: 1,
        overflow: OverflowBehavior::Block,
    };
    let registry = SubscriptionRegistry::<String, String>::new(config);

    let (mut rx, _handle) = registry.subscribe(1, "topic".to_string());

    let _ = registry
        .send(1, &"topic".to_string(), "first".to_string())
        .await;

    let registry_clone = registry.clone();
    let send_task = tokio::spawn(async move {
        registry_clone
            .send(1, &"topic".to_string(), "second".to_string())
            .await
    });

    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

    assert_eq!(rx.recv().await, Some("first".to_string()));

    let result = send_task.await.unwrap();
    assert_eq!(result, SendResult::Sent);
    assert_eq!(rx.recv().await, Some("second".to_string()));
}
