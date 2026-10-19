//! Purpose: actual provider search tools cannot traverse an internal symlink outside the Location.
//! Scope: real Process, native path resolver and loopback provider; no duplicated path policy.
//! Uses: runtime permission acceptance Host, native filesystem symlinks.
//! Used by: runtime test permissions.
//! Invariants: no external file content enters model history and no external search is silently granted.
//! Contract tests: this module.

#[cfg(unix)]
use super::{Host, Provider, json, provider};

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_search_rejects_symlink_and_relative_escape_before_observing_contents() {
    for (name, path) in [("grep", "escape"), ("glob", "../")] {
        let provider = Provider::start(vec![
            provider::Reply::Tool {
                name,
                arguments: json!({"pattern":"forbidden","path":path}),
            },
            provider::Reply::Text("secured search continuation"),
        ])
        .await;
        let host = Host::start(&provider).await;
        let external = tempfile::tempdir().unwrap();
        std::fs::write(
            external.path().join("evidence.txt"),
            "forbidden native search evidence",
        )
        .unwrap();
        std::os::unix::fs::symlink(external.path(), host.directory.join("escape")).unwrap();
        let session = host.create().await;
        host.prompt(&session).await;
        let context = host
            .completed(&session, "secured search continuation")
            .await;
        assert!(
            !context
                .to_string()
                .contains("forbidden native search evidence"),
            "{context}"
        );
        assert!(context.to_string().contains("error"), "{context}");
        assert!(
            host.process
                .pending_permissions(host.directory.clone())
                .await
                .unwrap()
                .is_empty()
        );
        host.stop().await;
        provider.stop().await;
    }
}
