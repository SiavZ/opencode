//! Purpose: exercise durable billing lifecycle against local migrated MySQL fixtures.
//! Scope: invoice replay, refunds, coupon credit, subscription membership and settings.
//! Uses:
//! rust/crates/
//! |-- hosted-adapters/src/console/billing_store.rs
//! |-- hosted-adapters/src/console/db.rs
//! `-- hosted-adapters/src/console/stripe.rs
//! External:
//! |-- mysql_async::prelude::Queryable
//! `-- serde_json::json
//! Used by:
//! `-- none, entrypoint
//! Invariants: isolated fixture databases only; no external Stripe calls or real payments.
//! Contract tests: this file.

use mysql_async::prelude::Queryable;
use opencode_hosted_adapters::console::billing_store::{
    BillingStore, CheckoutCredit, CreditOutcome, InvoicePayment, LiteSubscription,
};
use opencode_hosted_adapters::console::db::TestDatabase;
use opencode_hosted_adapters::console::stripe::PaymentMethod;
use serde_json::json;

#[tokio::test]
async fn fixture_invoice_refund_coupon_and_subscription_lifecycle() {
    let fixture = TestDatabase::create("commerce_lifecycle").await.unwrap();
    let mut conn = fixture.db.conn().await.unwrap();
    conn.exec_drop(
        "INSERT INTO workspace (id,name) VALUES ('wrk_lifecycle','Lifecycle')",
        (),
    )
    .await
    .unwrap();
    conn.exec_drop("INSERT INTO billing (id,workspace_id,balance,customer_id,payment_method_id,reload,reload_error,time_reload_error) VALUES ('bil_lifecycle','wrk_lifecycle',0,'cus_lifecycle','pm_lifecycle',TRUE,'keep existing failure',NOW(3))",()).await.unwrap();
    conn.exec_drop(
        "INSERT INTO coupon (email,type) VALUES ('fixture@example.test','BUILDATHON')",
        (),
    )
    .await
    .unwrap();
    drop(conn);
    let store = BillingStore::new(fixture.db.clone());
    let credit = CheckoutCredit {
        event_id: "evt_lifecycle".into(),
        workspace_id: "wrk_lifecycle".into(),
        customer_id: "cus_lifecycle".into(),
        payment_id: "pi_lifecycle".into(),
        invoice_id: "in_lifecycle".into(),
        amount_cents: 2000,
        payment_method_id: "pm_lifecycle".into(),
        payment_method_type: "card".into(),
        payment_method_last4: Some("4242".into()),
    };
    assert_eq!(
        store.complete_checkout(&credit).await.unwrap(),
        CreditOutcome::Applied
    );
    assert_eq!(
        store
            .summary("wrk_lifecycle")
            .await
            .unwrap()
            .reload_error
            .as_deref(),
        Some("keep existing failure")
    );
    let invoice = InvoicePayment {
        workspace_id: "wrk_lifecycle".into(),
        customer_id: "cus_lifecycle".into(),
        invoice_id: "in_reload".into(),
        payment_id: Some("pi_reload".into()),
        amount_cents: 1000,
        enrichment: None,
        credit_balance: true,
    };
    let (first, second) = tokio::join!(
        store.record_invoice(&invoice),
        store.record_invoice(&invoice)
    );
    assert!(matches!(
        (first.unwrap(), second.unwrap()),
        (CreditOutcome::Applied, CreditOutcome::Duplicate)
            | (CreditOutcome::Duplicate, CreditOutcome::Applied)
    ));
    assert_eq!(
        store.summary("wrk_lifecycle").await.unwrap().balance,
        3_000_000_000
    );
    assert!(
        store
            .summary("wrk_lifecycle")
            .await
            .unwrap()
            .reload_error
            .is_none()
    );
    let sub_payment = InvoicePayment {
        invoice_id: "in_subscription".into(),
        payment_id: Some("pi_subscription".into()),
        amount_cents: 500,
        enrichment: Some(json!({"type":"lite","currency":"inr","couponID":"coupon_fixture"})),
        credit_balance: false,
        ..InvoicePayment {
            workspace_id: invoice.workspace_id.clone(),
            customer_id: invoice.customer_id.clone(),
            invoice_id: String::new(),
            payment_id: None,
            amount_cents: 0,
            enrichment: None,
            credit_balance: false,
        }
    };
    store.record_invoice(&sub_payment).await.unwrap();
    store
        .refund("cus_lifecycle", "pi_subscription", 1_700_000_000)
        .await
        .unwrap();
    assert_eq!(
        store.summary("wrk_lifecycle").await.unwrap().balance,
        3_000_000_000
    );
    assert_eq!(
        store
            .refund("cus_lifecycle", "pi_reload", 1_700_000_000)
            .await
            .unwrap(),
        CreditOutcome::Applied
    );
    assert_eq!(
        store
            .refund("cus_lifecycle", "pi_reload", 1_700_000_000)
            .await
            .unwrap(),
        CreditOutcome::Duplicate
    );
    assert_eq!(
        store.summary("wrk_lifecycle").await.unwrap().balance,
        2_000_000_000
    );
    let wrong = InvoicePayment {
        customer_id: "cus_other".into(),
        ..invoice
    };
    assert!(store.record_invoice(&wrong).await.is_err());
    assert!(
        store
            .redeem_coupon("wrk_lifecycle", "fixture@example.test", "BAD")
            .await
            .is_err()
    );
    store
        .redeem_coupon("wrk_lifecycle", "fixture@example.test", "BUILDATHON")
        .await
        .unwrap();
    assert!(
        store
            .redeem_coupon("wrk_lifecycle", "fixture@example.test", "BUILDATHON")
            .await
            .is_err()
    );
    assert_eq!(
        store.summary("wrk_lifecycle").await.unwrap().balance,
        52_000_000_000
    );
    store
        .redeem_coupon("wrk_lifecycle", "fixture@example.test", "GO1MONTH50")
        .await
        .unwrap();
    store
        .update_reload("wrk_lifecycle", true, Some(20), Some(5))
        .await
        .unwrap();
    assert!(
        store
            .update_reload("wrk_lifecycle", true, Some(9), Some(5))
            .await
            .is_err()
    );
    store.set_monthly_limit("wrk_lifecycle", 0).await.unwrap();
    assert!(store.set_monthly_limit("wrk_lifecycle", -1).await.is_err());
    let subscription = LiteSubscription {
        workspace_id: "wrk_lifecycle".into(),
        user_id: "usr_lifecycle".into(),
        customer_id: "cus_lifecycle".into(),
        subscription_id: "sub_lifecycle".into(),
        payment_method: serde_json::from_value::<PaymentMethod>(
            json!({"id":"pm_lifecycle","type":"card","card":{"last4":"4242"}}),
        )
        .unwrap(),
        coupon: None,
    };
    store.subscribe_lite(&subscription).await.unwrap();
    store.subscribe_lite(&subscription).await.unwrap();
    store
        .set_use_balance("wrk_lifecycle", true, true)
        .await
        .unwrap();
    assert_eq!(
        store.summary("wrk_lifecycle").await.unwrap().lite,
        Some(json!({"useBalance":true}))
    );
    store.unsubscribe("sub_lifecycle", true).await.unwrap();
    store.unsubscribe("sub_lifecycle", true).await.unwrap();
    assert!(
        store
            .summary("wrk_lifecycle")
            .await
            .unwrap()
            .lite_subscription_id
            .is_none()
    );
    let mut conn = fixture.db.conn().await.unwrap();
    let count: Option<i64> = conn
        .exec_first(
            "SELECT COUNT(*) FROM lite WHERE workspace_id='wrk_lifecycle'",
            (),
        )
        .await
        .unwrap();
    assert_eq!(count, Some(0));
    drop(conn);
    fixture.drop_database().await.unwrap();
}
