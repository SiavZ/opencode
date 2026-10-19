//! Purpose: conformance tests for the data-moving migrations (backfills, beta resets, path
//! normalization), ported from packages/core/test/database-migration.test.ts.
//! Scope: live tests on in-memory SQLite databases.
//! Uses:
//! rust/crates/
//! |-- storage-sqlite/src/migration.rs
//! `-- storage-sqlite/tests/common/mod.rs
//! External:
//! `-- serde_json::to_string
//! Used by:
//! `-- none, entrypoint
//! Invariants: each test names the TS test it mirrors.
//! Contract tests: storage-sqlite/tests/migration_data.rs

mod common;

use common::{json, mem, one, strings};
use opencode_storage_sqlite::migration::{apply, apply_only};

// "backfills existing Context Epoch rows to the build agent"
#[test]
fn backfills_context_epoch_agent() {
    let conn = mem();
    conn.execute_batch(
        "CREATE TABLE session_context_epoch (session_id text PRIMARY KEY, baseline text NOT NULL, snapshot text NOT NULL, baseline_seq integer NOT NULL, replacement_seq integer, revision integer DEFAULT 0 NOT NULL);
         INSERT INTO session_context_epoch (session_id, baseline, snapshot, baseline_seq) VALUES ('ses_existing', 'baseline', '{}', 0);",
    )
    .unwrap();
    apply_only(&conn, &one("20260605042240_add_context_epoch_agent")).unwrap();
    assert_eq!(
        strings(
            &conn,
            "SELECT agent FROM session_context_epoch WHERE session_id = 'ses_existing'"
        ),
        ["build"]
    );
}

// "keeps legacy credential fields nullable"
#[test]
fn keeps_legacy_credential_fields_nullable() {
    let conn = mem();
    conn.execute_batch(
        "CREATE TABLE credential (id text PRIMARY KEY, connector_id text NOT NULL, method_id text NOT NULL, label text NOT NULL, value text NOT NULL, active integer DEFAULT false NOT NULL, time_created integer NOT NULL, time_updated integer NOT NULL);
         CREATE UNIQUE INDEX credential_connector_active_idx ON credential (connector_id) WHERE active = 1;",
    )
    .unwrap();
    apply_only(&conn, &one("20260611192811_lush_chimera")).unwrap();
    conn.execute_batch(
        "INSERT INTO credential (id, connector_id, method_id, label, value, active, time_created, time_updated) VALUES ('legacy', 'openai', 'oauth', 'Legacy', '{}', 1, 1, 1);
         INSERT INTO credential (id, integration_id, label, value, time_created, time_updated) VALUES ('current', 'anthropic', 'Current', '{}', 2, 2);",
    )
    .unwrap();
    assert_eq!(
        json(
            &conn,
            "SELECT connector_id, method_id, active FROM credential WHERE id = 'current'"
        ),
        serde_json::json!([{"connector_id": null, "method_id": null, "active": null}])
    );
}

