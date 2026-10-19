//! Purpose: workspace lifecycle tests: real `git worktree` create/list/remove through the built-in
//! adapter, `syncList` adoption, non-atomic create with injected host failures and cleanup by
//! `remove`, remove tolerating adapter failures, session removal ordering, typed `Unsupported`
//! defaults, the experimental flag, and the legacy routing ports.
//! Scope: live tests (real git in temp dirs, migrated in-memory SQLite, real `SqlEventStore`).
//! Failing adapters are test doubles for Phase 05/06 hosts.
//! Uses:
//! rust/crates/
//! |-- event-store/src/lib.rs
//! |-- locations/src/lib.rs
//! |-- locations/src/ports.rs
//! |-- locations/src/workspace/mod.rs
//! |-- locations/src/workspace/worktree.rs
//! |-- locations/tests/support/mod.rs
//! |-- locations/tests/support/workspace.rs
//! `-- schema/src/workspace.rs
//! External:
//! |-- serde_json::json
//! `-- tokio::test
//! Used by:
//! `-- none, entrypoint
//! Invariants: every assertion names the baseline behavior it pins (control-plane/workspace.ts,
//! adapters/worktree.ts, worktree/index.ts).
//! Contract tests: locations/tests/workspace_lifecycle.rs

mod support;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use opencode_event_store::SqlEventStore;
use opencode_locations::LocationError;
use opencode_locations::ports::{
    LegacyWorkspaces, SessionPlacement, SqlSessionPlacement, WorkspaceRouting,
};
use opencode_locations::workspace::worktree::WorktreeAdapter;
use opencode_locations::workspace::{
    AdapterContext, AdapterRegistry, Info, SyncTransport, Target, UnsupportedHost,
    UnsupportedTransport, WorkspaceAdapter, WorkspaceBus, WorkspaceOptions, WorkspaceService,
};
use opencode_schema::workspace::ConnectionState;
use serde_json::json;
use support::workspace::*;
use support::*;

#[tokio::test]
async fn worktree_adapter_create_list_remove_with_real_git() {
    let (_guard, dir) = temp();
    let repo = dir.join("repo");
    init_repo(&repo).await;
    let data = dir.join("data");
    let db = db().await;
    let bus = WorkspaceBus::default();
    let disposed = Arc::new(Mutex::new(Vec::<String>::new()));
    let sink = disposed.clone();
    let adapter = Arc::new(
        WorktreeAdapter::new(git(), data.clone(), bus.clone()).with_dispose(Arc::new(move |d| {
            sink.lock().unwrap().push(d);
            Box::pin(async {})
        })),
    );
    let adapters = AdapterRegistry::default();
    adapters.builtin("worktree", adapter.clone());
    let (service, _) = setup(
        db.clone(),
        adapters,
        Arc::new(UnsupportedTransport),
        host(),
        true,
    )
    .await;
    let ctx = context(repo.to_str().unwrap(), true);
    let mut events = bus.subscribe();

    let info = service.create(input("worktree"), &ctx).await.unwrap();
    assert!(info.id.starts_with("wrk"));
    let directory = info.directory.clone().unwrap();
    assert!(
        directory.starts_with(data.join("worktree").join(PROJECT).to_str().unwrap()),
        "worktrees live in <data>/worktree/<projectID>/<name>"
    );
    assert!(directory.ends_with(&info.name));
    assert_eq!(
        service.status(),
        vec![(info.id.clone(), ConnectionState::Connected)],
        "local target with an existing directory is connected"
    );
    adapter.wait_idle().await;
    assert!(
        std::path::Path::new(&directory).join("README.md").exists(),
        "reset --hard populated it"
    );
    let ready = loop {
        let event = events.recv().await.unwrap();
        if event.payload_type() == Some("worktree.ready") {
            break event;
        }
    };
    assert_eq!(ready.payload["properties"], json!({ "name": info.name }));

    let listed = adapter.list(&ctx).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].name, info.name);
    assert_eq!(listed[0].kind, "worktree");

    // `syncList` sees the same name and inserts nothing.
    service.sync_list(PROJECT, &ctx).await.unwrap();
    assert_eq!(service.list(PROJECT).await.unwrap().len(), 1);

    let removed = service.remove(&info.id, &ctx).await.unwrap().unwrap();
    assert_eq!(removed.id, info.id);
    assert!(
        !std::path::Path::new(&directory).exists(),
        "directory cleaned"
    );
    assert!(
        adapter.list(&ctx).await.unwrap().is_empty(),
        "git worktree removed"
    );
    assert!(
        service.get(&info.id).await.unwrap().is_none(),
        "row deleted"
    );
    assert!(
        !disposed.lock().unwrap().is_empty(),
        "location disposed before removal"
    );
    assert!(
        service.status().is_empty(),
        "stopSync forgets the connection"
    );
}

