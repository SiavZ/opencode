//! Purpose: conformance tests for the account (+ account_state, control_account), session_share and
//! data_migration repositories, pinned to account/repo.ts, account/url.ts and share-next.ts.
//! Scope: live tests on a migrated in-memory `SqliteDatabase` (real schema, no fixtures).
//! Uses:
//! rust/crates/
//! |-- storage-api/src/lib.rs
//! |-- storage-sqlite/src/lib.rs
//! `-- storage-sqlite/tests/support/mod.rs
//! External:
//! `-- tokio
//! Used by:
//! `-- none, entrypoint
//! Invariants: every assertion names the TS behavior it pins.
//! Contract tests: storage-sqlite/tests/repositories_accounts.rs

mod support;

use opencode_storage_api::Value;
use opencode_storage_api::records::{ControlAccountRow, Crud, Secret, SessionRow};
use opencode_storage_sqlite::repositories::{account, data_migration, project, session_share};
use support::{db, project_row, raw, session_row, tx};

// account/repo.ts: persistAccount normalizes the url and activates, remove clears state.
#[tokio::test]
async fn account_persist_activate_and_remove() {
    let db = db().await;
    let (active, url, debug, after_token, after_remove) = tx(&db, |sql| {
        account::persist(
            sql,
            &account::Persist {
                id: "acc_1".into(),
                email: "a@b.c".into(),
                url: "https://Console.Example.com/api//?q=1#frag".into(),
                access_token: Secret::new("at".into()),
                refresh_token: Secret::new("rt".into()),
                expiry: 10,
                org_id: Some("org_1".into()),
            },
        )?;
        let active = account::active(sql)?.unwrap();
        let url = active.account.url.clone();
        let debug = format!("{:?}", active.account);
        account::persist_token(
            sql,
            "acc_1",
            &Secret::new("at2".into()),
            &Secret::new("rt2".into()),
            None,
        )?;
        let after_token = account::get(sql, "acc_1")?.unwrap();
        account::remove(sql, "acc_1")?;
        let state = raw(
            sql,
            "SELECT id, active_account_id, active_org_id FROM account_state",
        );
        Ok((
            active.active_org_id,
            url,
            debug,
            after_token,
            (account::active(sql)?, state),
        ))
    })
    .await
    .unwrap();
    assert_eq!(active.as_deref(), Some("org_1"));
    assert_eq!(url, "https://console.example.com/api");
    assert!(
        !debug.contains("\"at\"") && !debug.contains("\"rt\""),
        "{debug}"
    );
    assert!(debug.contains("Secret([REDACTED])"));
    assert_eq!(after_token.access_token.expose(), "at2");
    assert_eq!(after_token.token_expiry, None);
    assert_eq!(after_remove.0, None);
    assert_eq!(
        after_remove.1,
        [vec![Value::Integer(1), Value::Null, Value::Null]]
    );
}

#[test]
fn normalize_server_url_matches_ts() {
    assert_eq!(
        account::normalize_server_url("https://a.com/").unwrap(),
        "https://a.com"
    );
    assert_eq!(
        account::normalize_server_url("https://a.com:8443/x/y/").unwrap(),
        "https://a.com:8443/x/y"
    );
    assert_eq!(
        account::normalize_server_url("http://a.com:80/").unwrap(),
        "http://a.com"
    );
    assert!(account::normalize_server_url("not a url").is_err());
}

// control_account.active is integer boolean mode; Drizzle reads Number(v) === 1.
#[tokio::test]
async fn control_account_boolean_mode() {
    let db = db().await;
    let rows = tx(&db, |sql| {
        account::insert_control_account(
            sql,
            &ControlAccountRow {
                email: "a@b.c".into(),
                url: "https://x".into(),
                access_token: Secret::new("t".into()),
                refresh_token: Secret::new("r".into()),
                token_expiry: None,
                active: true,
                time_created: 1,
                time_updated: 1,
            },
        )?;
        let stored = raw(sql, "SELECT active FROM control_account");
        sql.execute("UPDATE control_account SET active = 2", &[])?;
        Ok((stored, account::control_accounts(sql)?))
    })
    .await
    .unwrap();
    assert_eq!(rows.0, [vec![Value::Integer(1)]]);
    assert!(!rows.1[0].active, "2 is not === 1");
}

// share-next.ts upsert keeps time_created, replaces id/secret/url, bumps time_updated.
#[tokio::test]
async fn session_share_upsert() {
    let db = db().await;
    let (first, second, removed) = tx(&db, |sql| {
        project::insert(sql, &project_row("p"))?;
        Crud::<SessionRow>::insert(sql, &session_row("ses_1", "p", "/w"))?;
        session_share::upsert(
            sql,
            "ses_1",
            "shr_1",
            &Secret::new("s1".into()),
            "https://u/1",
        )?;
        sql.execute(
            "UPDATE session_share SET time_created = 7, time_updated = 7",
            &[],
        )?;
        let first = session_share::get(sql, "ses_1")?.unwrap();
        session_share::upsert(
            sql,
            "ses_1",
            "shr_2",
            &Secret::new("s2".into()),
            "https://u/2",
        )?;
        let second = session_share::get(sql, "ses_1")?.unwrap();
        Ok((first, second, session_share::remove(sql, "ses_1")?))
    })
    .await
    .unwrap();
    assert_eq!(first.id, "shr_1");
    assert_eq!(
        (second.id.as_str(), second.secret.expose().as_str()),
        ("shr_2", "s2")
    );
    assert_eq!(second.time_created, 7);
    assert!(second.time_updated > 7);
    assert!(removed);
}

#[tokio::test]
async fn data_migration_marks_once() {
    let db = db().await;
    let out = tx(&db, |sql| {
        let first = data_migration::mark(sql, "m1")?;
        let t = data_migration::completed(sql, "m1")?;
        let again = data_migration::mark(sql, "m1")?;
        Ok((
            first,
            again,
            t == data_migration::completed(sql, "m1")?,
            data_migration::completed(sql, "x")?,
        ))
    })
    .await
    .unwrap();
    assert_eq!(out, (true, false, true, None));
}
