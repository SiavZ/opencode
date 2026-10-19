//! Purpose: compose real automatic Session compaction and native snapshot services for the runner.
//! Scope: Location-scoped execution adapters, with configuration supplied by the Location builder.
//! Uses:
//! rust/crates/
//! |-- event-store/src/lib.rs
//! |-- llm-api/src/lib.rs
//! |-- platform-api/src/snapshot.rs
//! |-- platform-native/src/vcs/mod.rs
//! |-- platform-native/src/vcs/snapshot.rs
//! |-- provider-adapters/src/protocols/openai/chat.rs
//! |-- provider-adapters/src/route.rs
//! |-- provider-adapters/src/transport/executor.rs
//! |-- provider-adapters/src/transport/http.rs
//! |-- runtime/tests/support/provider.rs
//! |-- schema/src/session_message.rs
//! |-- schema/src/values.rs
//! |-- session-api/src/lib.rs
//! |-- session-runtime/src/compaction.rs
//! |-- session-runtime/src/projection.rs
//! |-- session-runtime/src/runner/ports.rs
//! |-- storage-api/src/lib.rs
//! `-- storage-sqlite/src/lib.rs
//! External:
//! |-- async_trait::async_trait
//! |-- compaction::Compaction
//! |-- compaction::Settings
//! |-- ports::Compaction
//! |-- ports::CompactionInput
//! |-- ports::Snapshots
//! |-- provider::Provider::start
//! |-- reqwest::Client::builder
//! |-- serde_json::Value
//! |-- serde_json::json
//! |-- snapshot::Snapshots::capture
//! |-- snapshot::Snapshots::files
//! |-- tempfile::tempdir
//! |-- tokio::test
//! `-- tokio::time::timeout
//! Used by:
//! rust/crates/
//! |-- runtime/src/lib.rs
//! `-- runtime/src/location.rs
//! Invariants: compaction publishes through the real Session event projector; snapshot capture
//! and file comparison use the native Git implementation. Snapshot failures remain best effort.
//! Contract tests: runtime/src/execution_support.rs (unit tests),
//! platform-native/tests/snapshot.rs, session-runtime/tests/compaction.rs

use std::sync::Arc;

use async_trait::async_trait;
use opencode_llm_api::LlmClient;
use opencode_platform_api::snapshot;
use opencode_platform_native::vcs::snapshot::NativeSnapshots;
use opencode_schema::values::RelativePath;
use opencode_session_api::SessionError;
use opencode_session_runtime::compaction;
use opencode_session_runtime::projection::SessionEvents;
use opencode_session_runtime::runner::ports;

/// Real compaction with the folded Location configuration, not a disabled fallback.
pub struct SessionCompaction {
    pub events: Arc<dyn SessionEvents>,
    pub llm: Arc<dyn LlmClient>,
    pub settings: compaction::Settings,
}

#[async_trait]
impl ports::Compaction for SessionCompaction {
    async fn compact_if_needed(
        &self,
        input: &ports::CompactionInput<'_>,
    ) -> Result<bool, SessionError> {
        compaction::Compaction {
            events: self.events.as_ref(),
            llm: self.llm.as_ref(),
            settings: self.settings,
        }
        .compact_if_needed(
            input.session_id,
            &input
                .entries
                .iter()
                .map(|(_, message)| message.clone())
                .collect::<Vec<_>>(),
            input.request,
        )
        .await
    }

    async fn compact_after_overflow(
        &self,
        input: &ports::CompactionInput<'_>,
    ) -> Result<bool, SessionError> {
        compaction::Compaction {
            events: self.events.as_ref(),
            llm: self.llm.as_ref(),
            settings: self.settings,
        }
        .compact_after_overflow(
            input.session_id,
            &input
                .entries
                .iter()
                .map(|(_, message)| message.clone())
                .collect::<Vec<_>>(),
            input.request,
        )
        .await
    }
}

/// Native snapshots initialized with the actual project, Location and data directory.
pub struct RunnerSnapshots(pub NativeSnapshots);

#[async_trait]
impl ports::Snapshots for RunnerSnapshots {
    async fn capture(&self) -> Option<String> {
        snapshot::Snapshots::capture(&self.0).await
    }