#[tokio::test]
async fn sync_list_adopts_worktrees_created_outside() {
    let (_guard, dir) = temp();
    let repo = dir.join("repo");
    init_repo(&repo).await;
    let outside = dir.join("elsewhere").join("feature");
    run(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feat",
            outside.to_str().unwrap(),
        ],
    )
    .await;
    let adapters = AdapterRegistry::default();
    adapters.builtin(
        "worktree",
        Arc::new(WorktreeAdapter::new(
            git(),
            dir.join("data"),
            WorkspaceBus::default(),
        )),
    );
    let (service, _) = setup(
        db().await,
        adapters,
        Arc::new(UnsupportedTransport),
        host(),
        true,
    )
    .await;
    let ctx = context(repo.to_str().unwrap(), true);
    service.sync_list(PROJECT, &ctx).await.unwrap();
    let list = service.list(PROJECT).await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].name, "feature");
    assert_eq!(list[0].branch.as_deref(), Some("feat"));
    assert_eq!(
        list[0].directory.as_deref(),
        Some(outside.to_str().unwrap())
    );
}

#[tokio::test]
async fn worktree_adapter_rejects_non_git_and_missing_context() {
    let adapter = WorktreeAdapter::new(git(), "/tmp/unused".into(), WorkspaceBus::default());
    let draft = Info {
        id: "wrk_1".into(),
        kind: "worktree".into(),
        branch: None,
        name: "n".into(),
        directory: None,
        extra: None,
        project_id: PROJECT.into(),
        time_used: 0,
    };
    let error = adapter
        .configure(draft.clone(), &context("/x", false))
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Worktrees are only supported for git projects"
    );
    let error = adapter
        .configure(draft, &AdapterContext::default())
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Worktree adapter requires an instance context"
    );
    assert!(
        adapter
            .list(&context("/x", false))
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn host_create_failure_leaves_row_for_remove_to_clean_up() {
    let flaky = flaky(true, false);
    let adapters = AdapterRegistry::default();
    adapters.register(PROJECT, "flaky", flaky.clone());
    let (service, _) = setup(
        db().await,
        adapters,
        Arc::new(UnsupportedTransport),
        host(),
        true,
    )
    .await;
    let ctx = AdapterContext::default();
    let error = service.create(input("flaky"), &ctx).await.unwrap_err();
    let LocationError::CreateFailed {
        workspace_id,
        source,
    } = error
    else {
        panic!("expected CreateFailed");
    };
    assert_eq!(source.to_string(), "host create failed");
    let row = service.get(&workspace_id).await.unwrap();
    assert!(
        row.is_some(),
        "create is not atomic: the row is inserted before the host"
    );
    assert!(service.status().is_empty(), "sync never started");

    service.remove(&workspace_id, &ctx).await.unwrap();
    assert!(service.get(&workspace_id).await.unwrap().is_none());
    assert_eq!(*flaky.removed.lock().unwrap(), 1);
}

#[tokio::test]
async fn host_remove_failure_is_logged_and_row_still_deleted() {
    let flaky = flaky(false, true);
    let adapters = AdapterRegistry::default();
    adapters.register(PROJECT, "flaky", flaky.clone());
    let (service, _) = setup(
        db().await,
        adapters,
        Arc::new(UnsupportedTransport),
        host(),
        true,
    )
    .await;
    let ctx = AdapterContext::default();
    let info = service.create(input("flaky"), &ctx).await.unwrap();
    assert_eq!(
        service.status(),
        vec![(info.id.clone(), ConnectionState::Error)],
        "local target whose directory is missing reports error, which ends the create wait"
    );
    let removed = service.remove(&info.id, &ctx).await.unwrap();
    assert!(removed.is_some());
    assert!(service.get(&info.id).await.unwrap().is_none());
    assert!(
        service.remove(&info.id, &ctx).await.unwrap().is_none(),
        "missing workspace"
    );
}

#[tokio::test]
async fn remove_deletes_top_level_sessions_and_ignores_not_found() {
    let flaky = flaky(false, false);
    let adapters = AdapterRegistry::default();
    adapters.register(PROJECT, "flaky", flaky);
    let host = host();
    let db = db().await;
    let (service, _) = setup(
        db.clone(),
        adapters,
        Arc::new(UnsupportedTransport),
        host.clone(),
        true,
    )
    .await;
    let ctx = AdapterContext::default();
    let info = service.create(input("flaky"), &ctx).await.unwrap();
    insert_session(&db, "ses_parent", PROJECT, "/p", Some(&info.id), None).await;
    insert_session(
        &db,
        "ses_child",
        PROJECT,
        "/p",
        Some(&info.id),
        Some("ses_parent"),
    )
    .await;
    insert_session(&db, "ses_missing", PROJECT, "/p", Some(&info.id), None).await;
    insert_session(
        &db,
        "ses_orphan",
        PROJECT,
        "/p",
        Some(&info.id),
        Some("ses_elsewhere"),
    )
    .await;
    service.remove(&info.id, &ctx).await.unwrap();
    let mut removed = host.removed.lock().unwrap().clone();
    removed.sort();
    assert_eq!(
        removed,
        vec!["ses_missing", "ses_orphan", "ses_parent"],
        "children of removed sessions are skipped, NotFound is ignored"
    );
}

#[tokio::test]
async fn defaults_are_typed_unsupported() {
    struct Bare;
    impl WorkspaceAdapter for Bare {
        fn name(&self) -> &str {
            "Bare"
        }
        fn description(&self) -> &str {
            ""
        }
    }
    let adapters = AdapterRegistry::default();
    adapters.register(PROJECT, "bare", Arc::new(Bare));
    let (service, _) = setup(
        db().await,
        adapters,
        Arc::new(UnsupportedTransport),
        Arc::new(UnsupportedHost),
        true,
    )
    .await;
    let error = service
        .create(input("bare"), &AdapterContext::default())
        .await
        .unwrap_err();
    assert!(
        matches!(error, LocationError::Unsupported { .. }),
        "{error}"
    );
    let error = service
        .create(input("nope"), &AdapterContext::default())
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "Unknown workspace adapter: nope");
    let error = UnsupportedTransport
        .connect("http://x", &[])
        .await
        .err()
        .unwrap();
    assert!(matches!(error, LocationError::Unsupported { .. }));
    assert!(service.list("other").await.unwrap().is_empty());
}

