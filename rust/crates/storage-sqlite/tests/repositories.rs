//! Purpose: conformance tests for the baseline table repositories: Drizzle encodings (JSON mode,
//! boolean mode, path columns, `$onUpdate` timestamps), conflict behavior, ordering and the
//! transactional flows of project.ts, directories.ts, permission/saved.ts and
//! control-plane/workspace.ts. Accounts and shares are in repositories_accounts.rs.
//! Scope: live tests on a migrated in-memory `SqliteDatabase` (real schema, no fixtures).
//! Uses:
//! rust/crates/
//! |-- storage-api/src/lib.rs
//! |-- storage-sqlite/src/lib.rs
//! `-- storage-sqlite/tests/support/mod.rs
//! External:
//! |-- serde_json
//! `-- tokio
//! Used by:
//! `-- none, entrypoint
//! Invariants: every assertion names the TS behavior it pins.
//! Contract tests: storage-sqlite/tests/repositories.rs

mod support;

use opencode_storage_api::records::{
    ControlAccountRow, Crud, ProjectRow, SessionRow, TableRecord, WorkspaceRow,
};
use opencode_storage_api::{DatabaseExt, StorageError, TxOptions, Value};
use opencode_storage_sqlite::repositories::{permission, project, project_directory, workspace};
use serde_json::json;
use support::{R, db, project_row, raw, session_row, tx};

#[test]
fn records_follow_drizzle_column_order() {
    assert_eq!(ProjectRow::COLUMNS.len(), 12);
    assert_eq!(SessionRow::COLUMNS.len(), 29);
    assert_eq!(WorkspaceRow::COLUMNS[1], "type");
    assert_eq!(ControlAccountRow::PRIMARY_KEY, ["email", "url"]);
}

// Drizzle JSON mode is compact JSON.stringify, absoluteArrayColumn stores a JSON array of paths.
#[tokio::test]
async fn project_round_trips_with_drizzle_encodings() {
    let db = db().await;
    let (stored, back) = tx(&db, |sql| {
        project::insert(sql, &project_row("prj_a"))?;
        Ok((
            raw(
                sql,
                "SELECT sandboxes, commands, time_initialized FROM project",
            ),
            project::get(sql, "prj_a")?,
        ))
    })
    .await
    .unwrap();
    assert_eq!(
        stored[0],
        [
            Value::Text("[\"/work/sb\"]".into()),
            Value::Text("{\"start\":\"bun dev\"}".into()),
            Value::Null
        ]
    );
    assert_eq!(back, Some(project_row("prj_a")));
}

// absoluteColumn.toDriver throws "Path is not absolute" for relative paths.
#[tokio::test]
async fn relative_paths_are_rejected_before_writing() {
    let db = db().await;
    let mut row = project_row("prj_rel");
    row.worktree = "relative/dir".into();
    let error = tx(&db, move |sql| project::insert(sql, &row))
        .await
        .unwrap_err();
    assert!(
        matches!(error, StorageError::Validation(ref m) if m == "Path is not absolute: relative/dir")
    );
    let error = tx(&db, |sql| {
        project::insert(sql, &project_row("prj_a"))?;
        project_directory::create(
            sql,
            "prj_a",
            "rel",
            None,
            project_directory::Behavior::Ignore,
        )
    })
    .await
    .unwrap_err();
    assert!(matches!(error, StorageError::Validation(_)));
}

// fromDirectory upsert: undefined optionals do not overwrite, time_created is kept.
#[tokio::test]
async fn project_upsert_keeps_unset_columns() {
    let db = db().await;
    let after = tx(&db, |sql| {
        let mut first = project_row("prj_a");
        first.name = Some("Named".into());
        first.time_initialized = Some(9);
        project::insert(sql, &first)?;
        project::upsert(
            sql,
            &project::Upsert {
                id: "prj_a".into(),
                worktree: "/other".into(),
                vcs: None,
                name: None,
                icon_url: Some("https://icon".into()),
                icon_url_override: None,
                icon_color: None,
                time_created: 100,
                time_updated: 200,
                time_initialized: None,
                sandboxes: vec![],
                commands: None,
            },
        )?;
        project::get(sql, "prj_a")
    })
    .await
    .unwrap()
    .unwrap();
    assert_eq!(after.worktree, "/other");
    assert_eq!(after.vcs, None, "vcs is always set (`?? null`)");
    assert_eq!(after.name.as_deref(), Some("Named"));
    assert_eq!(after.icon_url.as_deref(), Some("https://icon"));
    assert_eq!(after.time_created, 1);
    assert_eq!(after.time_updated, 200);
    assert_eq!(after.time_initialized, Some(9));
    assert!(after.sandboxes.is_empty());
    assert_eq!(after.commands, Some(json!({"start": "bun dev"})));
}