    async fn files(&self, from: &str, to: &str) -> Option<Vec<RelativePath>> {
        snapshot::Snapshots::files(&self.0, from, to)
            .await
            .ok()
            .map(|files| files.into_iter().map(RelativePath::new).collect())
    }
}

#[cfg(test)]
#[allow(dead_code)]
#[path = "../tests/support/provider.rs"]
mod provider;

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use opencode_platform_native::vcs::{GitCli, NativeGit, NativeSnapshots, SnapshotConfig};
    use opencode_session_runtime::runner::ports::Snapshots;

    use super::{RunnerSnapshots, SessionCompaction, provider};

    #[tokio::test]
    async fn compaction_uses_real_provider_transport_and_persists_projected_summary() {
        use std::sync::Arc;
        use std::time::Duration;

        use opencode_event_store::SqlEventStore;
        use opencode_llm_api::{LlmRequest, Model, ModelLimits, RouteClient};
        use opencode_provider_adapters::protocols::openai::chat::OpenAIChatProtocol;
        use opencode_provider_adapters::route::{Endpoint, ProtocolRoute};
        use opencode_provider_adapters::transport::executor::HttpExecutor;
        use opencode_provider_adapters::transport::http::HttpJsonTransport;
        use opencode_schema::session_message::Message;
        use opencode_session_api::SessionID;
        use opencode_session_runtime::compaction::Settings;
        use opencode_session_runtime::projection::LocalSessionEvents;
        use opencode_session_runtime::runner::ports::{Compaction, CompactionInput};
        use opencode_storage_api::{Database, DatabaseExt, TxOptions};
        use opencode_storage_sqlite::SqliteDatabase;
        use serde_json::{Value, json};

        let root = tempfile::tempdir().unwrap();
        let database: Arc<dyn Database> = Arc::new(
            SqliteDatabase::open(root.path().join("opencode.db"))
                .await
                .unwrap(),
        );
        database.transaction(TxOptions::default(), |tx| {
            tx.execute("INSERT OR IGNORE INTO project (id, worktree, sandboxes, time_created, time_updated) VALUES ('global', '/project', '[]', 0, 0)", &[])?;
            tx.execute("INSERT INTO session (id, project_id, slug, directory, title, version, time_created, time_updated) VALUES ('ses_compaction_adapter', 'global', 'test', '/project', 'test', 'test', 1, 1)", &[])
        }).await.unwrap();
        let events = Arc::new(LocalSessionEvents::install(
            database.clone(),
            Arc::new(SqlEventStore::new(database.clone())),
        ));
        let mut provider = provider::Provider::start(vec![provider::Reply::Text(
            "## Objective\n- actual summary",
        )])
        .await;
        let mut route = ProtocolRoute::new(
            "local-summary",
            Arc::new(OpenAIChatProtocol),
            Endpoint::new(Some(&provider.url), "/chat/completions"),
            Arc::new(HttpJsonTransport::sse(HttpExecutor::with_client(
                reqwest::Client::builder().no_proxy().build().unwrap(),
            ))),
        );
        route.defaults.limits = Some(ModelLimits {
            context: Some(100_000),
            output: Some(2_000),
        });
        let request = LlmRequest::new(Model::new("local-test", "local", Arc::new(route)));
        let entries: Vec<(i64, Message)> = vec![
            (1, serde_json::from_value(json!({"id":"msg_old","type":"user","text":"a".repeat(100),"time":{"created":1}})).unwrap()),
            (2, serde_json::from_value(json!({"id":"msg_new","type":"user","text":"hi","time":{"created":2}})).unwrap()),
        ];
        let session = SessionID::new("ses_compaction_adapter").unwrap();
        let compaction = SessionCompaction {
            events,
            llm: Arc::new(RouteClient),
            settings: Settings {
                auto: false,
                tokens: 3.0,
                ..Settings::default()
            },
        };
        let input = CompactionInput {
            session_id: &session,
            entries: &entries,
            model: &request.model,
            request: &request,
        };
        assert!(!compaction.compact_if_needed(&input).await.unwrap());
        assert!(
            tokio::time::timeout(
                Duration::from_secs(5),
                compaction.compact_after_overflow(&input)
            )
            .await
            .unwrap()
            .unwrap()
        );
        let (headers, sent) =
            tokio::time::timeout(Duration::from_secs(2), provider.requests.recv())
                .await
                .unwrap()
                .unwrap();
        assert!(headers.starts_with("POST /v1/chat/completions "));
        assert_eq!(sent["model"], "local-test");
        assert!(
            sent["messages"]
                .to_string()
                .contains("Create a new anchored summary")
        );
        let rows = database.read(|tx| tx.query("SELECT type, data FROM event WHERE aggregate_id = 'ses_compaction_adapter' ORDER BY seq", &[])).await.unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows[0].get::<String>("type").unwrap(),
            "session.next.compaction.started.1"
        );
        assert_eq!(
            rows[1].get::<String>("type").unwrap(),
            "session.next.compaction.ended.1"
        );
        let summary: Value = rows[1].get_json("data").unwrap();
        assert_eq!(summary["text"], "## Objective\n- actual summary");
        assert_eq!(summary["recent"], "[User]: hi");
        let projected = database
            .read(|tx| {
                tx.query(
                    "SELECT data FROM session_message WHERE session_id = 'ses_compaction_adapter'",
                    &[],
                )
            })
            .await
            .unwrap();
        assert_eq!(projected.len(), 1);
        assert_eq!(
            projected[0].get_json::<Value>("data").unwrap()["summary"],
            "## Objective\n- actual summary"
        );
        provider.stop().await;
    }

    #[tokio::test]
    async fn native_snapshots_preserve_real_changes_and_best_effort_failures() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().canonicalize().unwrap();
        let home = directory.join("home");
        let project = directory.join("project");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(project.join("scope")).unwrap();
        let env = BTreeMap::from([
            ("HOME".into(), home.display().to_string()),
            (
                "XDG_CONFIG_HOME".into(),
                home.join(".config").display().to_string(),
            ),
            (
                "GIT_CONFIG_GLOBAL".into(),
                home.join(".gitconfig").display().to_string(),
            ),
            ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
        ]);
        let git = GitCli::default().with_env(env);
        assert!(
            git.exec(&project, &["init", "-q"], None, &[])
                .await
                .unwrap()
                .ok()
        );
        std::fs::write(project.join("scope/tracked.txt"), "before\n").unwrap();
        std::fs::write(project.join("outside.txt"), "outside\n").unwrap();
        assert!(
            git.exec(&project, &["add", "."], None, &[])
                .await
                .unwrap()
                .ok()
        );
        assert!(
            git.exec(
                &project,
                &[
                    "-c",
                    "user.name=Test",
                    "-c",
                    "user.email=test@opencode.test",
                    "-c",
                    "commit.gpgsign=false",
                    "commit",
                    "-q",
                    "-m",
                    "initial"
                ],
                None,
                &[]
            )
            .await
            .unwrap()
            .ok()
        );
        let config = SnapshotConfig {
            data_dir: directory.join("data"),
            project_id: "project-id".into(),
            project_directory: project.clone(),
            directory: project.join("scope"),
            vcs_git: true,
            enabled: true,
        };
        let snapshots = RunnerSnapshots(
            NativeSnapshots::open(NativeGit::new(git.clone()), config.clone()).await,
        );
        let before = snapshots.capture().await.expect("real initial tree");
        std::fs::write(project.join("scope/tracked.txt"), "after\n").unwrap();
        std::fs::write(project.join("scope/added.txt"), "new\n").unwrap();
        std::fs::write(project.join("outside.txt"), "outside changed\n").unwrap();
        let after = snapshots.capture().await.expect("real changed tree");
        assert_ne!(before, after);
        assert_eq!(
            snapshots
                .files(&before, &after)
                .await
                .unwrap()
                .iter()
                .map(|path| path.as_str())
                .collect::<Vec<_>>(),
            ["scope/added.txt", "scope/tracked.txt"],
        );
        assert_eq!(snapshots.files("invalid-tree", &after).await, None);
        let disabled = RunnerSnapshots(
            NativeSnapshots::open(
                NativeGit::new(git),
                SnapshotConfig {
                    enabled: false,
                    ..config
                },
            )
            .await,
        );
        assert_eq!(disabled.capture().await, None);
    }
}