#[tokio::test]
async fn list_is_empty_without_experimental_flag() {
    let flaky = flaky(false, false);
    let adapters = AdapterRegistry::default();
    adapters.register(PROJECT, "flaky", flaky);
    let db = db().await;
    let (service, _) = setup(
        db.clone(),
        adapters.clone(),
        Arc::new(UnsupportedTransport),
        host(),
        false,
    )
    .await;
    assert!(service.list(PROJECT).await.unwrap().is_empty());

    // Without the flag `startSync` is a no-op, so no status arrives and create fails after the
    // status wait, with the row already persisted (baseline `waitEvent` timeout).
    let short = WorkspaceService::new(
        db,
        Arc::new(SqlEventStore::new(support::db().await)),
        adapters,
        WorkspaceBus::default(),
        Arc::new(UnsupportedTransport),
        host(),
        WorkspaceOptions {
            timeout: Duration::from_millis(50),
            ..WorkspaceOptions::default()
        },
    );
    let error = short
        .create(input("flaky"), &AdapterContext::default())
        .await
        .unwrap_err();
    let LocationError::CreateFailed {
        workspace_id,
        source,
    } = error
    else {
        panic!("expected CreateFailed");
    };
    assert_eq!(source.to_string(), "Timed out waiting for global event");
    assert!(short.get(&workspace_id).await.unwrap().is_some());
}

#[tokio::test]
async fn routing_ports_read_placement_and_target() {
    let db = db().await;
    let flaky = flaky(false, false);
    let adapters = AdapterRegistry::default();
    adapters.register(PROJECT, "flaky", flaky);
    let (service, _) = setup(
        db.clone(),
        adapters,
        Arc::new(UnsupportedTransport),
        host(),
        true,
    )
    .await;
    let info = service
        .create(input("flaky"), &AdapterContext::default())
        .await
        .unwrap();
    insert_session(&db, "ses_1", PROJECT, "/p/dir", Some(&info.id), None).await;
    let placement = SqlSessionPlacement::new(db.clone());
    assert_eq!(
        placement.placement("ses_1").await.unwrap(),
        Some(("/p/dir".into(), Some(info.id.clone())))
    );
    assert_eq!(placement.placement("ses_none").await.unwrap(), None);

    let routing = WorkspaceRouting::new(placement, service.clone(), AdapterContext::default());
    assert_eq!(routing.configured_workspace_id(), None);
    assert_eq!(routing.session("ses_1").await.unwrap().0, "/p/dir");
    let workspace = routing.workspace(&info.id).await.unwrap();
    assert_eq!(
        routing.target(&workspace).await,
        Target::Local {
            directory: "/nonexistent/flaky".into()
        }
    );
    assert!(
        !routing.is_syncing(&info.id).await,
        "local workspaces have no sync loop"
    );
    assert!(routing.workspace("wrk_missing").await.is_none());
}