// Project.update returns the row (or None) and bumps time_updated. setInitialized also bumps it ($onUpdate).
#[tokio::test]
async fn project_update_sandboxes_and_initialized() {
    let db = db().await;
    let (updated, missing, sandboxes, initialized) = tx(&db, |sql| {
        project::insert(sql, &project_row("prj_a"))?;
        let updated = project::update(
            sql,
            "prj_a",
            &project::Update {
                name: Some("New".into()),
                ..Default::default()
            },
        )?
        .unwrap();
        let missing = project::update(sql, "nope", &project::Update::default())?;
        project::add_sandbox(sql, "prj_a", "/work/sb")?;
        project::add_sandbox(sql, "prj_a", "/work/sb2")?;
        let sandboxes = project::remove_sandbox(sql, "prj_a", "/work/sb")?
            .unwrap()
            .sandboxes;
        project::set_initialized(sql, "prj_a")?;
        Ok((
            updated,
            missing,
            sandboxes,
            project::get(sql, "prj_a")?.unwrap(),
        ))
    })
    .await
    .unwrap();
    assert_eq!(updated.name.as_deref(), Some("New"));
    assert!(updated.time_updated > 1_000_000_000_000);
    assert_eq!(updated.commands, Some(json!({"start": "bun dev"})));
    assert_eq!(missing, None);
    assert_eq!(sandboxes, ["/work/sb2"]);
    assert!(initialized.time_initialized.is_some());
    assert!(initialized.time_updated >= updated.time_updated);
}

// Project.migrateID: copy, clear directories, move sessions (keeping time_updated) and workspaces.
#[tokio::test]
async fn project_migrate_id_moves_children() {
    let db = db().await;
    let (projects, dirs, session, ws) = db
        .transaction(TxOptions::immediate(), |sql| -> R<_> {
            project::insert(sql, &project_row("old"))?;
            project_directory::create(sql, "old", "/work/tree", None, Default::default())?;
            Crud::<SessionRow>::insert(sql, &session_row("ses_1", "old", "/work/tree"))?;
            workspace::insert(
                sql,
                &WorkspaceRow {
                    id: "wrk_1".into(),
                    kind: "worktree".into(),
                    name: "".into(),
                    branch: None,
                    directory: None,
                    extra: None,
                    project_id: "old".into(),
                    time_used: 1,
                },
            )?;
            project::migrate_id(sql, "old", "new")?;
            Ok((
                project::list(sql)?
                    .into_iter()
                    .map(|p| p.id)
                    .collect::<Vec<_>>(),
                project_directory::list(sql, "old")?,
                Crud::<SessionRow>::get(sql, &["ses_1".into()])?.unwrap(),
                workspace::get(sql, "wrk_1")?.unwrap(),
            ))
        })
        .await
        .unwrap();
    assert_eq!(projects, ["new"]);
    assert!(dirs.is_empty());
    assert_eq!(
        (session.project_id.as_str(), session.time_updated),
        ("new", 6)
    );
    assert_eq!(ws.project_id, "new");
}

// fromDirectory moves global sessions of the directory and bumps their time_updated.
#[tokio::test]
async fn adopt_global_sessions_only_matches_directory() {
    let db = db().await;
    let rows = tx(&db, |sql| {
        project::insert(sql, &project_row("global"))?;
        project::insert(sql, &project_row("prj_a"))?;
        Crud::<SessionRow>::insert(sql, &session_row("ses_in", "global", "/work/tree"))?;
        Crud::<SessionRow>::insert(sql, &session_row("ses_out", "global", "/elsewhere"))?;
        Crud::<SessionRow>::insert(sql, &session_row("ses_empty", "global", ""))?;
        let moved = project::adopt_global_sessions(sql, "prj_a", "/work/tree")?;
        Ok((
            moved,
            Crud::<SessionRow>::list(sql, None, &[], Some("\"id\""))?,
        ))
    })
    .await
    .unwrap();
    assert_eq!(rows.0, 1);
    let by_id: Vec<(String, String)> = rows.1.into_iter().map(|s| (s.id, s.project_id)).collect();
    assert_eq!(
        by_id,
        [
            ("ses_empty".into(), "global".into()),
            ("ses_in".into(), "prj_a".into()),
            ("ses_out".into(), "global".into())
        ]
    );
}

