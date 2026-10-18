//! Purpose: migration conformance tests ported from packages/core/test/database-migration.test.ts, plus
//! full-chain checks (every id once, incremental chain reaches the fresh schema's tables and indexes).
//! Scope: live tests on in-memory and temp-file SQLite databases.
//! Uses:
//! rust/crates/
//! |-- storage-sqlite/src/lib.rs
//! `-- storage-sqlite/tests/common/mod.rs
//! External:
//! |-- rusqlite::Connection
//! `-- serde_json
//! Used by:
//! `-- none, entrypoint
//! Invariants: each test names the TS test it mirrors.
//! Contract tests: storage-sqlite/tests/migration.rs

mod common;

use common::{json, mem, one, strings};
use opencode_storage_sqlite::migration::{apply, apply_only};
use opencode_storage_sqlite::migrations;
use rusqlite::Connection;

#[test]
fn registry_matches_typescript_order_and_count() {
    let ids = migrations::ids();
    assert_eq!(ids.len(), 38);
    assert_eq!(ids.first(), Some(&"20260127222353_familiar_lady_ursula"));
    assert_eq!(ids.last(), Some(&"20260622202450_simplify_session_input"));
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    assert_eq!(ids, sorted, "registry is timestamp ordered");
    // The packages/opencode/migration journal entry is the same id as the core registry entry.
    assert!(ids.contains(&"20260511173437_session-metadata"));
}

// "defaults missing workspace names while preserving legacy workspace data"
#[test]
fn workspace_name_defaults_when_missing() {
    let conn = mem();
    conn.execute_batch(
        "CREATE TABLE workspace (id text PRIMARY KEY, type text NOT NULL, branch text, directory text, extra text, project_id text NOT NULL);
         INSERT INTO workspace (id, type, branch, directory, extra, project_id) VALUES ('wrk_legacy', 'remote', 'main', '/repo', '{}', 'proj_legacy');",
    )
    .unwrap();
    apply_only(&conn, &one("20260410174513_workspace-name")).unwrap();
    assert_eq!(
        json(
            &conn,
            "SELECT id, name, branch, directory, extra FROM workspace"
        ),
        serde_json::json!([{"id": "wrk_legacy", "name": "", "branch": "main", "directory": "/repo", "extra": "{}"}])
    );
}

#[test]
fn workspace_name_preserved_when_present() {
    let conn = mem();
    conn.execute_batch(
        "CREATE TABLE workspace (id text PRIMARY KEY, type text NOT NULL, name text, branch text, directory text, extra text, project_id text NOT NULL);
         INSERT INTO workspace (id, type, name, branch, directory, extra, project_id) VALUES ('w', 'remote', 'kept', 'main', '/repo', '{}', 'p');",
    )
    .unwrap();
    apply_only(&conn, &one("20260410174513_workspace-name")).unwrap();
    assert_eq!(strings(&conn, "SELECT name FROM workspace"), ["kept"]);
}

// "imports unnamed legacy Drizzle journal entries by their actual migration timestamps"
#[test]
fn imports_unnamed_drizzle_journal_by_timestamp() {
    let conn = mem();
    conn.execute_batch(
        "CREATE TABLE __drizzle_migrations (id integer PRIMARY KEY, hash text, created_at integer)",
    )
    .unwrap();
    // Date.UTC(2026, 3, 10, 17, 45, 13)
    conn.execute(
        "INSERT INTO __drizzle_migrations (hash, created_at) VALUES ('', ?1)",
        [1_775_843_113_000_i64],
    )
    .unwrap();
    apply_only(&conn, &one("20260410174513_workspace-name")).unwrap();
    assert_eq!(
        strings(&conn, "SELECT id FROM migration"),
        ["20260410174513_workspace-name"]
    );
}

// "rejects unknown legacy Drizzle journal timestamps instead of guessing completed migrations"
#[test]
fn rejects_unknown_drizzle_timestamp() {
    let conn = mem();
    conn.execute_batch(
        "CREATE TABLE __drizzle_migrations (id integer PRIMARY KEY, hash text, created_at integer);
         INSERT INTO __drizzle_migrations (hash, created_at) VALUES ('', 1234567890000);",
    )
    .unwrap();
    let error = apply_only(&conn, &one("20260410174513_workspace-name")).unwrap_err();
    assert_eq!(
        error.to_string(),
        "migration failed: Legacy migration timestamp 1234567890000 does not match any known migration"
    );
}