// "resets beta history and rebuilds event-sourced Session input storage"
#[test]
fn resets_beta_history_and_rebuilds_session_input() {
    let conn = mem();
    conn.execute_batch(
        "CREATE TABLE session (id text PRIMARY KEY, workspace_id text);
         CREATE TABLE workspace (id text PRIMARY KEY);
         CREATE TABLE message (id text PRIMARY KEY);
         CREATE TABLE part (id text PRIMARY KEY);
         CREATE TABLE event_sequence (aggregate_id text PRIMARY KEY, seq integer NOT NULL);
         CREATE TABLE event (id text PRIMARY KEY, aggregate_id text NOT NULL, seq integer NOT NULL, type text NOT NULL, data text NOT NULL);
         CREATE INDEX event_aggregate_seq_idx ON event (aggregate_id, seq);
         CREATE INDEX event_aggregate_type_seq_idx ON event (aggregate_id, type, seq);
         CREATE TABLE session_message (id text PRIMARY KEY, session_id text NOT NULL, type text NOT NULL, seq integer NOT NULL, time_created integer NOT NULL, time_updated integer NOT NULL, data text NOT NULL);
         CREATE INDEX session_message_session_seq_idx ON session_message (session_id, seq);
         CREATE TABLE session_input (seq integer PRIMARY KEY AUTOINCREMENT, id text NOT NULL UNIQUE, session_id text NOT NULL, prompt text NOT NULL, delivery text NOT NULL, promoted_seq integer, time_created integer NOT NULL);
         CREATE INDEX session_input_session_pending_delivery_seq_idx ON session_input (session_id, promoted_seq, delivery, seq);
         INSERT INTO session (id, workspace_id) VALUES ('session', 'wrk_old');
         INSERT INTO workspace (id) VALUES ('wrk_old');
         INSERT INTO message (id) VALUES ('message');
         INSERT INTO part (id) VALUES ('part');
         INSERT INTO event_sequence (aggregate_id, seq) VALUES ('session', 0);
         INSERT INTO event (id, aggregate_id, seq, type, data) VALUES ('evt_old', 'session', 0, 'old.1', '{}');
         INSERT INTO session_message (id, session_id, type, seq, time_created, time_updated, data) VALUES ('msg_old', 'session', 'user', 0, 1, 1, '{}');
         INSERT INTO session_input (id, session_id, prompt, delivery, time_created) VALUES ('msg_pending', 'session', '{}', 'steer', 1);",
    )
    .unwrap();
    apply_only(&conn, &one("20260604172448_event_sourced_session_input")).unwrap();
    assert_eq!(
        json(&conn, "SELECT id, workspace_id FROM session"),
        serde_json::json!([{"id": "session", "workspace_id": null}])
    );
    for (table, expected) in [
        ("workspace", 0),
        ("message", 1),
        ("part", 1),
        ("event", 0),
        ("session_message", 0),
        ("session_input", 0),
    ] {
        assert_eq!(
            strings(&conn, &format!("SELECT count(*) || '' FROM {table}")),
            [expected.to_string()],
            "{table}"
        );
    }
    assert!(strings(&conn, "SELECT aggregate_id FROM event_sequence").is_empty());
    assert_eq!(
        strings(&conn, "SELECT name FROM pragma_table_info('session_input')"),
        [
            "id",
            "session_id",
            "prompt",
            "delivery",
            "admitted_seq",
            "promoted_seq",
            "time_created"
        ]
    );
    let unique = |table: &str, index: &str| {
        strings(
            &conn,
            &format!(
                "SELECT \"unique\" || '' FROM pragma_index_list('{table}') WHERE name = '{index}'"
            ),
        )
    };
    assert_eq!(
        unique("session_message", "session_message_session_seq_idx"),
        ["1"]
    );
    assert_eq!(unique("event", "event_aggregate_seq_idx"), ["1"]);
    assert_eq!(
        unique("session_input", "session_input_session_admitted_seq_idx"),
        ["1"]
    );
    assert_eq!(
        unique("session_input", "session_input_session_promoted_seq_idx"),
        ["1"]
    );
}

