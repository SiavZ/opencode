//! Purpose: verify real permission-approved tool continuation survives provider failure and interruption.
//! Scope: actual Process and loopback provider transport, not an injected runner.
//! Uses: shared permission Host, typed Process reply and native read tool.
//! Used by: runtime test permissions.
//! Invariants: provider failure/cancellation cannot strand an approved permission request or task.
//! Contract tests: this module.

use super::{Duration, Host, Provider, Reply, ReplyInput, json, provider};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn approved_native_tool_provider_failure_and_interrupt_leave_no_pending_requests() {
    for failure in [true, false] {
        let mut provider = Provider::start(vec![
            provider::Reply::Tool {
                name: "read",
                arguments: json!({"path":"evidence.txt"}),
            },
            if failure {
                provider::Reply::Failure
            } else {
                provider::Reply::Hold
            },
        ])
        .await;
        let host = Host::start(&provider).await;
        let session = host.create().await;
        host.prompt(&session).await;
        let pending = host.pending().await;
        host.process
            .reply_permission(
                host.directory.clone(),
                ReplyInput {
                    request_id: pending.id,
                    reply: Reply::Once,
                    message: None,
                },
            )
            .await
            .unwrap();
        let continuation = tokio::time::timeout(Duration::from_secs(10), async {
            provider.requests.recv().await.unwrap();
            provider.requests.recv().await.unwrap()
        })
        .await
        .expect("provider must receive the real approved tool continuation");
        assert!(
            continuation
                .1
                .to_string()
                .contains("native approved evidence"),
            "{:?}",
            continuation.1
        );
        if failure {
            let context = host.completed(&session, "controlled rejection").await;
            assert!(
                context
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|message| message["type"] == "assistant" && message["finish"] == "error"),
                "{context}"
            );
        } else {
            let response = host
                .client
                .post(format!(
                    "{}/api/session/{session}/interrupt",
                    host.listener.url()
                ))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 204);
        }
        assert!(
            host.process
                .pending_permissions(host.directory.clone())
                .await
                .unwrap()
                .is_empty()
        );
        tokio::time::timeout(Duration::from_secs(10), host.stop())
            .await
            .expect("native/provider task shutdown must join");
        provider.stop().await;
    }
}
