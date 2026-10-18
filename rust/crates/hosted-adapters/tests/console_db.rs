//! Purpose: fixture test: baseline console migrations apply cleanly to local MySQL 8 and are
//! journaled like drizzle so a second run is a no-op.
//! Scope: integration test against the local Docker MySQL (OPENCODE_P12_MYSQL_URL).
//! Uses:
//! rust/crates/
//! `-- hosted-adapters/src/console/db.rs
//! External:
//! `-- mysql_async::prelude::Queryable
//! Used by:
//! `-- none, entrypoint
//! Invariants: label fixture, never touches a non-local database.
//! Contract tests: this file.

use mysql_async::prelude::Queryable;
use opencode_hosted_adapters::console::db::{
    TestDatabase, baseline_migrations_dir, read_migrations,
};

#[tokio::test]
async fn fixture_baseline_migrations_apply_and_journal() {
    let migrations = read_migrations(&baseline_migrations_dir()).unwrap();
    assert!(migrations.len() >= 85);
    let t = TestDatabase::create("migrate").await.unwrap();
    assert_eq!(t.db.migrate(&migrations).await.unwrap(), 0);
    let mut conn = t.db.conn().await.unwrap();
    let journaled: i64 = conn
        .query_first("SELECT COUNT(*) FROM `__drizzle_migrations`")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(journaled as usize, migrations.len());
    let cols: Vec<String> = conn
        .query("SELECT column_name FROM information_schema.columns WHERE table_schema = DATABASE() AND table_name = 'workspace' ORDER BY column_name")
        .await
        .unwrap();
    assert!(cols.iter().any(|c| c == "migrated_at"));
    drop(conn);
    t.drop_database().await.unwrap();
}