// "applies tracked migrations to an empty database"
#[test]
fn applies_fresh_schema_to_empty_database() {
    let conn = mem();
    apply(&conn).unwrap();
    assert_eq!(
        strings(&conn, "SELECT count(*) || '' FROM migration"),
        ["38"]
    );
    assert!(strings(&conn, "SELECT name FROM pragma_table_info('session_context_epoch') WHERE name IN ('agent', 'replacement_seq', 'revision')").is_empty());
    assert_eq!(
        strings(
            &conn,
            "SELECT name FROM sqlite_master WHERE type = 'index' AND name IN ('event_aggregate_seq_idx', 'event_aggregate_type_seq_idx', 'session_input_session_pending_seq_idx', 'session_input_session_pending_delivery_seq_idx', 'session_input_session_admitted_seq_idx', 'session_input_session_promoted_seq_idx', 'session_message_session_idx', 'session_message_session_type_idx', 'session_message_session_seq_idx', 'session_message_session_type_seq_idx', 'session_message_session_time_created_id_idx') ORDER BY name"
        ),
        [
            "event_aggregate_seq_idx",
            "event_aggregate_type_seq_idx",
            "session_input_session_admitted_seq_idx",
            "session_input_session_pending_delivery_seq_idx",
            "session_input_session_promoted_seq_idx",
            "session_message_session_seq_idx",
            "session_message_session_time_created_id_idx",
            "session_message_session_type_seq_idx",
        ]
    );
    let mut tables = strings(
        &conn,
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
    );
    tables.retain(|t| t != "migration");
    assert_eq!(tables.len(), 19, "{tables:?}");
    // Re-applying is a no-op.
    apply(&conn).unwrap();
    assert_eq!(
        strings(&conn, "SELECT count(*) || '' FROM migration"),
        ["38"]
    );
}

// "rejects a non-empty database without a session table"
#[test]
fn rejects_non_empty_database_without_session() {
    let conn = mem();
    conn.execute_batch("CREATE TABLE unrelated (id text PRIMARY KEY)")
        .unwrap();
    assert_eq!(
        apply(&conn).unwrap_err().to_string(),
        "migration failed: Database is not empty and has no session table"
    );
}

// "imports existing drizzle migration state"
#[test]
fn imports_named_drizzle_journal() {
    let conn = mem();
    conn.execute_batch(
        "CREATE TABLE __drizzle_migrations (id INTEGER PRIMARY KEY, hash text NOT NULL, created_at numeric, name text, applied_at TEXT);
         INSERT INTO __drizzle_migrations (hash, created_at, name, applied_at) VALUES ('hash', 1, '20260127222353_familiar_lady_ursula', '2026-01-01T00:00:00.000Z');",
    )
    .unwrap();
    apply_only(&conn, &[]).unwrap();
    assert_eq!(
        strings(&conn, "SELECT id FROM migration"),
        ["20260127222353_familiar_lady_ursula"]
    );
}

// "does not replay a migrated session metadata column"
#[test]
fn does_not_replay_session_metadata() {
    let conn = mem();
    conn.execute_batch(
        "CREATE TABLE session (id text PRIMARY KEY, metadata text);
         CREATE TABLE __drizzle_migrations (id INTEGER PRIMARY KEY, hash text NOT NULL, created_at numeric, name text, applied_at TEXT);
         INSERT INTO __drizzle_migrations (hash, created_at, name, applied_at) VALUES ('hash', 1, '20260511173437_session-metadata', 'now');",
    )
    .unwrap();
    apply_only(&conn, &one("20260511173437_session-metadata")).unwrap();
    assert_eq!(
        strings(&conn, "SELECT id FROM migration"),
        ["20260511173437_session-metadata"]
    );
}

// "accepts the temporary replacement session metadata migration id"
#[test]
fn accepts_replacement_session_metadata_id() {
    let conn = mem();
    conn.execute_batch(
        "CREATE TABLE session (id text PRIMARY KEY, metadata text);
         CREATE TABLE migration (id TEXT PRIMARY KEY, time_completed INTEGER NOT NULL);
         INSERT INTO migration (id, time_completed) VALUES ('20260530232709_lovely_romulus', 1);",
    )
    .unwrap();
    apply_only(&conn, &one("20260511173437_session-metadata")).unwrap();
    assert_eq!(
        strings(&conn, "SELECT id FROM migration ORDER BY id"),
        [
            "20260511173437_session-metadata",
            "20260530232709_lovely_romulus"
        ]
    );
}

