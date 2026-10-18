//! Purpose: project identity and legacy persistence tests: resolution precedence on real git repos
//! (remote hash, cached id, root commit, linked worktree shares the common dir), non-git `global`
//! fallback, and `fromDirectory` migration of sessions, workspaces and permissions to the new id.
//! Scope: live tests (real `git` in temp dirs, migrated in-memory SQLite). No fixtures.
//! Uses:
//! rust/crates/
//! |-- locations/src/lib.rs
//! `-- locations/tests/support/mod.rs
//! External:
//! `-- tokio
//! Used by:
//! `-- none, entrypoint
//! Invariants: every assertion names the baseline behavior it pins (core/src/project.ts,
//! opencode/src/project/project.ts).
//! Contract tests: locations/tests/project.rs

mod support;

use std::sync::{Arc, Mutex};

use opencode_locations::git::GitRunner;
use opencode_locations::ports::{LocationResolver, ProjectLocationResolver, location_ref};
use opencode_locations::project::{normalize_remote, remote_id};
use opencode_locations::{GitProjectResolver, ProjectResolver, ProjectStore};
use opencode_schema::ids::ProjectID;
use opencode_storage_api::Value;
use support::*;

fn resolver() -> Arc<GitProjectResolver> {
    Arc::new(GitProjectResolver::new(git()))
}

#[tokio::test]
async fn non_git_directory_resolves_to_global_at_filesystem_root() {
    let (_guard, dir) = temp();
    let nested = dir.join("a/b");
    std::fs::create_dir_all(&nested).unwrap();
    let resolved = resolver().resolve(nested.to_str().unwrap()).await.unwrap();
    assert_eq!(resolved.id, ProjectID::global());
    assert_eq!(resolved.directory.to_str().unwrap(), "/");
    assert_eq!(resolved.vcs, None);
    assert_eq!(resolved.previous, None);
}

#[tokio::test]
async fn git_repo_uses_root_commit_then_cache_then_remote() {
    let (_guard, dir) = temp();
    let root = init_repo(&dir).await;
    let sub = dir.join("src");
    std::fs::create_dir_all(&sub).unwrap();

    let resolved = resolver().resolve(sub.to_str().unwrap()).await.unwrap();
    assert_eq!(
        resolved.id.as_str(),
        root,
        "root commit when no cache and no remote"
    );
    assert_eq!(resolved.directory, dir, "git toplevel");
    assert_eq!(resolved.vcs.as_ref().unwrap().store, dir.join(".git"));

    // Cached id wins over the root commit (`remote ?? previous ?? root`).
    std::fs::write(dir.join(".git/opencode"), "  cached-id \n").unwrap();
    let cached = resolver().resolve(dir.to_str().unwrap()).await.unwrap();
    assert_eq!(cached.id.as_str(), "cached-id");
    assert_eq!(
        cached.previous.as_ref().map(|p| p.as_str()),
        Some("cached-id")
    );

    // A normalizable origin wins over everything.
    run(
        &dir,
        &["remote", "add", "origin", "git@GitHub.com:acme/repo.git"],
    )
    .await;
    let remote = resolver().resolve(dir.to_str().unwrap()).await.unwrap();
    assert_eq!(remote.id.as_str(), remote_id("github.com/acme/repo"));
    assert_eq!(
        remote.previous.as_ref().map(|p| p.as_str()),
        Some("cached-id")
    );

    // file: remotes never produce an id.
    run(&dir, &["remote", "set-url", "origin", "file:///tmp/x"]).await;
    assert_eq!(normalize_remote("file:///tmp/x"), None);
    let file = resolver().resolve(dir.to_str().unwrap()).await.unwrap();
    assert_eq!(file.id.as_str(), "cached-id");
}

#[tokio::test]
async fn unborn_repo_without_cache_falls_back_to_global_with_vcs() {
    let (_guard, dir) = temp();
    run(&dir, &["init", "-q"]).await;
    let resolved = resolver().resolve(dir.to_str().unwrap()).await.unwrap();
    assert_eq!(resolved.id, ProjectID::global());
    assert_eq!(resolved.directory, dir);
    assert!(resolved.vcs.is_some(), "still a git checkout");
}