// "preserves canonical V1 state and restarts its event stream" (storage half: the publish half
// belongs to event-store and Phase 04 projectors).
#[test]
fn simplify_session_input_preserves_v1_state() {
    let conn = mem();
    conn.execute_batch("PRAGMA foreign_keys = ON").unwrap();
    apply(&conn).unwrap();
    conn.execute_batch(
        "INSERT INTO project (id, worktree, time_created, time_updated, sandboxes) VALUES ('global', '/project', 1, 1, '[]');
         INSERT INTO workspace (id, type, project_id, time_used) VALUES ('workspace', 'local', 'global', 1);
         INSERT INTO session (id, project_id, workspace_id, slug, directory, title, version, time_created, time_updated) VALUES ('session', 'global', 'workspace', 'session', '/project', 'Before', 'test', 1, 1);
         INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES ('message', 'session', 1, 1, '{}');
         INSERT INTO part (id, message_id, session_id, time_created, time_updated, data) VALUES ('part', 'message', 'session', 1, 1, '{}');
         INSERT INTO event_sequence (aggregate_id, seq) VALUES ('session', 9);
         INSERT INTO event (id, aggregate_id, seq, type, data) VALUES ('event', 'session', 9, 'session.updated.1', '{}');
         INSERT INTO session_input (id, session_id, prompt, delivery, admitted_seq, time_created) VALUES ('input', 'session', '{}', 'steer', 9, 1);
         INSERT INTO session_message (id, session_id, type, seq, time_created, time_updated, data) VALUES ('projected', 'session', 'user', 9, 1, 1, '{}');
         INSERT INTO session_context_epoch (session_id, baseline, snapshot, baseline_seq) VALUES ('session', 'baseline', '{}', 9);
         DELETE FROM migration WHERE id = '20260622202450_simplify_session_input';",
    )
    .unwrap();
    apply_only(&conn, &one("20260622202450_simplify_session_input")).unwrap();
    assert_eq!(
        json(
            &conn,
            "SELECT (SELECT title FROM session WHERE id = 'session') AS title,
                    (SELECT workspace_id FROM session WHERE id = 'session') AS workspaceID,
                    (SELECT COUNT(*) FROM message) AS messages, (SELECT COUNT(*) FROM part) AS parts,
                    (SELECT COUNT(*) FROM workspace) AS workspaces, (SELECT COUNT(*) FROM session_input) AS sessionInputs,
                    (SELECT COUNT(*) FROM session_message) AS sessionMessages,
                    (SELECT COUNT(*) FROM session_context_epoch) AS contextEpochs,
                    (SELECT COUNT(*) FROM event) AS events"
        ),
        serde_json::json!([{"title": "Before", "workspaceID": null, "messages": 1, "parts": 1, "workspaces": 0, "sessionInputs": 0, "sessionMessages": 0, "contextEpochs": 0, "events": 0}])
    );
}