// "skips drizzle import when migration table already has state"
#[test]
fn skips_drizzle_import_when_journal_has_state() {
    let conn = mem();
    conn.execute_batch(
        "CREATE TABLE migration (id TEXT PRIMARY KEY, time_completed INTEGER NOT NULL);
         INSERT INTO migration (id, time_completed) VALUES ('existing', 1);
         CREATE TABLE __drizzle_migrations (id INTEGER PRIMARY KEY, hash text NOT NULL, created_at numeric, name text, applied_at TEXT);
         INSERT INTO __drizzle_migrations (hash, created_at, name, applied_at) VALUES ('hash', 1, '20260127222353_familiar_lady_ursula', 'now');",
    )
    .unwrap();
    apply_only(&conn, &[]).unwrap();
    assert_eq!(
        strings(&conn, "SELECT id FROM migration ORDER BY id"),
        ["existing"]
    );
}

// Bun's query(sql).all() executes only the first statement of a string. The TS migration
// 20260423070820_add_icon_url_override therefore never runs its UPDATE; Rust must match.
#[test]
fn multi_statement_migration_runs_first_statement_only_like_bun() {
    let conn = mem();
    conn.execute_batch("CREATE TABLE project (id text PRIMARY KEY, icon_url text); INSERT INTO project VALUES ('p', 'u');")
        .unwrap();
    apply_only(&conn, &one("20260423070820_add_icon_url_override")).unwrap();
    assert_eq!(
        json(&conn, "SELECT id, icon_url, icon_url_override FROM project"),
        serde_json::json!([{"id": "p", "icon_url": "u", "icon_url_override": null}])
    );
}

// A failing step rolls back its statements and leaves its id unrecorded, and earlier steps stay committed.
#[test]
fn failing_step_is_atomic() {
    let conn = mem();
    let all = migrations::all();
    apply_only(&conn, &all[..2]).unwrap();
    // Make step 3 (control_account) fail by pre-creating the table it creates.
    conn.execute_batch("CREATE TABLE control_account (x text)")
        .unwrap();
    assert!(apply_only(&conn, &all[..3]).is_err());
    assert_eq!(
        strings(&conn, "SELECT count(*) || '' FROM migration"),
        ["2"]
    );
    // Reopen twice: same result.
    assert!(apply_only(&conn, &all[..3]).is_err());
    assert_eq!(
        strings(&conn, "SELECT count(*) || '' FROM migration"),
        ["2"]
    );
}

/// Normalized schema: `type|name|tbl_name|sql` sorted, excluding SQLite internals.
fn schema(conn: &Connection) -> Vec<String> {
    strings(
        conn,
        "SELECT type || '|' || name || '|' || tbl_name FROM sqlite_master WHERE name NOT LIKE 'sqlite_%' ORDER BY type, name",
    )
}

// Every id applies once on an incremental chain from an empty file, and the result has the same
// tables, indexes and columns as the fresh schema.
#[test]
fn incremental_chain_reaches_fresh_schema_objects() {
    let fresh = mem();
    apply(&fresh).unwrap();
    let incremental = mem();
    incremental
        .execute_batch("PRAGMA foreign_keys = ON")
        .unwrap();
    apply_only(&incremental, migrations::all()).unwrap();
    assert_eq!(
        strings(&incremental, "SELECT count(*) || '' FROM migration"),
        ["38"]
    );
    assert_eq!(schema(&fresh), schema(&incremental));
    let mut diffs = Vec::new();
    for table in strings(
        &fresh,
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
    ) {
        let columns = |conn: &Connection| {
            let mut cols = strings(
                conn,
                &format!(
                    "SELECT name || ':' || type || ':' || \"notnull\" FROM pragma_table_info('{table}')"
                ),
            );
            cols.sort();
            cols
        };
        if columns(&fresh) != columns(&incremental) {
            diffs.push(format!(
                "{table}: fresh={:?} incremental={:?}",
                columns(&fresh),
                columns(&incremental)
            ));
        }
    }
    // Baseline divergence present in TypeScript too: 20260228203230_blue_harpoon declares
    // `account_state.id integer PRIMARY KEY NOT NULL`, schema.gen.ts declares `integer PRIMARY KEY`.
    // Recorded in contracts/phase-02/leaves/sqlite.md; ts_interop.rs checks TS shows the same diff.
    assert_eq!(
        diffs,
        [
            r#"account_state: fresh=["active_account_id:TEXT:0", "active_org_id:TEXT:0", "id:INTEGER:0"] incremental=["active_account_id:TEXT:0", "active_org_id:TEXT:0", "id:INTEGER:1"]"#
        ]
    );
}