#[tokio::test]
async fn linked_worktree_shares_common_dir_and_id() {
    let (_guard, dir) = temp();
    let main = dir.join("main");
    let root = init_repo(&main).await;
    let linked = dir.join("linked");
    run(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            linked.to_str().unwrap(),
        ],
    )
    .await;
    let repo = git().discover(&linked).await.unwrap();
    assert_eq!(repo.worktree, linked);
    assert_eq!(repo.common_directory, main.join(".git"));
    assert_ne!(repo.git_directory, repo.common_directory);
    let resolved = resolver().resolve(linked.to_str().unwrap()).await.unwrap();
    assert_eq!(resolved.id.as_str(), root);
    assert_eq!(resolved.directory, linked);
}

#[tokio::test]
async fn location_resolver_builds_location_info() {
    let (_guard, dir) = temp();
    let root = init_repo(&dir).await;
    let sub = dir.join("pkg");
    std::fs::create_dir_all(&sub).unwrap();
    let info = ProjectLocationResolver::new(resolver())
        .resolve(&location_ref(sub.to_str().unwrap(), None))
        .await
        .unwrap();
    assert_eq!(info.directory.as_str(), sub.to_str().unwrap());
    assert_eq!(info.workspace_id, None);
    assert_eq!(info.project.id.as_str(), root);
    assert_eq!(info.project.directory.as_str(), dir.to_str().unwrap());
    assert_eq!(
        serde_json::to_value(&info).unwrap(),
        serde_json::json!({
            "directory": sub.to_str().unwrap(),
            "project": { "id": root, "directory": dir.to_str().unwrap() },
        }),
        "workspaceID omitted when absent"
    );
}

#[tokio::test]
async fn from_directory_migrates_sessions_workspaces_and_permissions() {
    let (_guard, dir) = temp();
    let root = init_repo(&dir).await;
    let db = db().await;
    let worktree = dir.to_str().unwrap().to_owned();

    // Legacy state: the repo cached an old id, which owns a project row and dependents.
    std::fs::write(dir.join(".git/opencode"), "old-id").unwrap();
    insert_project(&db, "old-id", &worktree).await;
    insert_session(&db, "ses_1", "old-id", &worktree, Some("wrk_1"), None).await;
    exec(
        &db,
        "INSERT INTO workspace (id, type, name, project_id, time_used) VALUES ('wrk_1', 'worktree', 'w', 'old-id', 5)",
        vec![],
    )
    .await;
    exec(
        &db,
        "INSERT INTO project_directory (project_id, directory, time_created) VALUES ('old-id', ?1, 1)",
        vec![Value::from(format!("{worktree}/stale"))],
    )
    .await;
    exec(
        &db,
        "INSERT INTO permission (id, project_id, action, resource, time_created, time_updated) VALUES ('per_1', 'old-id', 'allow', 'bash', 1, 1)",
        vec![],
    )
    .await;
    // A global-project session in this directory is adopted.
    insert_project(&db, "global", "/").await;
    insert_session(&db, "ses_g", "global", &worktree, None, None).await;

    run(
        &dir,
        &["remote", "add", "origin", "https://github.com/acme/repo"],
    )
    .await;
    let new_id = remote_id("github.com/acme/repo");
    let updated = Arc::new(Mutex::new(Vec::new()));
    let mut store = ProjectStore::new(db.clone(), resolver());
    let hook = updated.clone();
    store.on_updated = Some(Arc::new(move |info| {
        hook.lock().unwrap().push(info.id.clone())
    }));

    let result = store.from_directory(&worktree).await.unwrap();
    assert_eq!(result.project.id.as_str(), new_id);
    assert_eq!(
        result.project.name.as_deref(),
        Some("named"),
        "old row copied to the new id"
    );
    assert_eq!(result.project.time.created.get(), 10);
    assert_eq!(result.sandbox, worktree);

    let projects = rows(&db, "SELECT id FROM project ORDER BY id").await;
    let ids: Vec<String> = projects.iter().map(|r| text(&r[0])).collect();
    assert!(
        ids.contains(&new_id) && !ids.contains(&"old-id".to_owned()),
        "{ids:?}"
    );

    let sessions = rows(
        &db,
        "SELECT id, project_id, time_updated FROM session ORDER BY id",
    )
    .await;
    assert_eq!(text(&sessions[0][1]), new_id, "ses_1 moved");
    assert_eq!(
        sessions[0][2],
        Value::Integer(77),
        "migration keeps session time_updated"
    );
    assert_eq!(
        text(&sessions[1][1]),
        new_id,
        "global session in the directory adopted"
    );

    let workspaces = rows(&db, "SELECT project_id FROM workspace").await;
    assert_eq!(text(&workspaces[0][0]), new_id, "workspaces re-pointed");

    let permissions = rows(&db, "SELECT id FROM permission").await;
    assert!(
        permissions.is_empty(),
        "saved permissions cascade with the old project row like the baseline FK"
    );

    let directories = rows(&db, "SELECT project_id, directory FROM project_directory").await;
    assert_eq!(
        directories.len(),
        1,
        "stale old directories cleared, opened directory saved"
    );
    assert_eq!(text(&directories[0][0]), new_id);
    assert_eq!(text(&directories[0][1]), worktree);

    assert_eq!(
        std::fs::read_to_string(dir.join(".git/opencode")).unwrap(),
        new_id,
        "commit writes the resolved id after persistence"
    );
    assert_eq!(updated.lock().unwrap().len(), 1);

    // Idempotent: the second open has previous == id, nothing moves.
    let again = store.from_directory(&worktree).await.unwrap();
    assert_eq!(again.project.id.as_str(), new_id);
    assert_ne!(root, new_id);
}

