//! Purpose: port of `DatabaseMigration.apply/applyOnly` (packages/core/src/database/migration.ts:15-108)
//! over the exact SQL of the 38-step baseline registry and fresh schema.
//! Scope: runs on the connection worker at open, or directly on a rusqlite connection in tests/tools.
//! Uses:
//! rust/crates/
//! |-- storage-api/src/lib.rs
//! |-- storage-sqlite/src/migrations/mod.rs
//! `-- storage-sqlite/src/transaction.rs
//! External:
//! |-- i64::MAX
//! |-- i64::try_from
//! |-- migrations::all
//! |-- migrations::schema
//! |-- rusqlite::Batch::new
//! |-- rusqlite::Connection
//! |-- rusqlite::fallible_iterator::FallibleIterator
//! |-- rusqlite::params
//! |-- transaction::begin
//! |-- transaction::commit
//! |-- transaction::query
//! `-- transaction::rollback
//! Used by:
//! rust/crates/
//! |-- storage-sqlite/src/connection.rs
//! |-- storage-sqlite/src/lib.rs
//! |-- storage-sqlite/tests/migration.rs
//! `-- storage-sqlite/tests/migration_data.rs
//! Invariants: same journal table (`migration(id, time_completed)`), same order, one transaction per
//! step with its journal row. Like bun:sqlite `query(sql).all()`, each recorded string executes only
//! its FIRST statement (so `20260423070820_add_icon_url_override` never runs its UPDATE, matching TS).
//! Never re-runs a completed id, so historical beta-reset deletes never touch migrated data again.
//! Contract tests: storage-sqlite/tests/migration.rs

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use opencode_storage_api::{Result, StorageError, Value};
use rusqlite::Connection;
use rusqlite::fallible_iterator::FallibleIterator;

use crate::migrations::{self, Migration};
use crate::transaction::{self, map_error};

/// Process-wide lock, `Semaphore.makeUnsafe(1)` in migration.ts:11.
static LOCK: Mutex<()> = Mutex::new(());

/// Apply the fresh schema to an empty database or pending migrations to an existing one.
pub fn apply(conn: &Connection) -> Result<()> {
    let _guard = LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let tables = names(
        conn,
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
    )?;
    if tables.iter().any(|name| name == "session") {
        return apply_only(conn, migrations::all());
    }
    if !tables.is_empty() {
        return Err(StorageError::Migration(
            "Database is not empty and has no session table".into(),
        ));
    }
    in_transaction(conn, || {
        for statement in migrations::schema() {
            run_first(conn, statement)?;
        }
        run_first(
            conn,
            "CREATE TABLE \"migration\" (id TEXT PRIMARY KEY, time_completed INTEGER NOT NULL)",
        )?;
        for migration in migrations::all() {
            journal(
                conn,
                migration.id,
                "INSERT INTO \"migration\" (id, time_completed) VALUES (?1, ?2)",
            )?;
        }
        Ok(())
    })
}

/// Apply `input` migrations that are not yet recorded, seeding the journal from Drizzle's first.
pub fn apply_only(conn: &Connection, input: &[Migration]) -> Result<()> {
    run_first(
        conn,
        "CREATE TABLE IF NOT EXISTS \"migration\" (id TEXT PRIMARY KEY, time_completed INTEGER NOT NULL)",
    )?;
    let mut completed = names(conn, "SELECT id FROM \"migration\"")?;
    if completed.is_empty() {
        seed_from_drizzle(conn, input)?;
        completed = names(conn, "SELECT id FROM \"migration\"")?;
    }
    for migration in input {
        if completed.iter().any(|id| id == migration.id) {
            continue;
        }
        in_transaction(conn, || {
            for statement in migration.statements(conn)? {
                run_first(conn, &statement)?;
            }
            journal(
                conn,
                migration.id,
                "INSERT INTO \"migration\" (id, time_completed) VALUES (?1, ?2)",
            )
        })?;
    }
    Ok(())
}

/// Existing installs used Drizzle's journal. Seed the new journal once so migrations don't replay.
fn seed_from_drizzle(conn: &Connection, input: &[Migration]) -> Result<()> {
    let present = transaction::query(
        conn,
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name = ?1",
        &[Value::from("__drizzle_migrations")],
    )?;
    if present.is_empty() {
        return Ok(());
    }
    let named = names(
        conn,
        "SELECT name FROM pragma_table_info('__drizzle_migrations')",
    )?
    .iter()
    .any(|c| c == "name");
    if named {
        transaction::query(
            conn,
            "INSERT INTO \"migration\" (id, time_completed) SELECT name, ?1 FROM \"__drizzle_migrations\" WHERE name IS NOT NULL",
            &[Value::Integer(now())],
        )?;
        return Ok(());
    }
    let entries = transaction::query(
        conn,
        "SELECT created_at, strftime('%Y%m%d%H%M%S', created_at / 1000, 'unixepoch') AS prefix FROM \"__drizzle_migrations\" WHERE created_at IS NOT NULL",
        &[],
    )?;
    for entry in entries {
        let prefix: Option<String> = entry.get("prefix")?;
        let wanted = format!("{}_", prefix.as_deref().unwrap_or("null"));
        let Some(migration) = input.iter().find(|m| m.id.starts_with(&wanted)) else {
            let created = entry.value("created_at")?;
            let shown = match created {
                Value::Integer(v) => v.to_string(),
                Value::Real(v) => v.to_string(),
                Value::Text(v) => v.clone(),
                other => format!("{other:?}"),
            };
            return Err(StorageError::Migration(format!(
                "Legacy migration timestamp {shown} does not match any known migration"
            )));
        };
        journal(
            conn,
            migration.id,
            "INSERT OR IGNORE INTO \"migration\" (id, time_completed) VALUES (?1, ?2)",
        )?;
    }
    Ok(())
}

fn in_transaction(conn: &Connection, body: impl FnOnce() -> Result<()>) -> Result<()> {
    transaction::begin(conn, opencode_storage_api::Behavior::Deferred)?;
    match body() {
        Ok(()) => transaction::commit(conn),
        Err(error) => {
            transaction::rollback(conn);
            Err(error)
        }
    }
}

fn journal(conn: &Connection, id: &str, sql: &str) -> Result<()> {
    conn.execute(sql, rusqlite::params![id, now()])
        .map(|_| ())
        .map_err(map_error)
}

fn names(conn: &Connection, sql: &str) -> Result<Vec<String>> {
    transaction::query(conn, sql, &[])?
        .iter()
        .map(|row| row.get_index::<String>(0))
        .collect()
}

/// Execute the first statement of `sql` and step it to completion, like bun:sqlite `query(sql).all()`.
pub(crate) fn run_first(conn: &Connection, sql: &str) -> Result<()> {
    let mut batch = rusqlite::Batch::new(conn, sql);
    if let Some(mut statement) = batch.next().map_err(map_error)? {
        let mut rows = statement.raw_query();
        while rows.next().map_err(map_error)?.is_some() {}
    }
    Ok(())
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}
