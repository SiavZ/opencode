//! Purpose: fixture acceptance for the Go model-list disabled-model query.
//! Scope: tenant isolation and soft-delete behavior using migrated local MySQL.
//! Uses:
//! rust/crates/
//! `-- hosted-adapters/src/console/tenant.rs
//! External:
//! |-- mysql_async::prelude::Queryable
//! `-- opencode_hosted_adapters::console::db::TestDatabase
//! Used by:
//! `-- none, entrypoint
//! Invariants: no provider request is made; the fixture database is unique and local.
//! Contract tests: this file.

use mysql_async::prelude::Queryable;
use opencode_hosted_adapters::console::db::TestDatabase;
use opencode_hosted_adapters::console::tenant::TenantStore;

#[tokio::test]
async fn fixture_disabled_models_are_scoped_to_live_api_key_and_live_models() {
    let fixture = TestDatabase::create("gateway_models").await.unwrap();
    let mut conn = fixture.db.conn().await.unwrap();
    conn.exec_drop(
        "INSERT INTO `workspace` (`id`, `name`) VALUES ('wrk_gateway_live', 'Gateway live'), ('wrk_gateway_other', 'Gateway other')",
        (),
    )
    .await
    .unwrap();
    conn.exec_drop(
        "INSERT INTO `key` (`id`, `workspace_id`, `user_id`, `name`, `key`) VALUES ('key_gateway_live', 'wrk_gateway_live', 'usr_gateway', 'Live', 'oc_sk_gateway_live'), ('key_gateway_other', 'wrk_gateway_other', 'usr_gateway', 'Other', 'oc_sk_gateway_other'), ('key_gateway_deleted', 'wrk_gateway_live', 'usr_gateway', 'Deleted', 'oc_sk_gateway_deleted')",
        (),
    )
    .await
    .unwrap();
    conn.exec_drop(
        "UPDATE `key` SET `time_deleted` = now() WHERE `id` = 'key_gateway_deleted'",
        (),
    )
    .await
    .unwrap();
    conn.exec_drop(
        "INSERT INTO `model` (`id`, `workspace_id`, `model`) VALUES ('mod_gateway_live', 'wrk_gateway_live', 'disabled-live'), ('mod_gateway_deleted', 'wrk_gateway_live', 'disabled-deleted'), ('mod_gateway_other', 'wrk_gateway_other', 'other-workspace')",
        (),
    )
    .await
    .unwrap();
    conn.exec_drop(
        "UPDATE `model` SET `time_deleted` = now() WHERE `id` = 'mod_gateway_deleted' AND `workspace_id` = 'wrk_gateway_live'",
        (),
    )
    .await
    .unwrap();
    drop(conn);

    let store = TenantStore::new(fixture.db.clone());
    assert_eq!(
        store
            .disabled_models_for_key("oc_sk_gateway_live")
            .await
            .unwrap(),
        Some(vec!["disabled-live".to_string()])
    );
    assert_eq!(
        store
            .disabled_models_for_key("oc_sk_gateway_other")
            .await
            .unwrap(),
        Some(vec!["other-workspace".to_string()])
    );
    assert!(
        store
            .disabled_models_for_key("oc_sk_gateway_deleted")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .disabled_models_for_key("oc_sk_gateway_missing")
            .await
            .unwrap()
            .is_none()
    );
    fixture.drop_database().await.unwrap();
}