#[tokio::test]
async fn migrate_id_noops_for_global_missing_and_same() {
    let db = db().await;
    insert_project(&db, "global", "/").await;
    insert_project(&db, "keep", "/k").await;
    let store = ProjectStore::new(db.clone(), resolver());
    store.migrate_id(None, &ProjectID::new("x")).await.unwrap();
    store
        .migrate_id(Some(&ProjectID::global()), &ProjectID::new("x"))
        .await
        .unwrap();
    store
        .migrate_id(Some(&ProjectID::new("keep")), &ProjectID::new("keep"))
        .await
        .unwrap();
    let ids: Vec<String> = rows(&db, "SELECT id FROM project ORDER BY id")
        .await
        .iter()
        .map(|r| text(&r[0]))
        .collect();
    assert_eq!(ids, vec!["global", "keep"]);
}

#[tokio::test]
async fn migrate_into_existing_project_keeps_new_row() {
    let db = db().await;
    insert_project(&db, "old", "/old").await;
    insert_project(&db, "new", "/new").await;
    insert_session(&db, "ses_1", "old", "/old", None, None).await;
    let store = ProjectStore::new(db.clone(), resolver());
    store
        .migrate_id(Some(&ProjectID::new("old")), &ProjectID::new("new"))
        .await
        .unwrap();
    let projects = rows(&db, "SELECT id, worktree FROM project").await;
    assert_eq!(projects.len(), 1);
    assert_eq!(
        text(&projects[0][1]),
        "/new",
        "existing target row is not overwritten"
    );
    let sessions = rows(&db, "SELECT project_id FROM session").await;
    assert_eq!(text(&sessions[0][0]), "new");
}

#[tokio::test]
async fn from_directory_non_git_is_global_and_writes_no_cache() {
    let (_guard, dir) = temp();
    let db = db().await;
    let store = ProjectStore::new(db.clone(), resolver());
    let result = store.from_directory(dir.to_str().unwrap()).await.unwrap();
    assert_eq!(result.project.id, ProjectID::global());
    assert_eq!(result.project.worktree, "/");
    assert_eq!(result.project.vcs, None);
    assert_eq!(result.sandbox, "/");
    assert!(
        rows(&db, "SELECT * FROM project_directory")
            .await
            .is_empty(),
        "global directories are never saved"
    );
}

#[tokio::test]
async fn sandboxes_add_remove_and_missing_project() {
    let db = db().await;
    insert_project(&db, "p", "/p").await;
    let store = ProjectStore::new(db.clone(), resolver());
    let id = ProjectID::new("p");
    let info = store.add_sandbox(&id, "/p/sb").await.unwrap();
    assert_eq!(info.sandboxes, vec!["/p/sb"]);
    let info = store.add_sandbox(&id, "/p/sb").await.unwrap();
    assert_eq!(info.sandboxes, vec!["/p/sb"], "no duplicates");
    let info = store.remove_sandbox(&id, "/p/sb").await.unwrap();
    assert!(info.sandboxes.is_empty());
    let error = store
        .add_sandbox(&ProjectID::new("nope"), "/x")
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "Project not found: nope");
}