// directories.ts: ignore vs replace conflict behavior and list ordering.
#[tokio::test]
async fn project_directory_create_replace_and_order() {
    use project_directory::Behavior::{Ignore, Replace};
    let db = db().await;
    let out = tx(&db, |sql| {
        project::insert(sql, &project_row("p"))?;
        let created = project_directory::create(sql, "p", "/b", None, Ignore)?;
        let dup = project_directory::create(sql, "p", "/b", Some("x"), Ignore)?;
        let replaced = project_directory::create(sql, "p", "/b", Some("x"), Replace)?;
        let same = project_directory::create(sql, "p", "/b", Some("x"), Replace)?;
        let cleared = project_directory::create(sql, "p", "/b", None, Replace)?;
        let cleared_again = project_directory::create(sql, "p", "/b", None, Replace)?;
        sql.execute("UPDATE project_directory SET time_created = 5", &[])?;
        project_directory::create(sql, "p", "/a", Some("s"), Ignore)?;
        project_directory::create(sql, "p", "/c", None, Ignore)?;
        sql.execute(
            "UPDATE project_directory SET time_created = 5 WHERE directory = '/c'",
            &[],
        )?;
        let list = project_directory::list(sql, "p")?;
        let removed = project_directory::remove(sql, "p", "/a")?;
        let removed_again = project_directory::remove(sql, "p", "/a")?;
        let contains = project_directory::contains(sql, "p", "/b")?;
        Ok((
            [created, dup, replaced, same, cleared, cleared_again],
            list,
            removed,
            removed_again,
            contains,
        ))
    })
    .await
    .unwrap();
    assert_eq!(out.0, [true, false, true, false, true, false]);
    let order: Vec<(String, Option<String>)> = out
        .1
        .into_iter()
        .map(|d| (d.directory, d.strategy))
        .collect();
    assert_eq!(
        order,
        [
            ("/a".into(), Some("s".into())),
            ("/b".into(), None),
            ("/c".into(), None)
        ]
    );
    assert!(out.2 && !out.3 && out.4);
}

// PermissionSaved.add: psv_ ids, onConflictDoNothing on the unique index, list filter.
#[tokio::test]
async fn permission_add_skips_duplicates() {
    let db = db().await;
    let (rows, all) = tx(&db, |sql| {
        project::insert(sql, &project_row("p1"))?;
        project::insert(sql, &project_row("p2"))?;
        permission::add(
            sql,
            "p1",
            "bash",
            &["ls".into(), "ls".into(), "git *".into()],
        )?;
        permission::add(sql, "p1", "bash", &["ls".into()])?;
        permission::add(sql, "p2", "edit", &["*".into()])?;
        permission::add(sql, "p2", "edit", &[])?;
        let rows = permission::list(sql, Some("p1"))?;
        permission::remove(sql, &rows[0].id)?;
        Ok((rows, permission::list(sql, None)?))
    })
    .await
    .unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|r| r.id.starts_with("psv_")));
    assert!(rows[0].time_created > 0 && rows[0].time_created == rows[0].time_updated);
    assert_eq!(all.len(), 2);
}

// Workspace.list sorts with localeCompare, not byte order.
#[tokio::test]
async fn workspace_list_uses_locale_order() {
    let db = db().await;
    let ids = tx(&db, |sql| {
        project::insert(sql, &project_row("p"))?;
        for id in ["wrk_B", "wrk_a", "wrk_-x", "wrk_b"] {
            workspace::insert(
                sql,
                &WorkspaceRow {
                    id: id.into(),
                    kind: "worktree".into(),
                    name: id.into(),
                    branch: Some("main".into()),
                    directory: Some("relative ok".into()),
                    extra: Some(json!({"k": 1})),
                    project_id: "p".into(),
                    time_used: 3,
                },
            )?;
        }
        let ids: Vec<String> = workspace::list(sql, "p")?
            .into_iter()
            .map(|w| w.id)
            .collect();
        workspace::remove(sql, "wrk_a")?;
        Ok((ids, workspace::get(sql, "wrk_a")?))
    })
    .await
    .unwrap();
    assert_eq!(ids.0, ["wrk_-x", "wrk_a", "wrk_b", "wrk_B"]);
    assert_eq!(ids.1, None);
}

// Session directoryColumn keeps legacy empty directories readable; pathColumn is stored verbatim on posix.
#[tokio::test]
async fn session_row_path_columns() {
    let db = db().await;
    let row = tx(&db, |sql| {
        project::insert(sql, &project_row("p"))?;
        let mut session = session_row("ses_1", "p", "");
        session.path = Some("sub\\dir".into());
        session.cost = 0.25;
        session.model = Some(json!({"id": "m", "providerID": "p"}));
        Crud::<SessionRow>::insert(sql, &session)?;
        Crud::<SessionRow>::get(sql, &["ses_1".into()])
    })
    .await
    .unwrap()
    .unwrap();
    assert_eq!(row.directory, "");
    let expected = if cfg!(windows) { "sub/dir" } else { "sub\\dir" };
    assert_eq!(row.path.as_deref(), Some(expected));
    assert_eq!(row.cost, 0.25);
}

// Foreign keys stay enforced through repositories (ON DELETE CASCADE from project).
#[tokio::test]
async fn deleting_project_cascades() {
    let db = db().await;
    let left = tx(&db, |sql| {
        project::insert(sql, &project_row("p"))?;
        permission::add(sql, "p", "bash", &["ls".into()])?;
        project_directory::create(sql, "p", "/d", None, Default::default())?;
        Crud::<ProjectRow>::delete(sql, &["p".into()])?;
        Ok((
            permission::list(sql, None)?.len(),
            project_directory::list(sql, "p")?.len(),
        ))
    })
    .await
    .unwrap();
    assert_eq!(left, (0, 0));
    let error = tx(&db, |sql| {
        permission::add(sql, "missing", "bash", &["ls".into()])
    })
    .await
    .unwrap_err();
    assert!(matches!(error, StorageError::ForeignKey(_)));
}
