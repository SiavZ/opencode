//! Purpose: real provider question-tool continuation across shared Process and HTTP replies.
//! Scope: isolated loopback provider and actual native runtime, no synthetic question consumer.
//! Uses: runtime/tests/permissions.rs host; runtime Process shared question facades.
//! Used by: runtime test permissions.
//! Invariants: wrong-session replies cannot settle a pending question; replies resume provider work.
//! Contract tests: this module.

use super::{Duration, Host, Provider, Reply, ReplyInput, json, provider};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_question_http_reply_continues_provider_and_reject_cleans_pending() {
    for reject in [false, true] {
        let provider = Provider::start(vec![provider::Reply::Tool {
            name: "question", arguments: json!({"questions":[{"question":"Choose target","header":"Target","options":[{"label":"Linux","description":"Native"}],"multiple":false}]}),
        }, provider::Reply::Text("question continuation")]).await;
        let host = Host::start(&provider).await;
        let session = host.create().await;
        host.prompt(&session).await;
        let request = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Some(request) = host
                    .process
                    .pending_questions(host.directory.clone())
                    .await
                    .unwrap()
                    .into_iter()
                    .next()
                {
                    break request;
                }
                // A configured question ask, if present, uses the same permission service.
                if let Some(request) = host
                    .process
                    .pending_permissions(host.directory.clone())
                    .await
                    .unwrap()
                    .into_iter()
                    .next()
                {
                    assert_eq!(request.action, "question");
                    host.process
                        .reply_permission(
                            host.directory.clone(),
                            ReplyInput {
                                request_id: request.id,
                                reply: Reply::Once,
                                message: None,
                            },
                        )
                        .await
                        .unwrap();
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("native question tool must publish a real pending request");
        assert_eq!(request.session_id.as_str(), session);
        assert_eq!(request.questions[0].header, "Target");
        let other = host.create().await;
        let url = format!(
            "{}/api/session/{other}/question/{}/reply",
            host.listener.url(),
            request.id
        );
        let response = host
            .client
            .post(url)
            .json(&json!({"answers":[["Linux"]]}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 404);
        assert_eq!(
            host.process
                .pending_questions(host.directory.clone())
                .await
                .unwrap()
                .len(),
            1
        );
        if reject {
            host.process
                .reject_question(host.directory.clone(), request.id.as_str())
                .await
                .unwrap();
        } else {
            let response = host
                .client
                .post(format!(
                    "{}/api/session/{session}/question/{}/reply",
                    host.listener.url(),
                    request.id
                ))
                .json(&json!({"answers":[["Linux"]]}))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 204);
        }
        let context = host.completed(&session, "question continuation").await;
        assert!(
            context.to_string().contains("question continuation"),
            "{context}"
        );
        assert!(
            context
                .to_string()
                .contains(if reject { "dismissed" } else { "Linux" }),
            "{context}"
        );
        assert!(
            host.process
                .pending_questions(host.directory.clone())
                .await
                .unwrap()
                .is_empty()
        );
        host.stop().await;
        provider.stop().await;
    }
}