// "resets incompatible projected Session messages before adding sequence order"
#[test]
fn resets_projected_messages_before_seq() {
    let conn = mem();
    conn.execute_batch(
        "CREATE TABLE session (id text PRIMARY KEY);
         CREATE TABLE message (id text PRIMARY KEY, session_id text NOT NULL, time_created integer NOT NULL, time_updated integer NOT NULL, data text NOT NULL);
         CREATE TABLE part (id text PRIMARY KEY, message_id text NOT NULL, session_id text NOT NULL, time_created integer NOT NULL, time_updated integer NOT NULL, data text NOT NULL);
         CREATE TABLE event (id text PRIMARY KEY, seq integer NOT NULL);
         CREATE TABLE session_message (id text PRIMARY KEY, session_id text NOT NULL, type text NOT NULL, time_created integer NOT NULL, time_updated integer NOT NULL, data text NOT NULL);
         CREATE INDEX session_message_session_time_created_id_idx ON session_message (session_id, time_created, id);
         CREATE INDEX session_message_session_type_time_created_id_idx ON session_message (session_id, type, time_created, id);
         INSERT INTO session (id) VALUES ('session');
         INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES ('legacy_message', 'session', 1, 1, '{\"role\":\"user\"}');
         INSERT INTO part (id, message_id, session_id, time_created, time_updated, data) VALUES ('legacy_part', 'legacy_message', 'session', 1, 1, '{\"type\":\"text\",\"text\":\"hello\"}');
         INSERT INTO session_message (id, session_id, type, time_created, time_updated, data) VALUES ('stale_projection', 'session', 'user', 1, 1, '{}');",
    )
    .unwrap();
    apply_only(
        &conn,
        &one("20260603040000_session_message_projection_order"),
    )
    .unwrap();
    assert_eq!(
        strings(&conn, "SELECT data FROM message"),
        [r#"{"role":"user"}"#]
    );
    assert_eq!(
        strings(&conn, "SELECT data FROM part"),
        [r#"{"type":"text","text":"hello"}"#]
    );
    assert!(strings(&conn, "SELECT id FROM session_message").is_empty());
    conn.execute_batch("INSERT INTO session_message (id, session_id, type, seq, time_created, time_updated, data) VALUES ('fresh_projection', 'session', 'user', 7, 2, 2, '{}')").unwrap();
    assert_eq!(
        json(&conn, "SELECT id, seq FROM session_message"),
        serde_json::json!([{"id": "fresh_projection", "seq": 7}])
    );
}

// "runs session usage backfill in order with schema changes"
#[test]
fn session_usage_backfill() {
    let conn = mem();
    conn.execute_batch(
        r#"CREATE TABLE session (id text PRIMARY KEY, time_updated integer NOT NULL);
           CREATE TABLE message (id text PRIMARY KEY, session_id text NOT NULL, data text NOT NULL);
           INSERT INTO session (id, time_updated) VALUES ('session_1', 1);
           INSERT INTO message (id, session_id, data) VALUES ('message_1', 'session_1', '{"role":"assistant","cost":1.25,"tokens":{"input":2,"output":3,"reasoning":4,"cache":{"read":5,"write":6}}}');"#,
    )
    .unwrap();
    apply_only(&conn, &one("20260510033149_session_usage")).unwrap();
    assert_eq!(
        json(
            &conn,
            "SELECT cost, tokens_input, tokens_output, tokens_reasoning, tokens_cache_read, tokens_cache_write FROM session WHERE id = 'session_1'"
        ),
        serde_json::json!([{"cost": 1.25, "tokens_input": 2, "tokens_output": 3, "tokens_reasoning": 4, "tokens_cache_read": 5, "tokens_cache_write": 6}])
    );
}

// "normalizes Windows storage paths and leaves POSIX paths untouched"
#[test]
fn normalizes_windows_storage_paths() {
    let conn = mem();
    conn.execute_batch(
        "CREATE TABLE project (id text PRIMARY KEY, worktree text NOT NULL, sandboxes text NOT NULL);
         CREATE TABLE session (id text PRIMARY KEY, directory text NOT NULL, path text);",
    )
    .unwrap();
    let insert_project = |id: &str, worktree: &str, sandboxes: &str| {
        conn.execute(
            "INSERT INTO project (id, worktree, sandboxes) VALUES (?1, ?2, ?3)",
            [id, worktree, sandboxes],
        )
        .unwrap();
    };
    insert_project(
        "win",
        r"C:\Repo\Thing",
        &serde_json::to_string(&[r"C:\Repo\Thing\sandbox"]).unwrap(),
    );
    insert_project(
        "unc",
        r"\\server\share",
        &serde_json::to_string(&[r"\\server\share\sandbox"]).unwrap(),
    );
    insert_project("global", "/", "[]");
    conn.execute(
        "INSERT INTO session (id, directory, path) VALUES ('win', ?1, ?2)",
        [r"C:\Repo\Thing\packages\api", r"packages\api"],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO session (id, directory, path) VALUES ('posix', ?1, ?2)",
        [r"/home/me/we\ird", r"src\weird"],
    )
    .unwrap();
    apply_only(&conn, &one("20260601010001_normalize_storage_paths")).unwrap();
    assert_eq!(
        json(
            &conn,
            "SELECT worktree, sandboxes FROM project WHERE id = 'win'"
        ),
        serde_json::json!([{"worktree": "C:/Repo/Thing", "sandboxes": "[\"C:/Repo/Thing/sandbox\"]"}])
    );
    assert_eq!(
        json(
            &conn,
            "SELECT directory, path FROM session WHERE id = 'win'"
        ),
        serde_json::json!([{"directory": "C:/Repo/Thing/packages/api", "path": "packages/api"}])
    );
    assert_eq!(
        json(
            &conn,
            "SELECT worktree, sandboxes FROM project WHERE id = 'unc'"
        ),
        serde_json::json!([{"worktree": "//server/share", "sandboxes": "[\"//server/share/sandbox\"]"}])
    );
    assert_eq!(
        strings(&conn, "SELECT worktree FROM project WHERE id = 'global'"),
        ["/"]
    );
    assert_eq!(
        json(
            &conn,
            "SELECT directory, path FROM session WHERE id = 'posix'"
        ),
        serde_json::json!([{"directory": r"/home/me/we\ird", "path": r"src\weird"}])
    );
}
