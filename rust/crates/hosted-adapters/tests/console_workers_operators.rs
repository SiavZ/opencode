//! Purpose: prove Black gift, transfer and statistics through local HTTP and durable MySQL.
//! Scope: isolated fixture operators, no actual payment provider calls.
//! Uses:
//! rust/crates/
//! |-- hosted-adapters/src/console/db.rs
//! `-- hosted-adapters/src/console/workers/operators.rs
//! External:
//! |-- axum::{Router,Json}
//! |-- mysql_async::prelude::Queryable
//! `-- serde_json::{Value,json}
//! Used by:
//! `-- none, entrypoint
//! Invariants: only literal-loopback Stripe captures and uniquely named fixture databases.
//! Contract tests: this file.

use axum::{Json, Router, body::Bytes, http::Uri};
use mysql_async::prelude::Queryable;
use opencode_hosted_adapters::console::{
    db::{ConsoleDb, TestDatabase},
    workers::{WorkerError, operators},
};
use serde_json::{Value, json};

async fn run(command: &str, mut input: Value, db: &ConsoleDb) -> Result<Value, WorkerError> {
    input["confirmDatabase"] = json!(db.database());
    operators::run(command, &input, db).await
}

#[tokio::test]
async fn fixture_black_gift_transfer_and_exact_stats_csv() {
    let fixture = TestDatabase::create("workerops").await.unwrap();
    let from = run(
        "create-api-key",
        json!({"email":"from@example.test"}),
        &fixture.db,
    )
    .await
    .unwrap();
    let to = run(
        "create-api-key",
        json!({"email":"to@example.test"}),
        &fixture.db,
    )
    .await
    .unwrap();
    let from_id = from["workspaceID"].as_str().unwrap();
    let to_id = to["workspaceID"].as_str().unwrap();
    let (sender, mut receiver) = tokio::sync::mpsc::channel(16);
    let app = Router::new().fallback(move |uri: Uri, body: Bytes| {
        let sender = sender.clone();
        async move {
            sender
                .send((
                    uri.path().to_string(),
                    String::from_utf8(body.to_vec()).unwrap(),
                ))
                .await
                .unwrap();
            let result = match uri.path() {
                "/v1/customers" => json!({"id":"cus_gift"}),
                "/v1/subscriptions" => json!({"id":"sub_gift"}),
                "/v1/customers/cus_previous/payment_methods" => {
                    json!({"data":[{"id":"pm_previous","type":"card","card":{"last4":"4242"}}]})
                }
                _ => json!({"id":"customer_updated"}),
            };
            Json(result)
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    assert!(run("black-gift",json!({"workspaceID":from_id,"seats":1,"priceID":"price_200","stripeCaptureUrl":"https://api.stripe.com"}),&fixture.db).await.is_err());
    let gift = run(
        "black-gift",
        json!({"workspaceID":from_id,"seats":1,"priceID":"price_200","stripeCaptureUrl":endpoint}),
        &fixture.db,
    )
    .await
    .unwrap();
    assert_eq!(gift["subscriptionID"], "sub_gift");
    assert_eq!(receiver.recv().await.unwrap().0, "/v1/customers");
    let request = receiver.recv().await.unwrap();
    assert_eq!(request.0, "/v1/subscriptions");
    let fields = url::form_urlencoded::parse(request.1.as_bytes())
        .collect::<std::collections::HashMap<_, _>>();
    assert_eq!(
        fields.get("items[0][discounts][0][coupon]").unwrap(),
        "JAIr0Pe1"
    );
    assert_eq!(fields.get("items[0][price]").unwrap(), "price_200");
    let mut conn = fixture.db.conn().await.unwrap();
    conn.exec_drop(
        "UPDATE subscription SET time_created = '2026-01-15 00:00:00.000' WHERE workspace_id = ?",
        (from_id,),
    )
    .await
    .unwrap();
    conn.exec_drop("INSERT INTO payment (id,workspace_id,customer_id,amount,time_created) VALUES ('pay_previous',?,'cus_previous',100,'2026-01-01 00:00:00.000')",(from_id,)).await.unwrap();
    conn.exec_drop("INSERT INTO payment (id,workspace_id,customer_id,amount,time_created,enrichment) VALUES ('pay_subscription',?,'cus_gift',20000000000,'2026-01-02 00:00:00.000',JSON_OBJECT('type','subscription'))",(from_id,)).await.unwrap();
    run(
        "black-transfer",
        json!({"fromWorkspaceID":from_id,"toWorkspaceID":to_id,"stripeCaptureUrl":endpoint}),
        &fixture.db,
    )
    .await
    .unwrap();
    assert_eq!(
        receiver.recv().await.unwrap().0,
        "/v1/customers/cus_previous/payment_methods"
    );
    assert_eq!(
        receiver.recv().await.unwrap().0,
        "/v1/customers/cus_previous"
    );
    assert_eq!(receiver.recv().await.unwrap().0, "/v1/customers/cus_gift");
    assert_eq!(
        conn.exec_first::<String, _, _>(
            "SELECT customer_id FROM billing WHERE workspace_id = ?",
            (from_id,)
        )
        .await
        .unwrap(),
        Some("cus_previous".into())
    );
    assert_eq!(
        conn.exec_first::<String, _, _>(
            "SELECT subscription_id FROM billing WHERE workspace_id = ?",
            (to_id,)
        )
        .await
        .unwrap(),
        Some("sub_gift".into())
    );
    assert_eq!(
        conn.exec_first::<String, _, _>(
            "SELECT workspace_id FROM payment WHERE id = 'pay_subscription'",
            ()
        )
        .await
        .unwrap(),
        Some(to_id.into())
    );
    conn.exec_drop("INSERT INTO `usage` (id,workspace_id,model,provider,input_tokens,output_tokens,reasoning_tokens,cache_read_tokens,cost,time_created,enrichment) VALUES ('usg_sub',?,'model,quoted','capture',5,7,3,9,300000000,'2026-01-20 00:00:00.000',JSON_OBJECT('plan','sub'))",(to_id,)).await.unwrap();
    conn.exec_drop("INSERT INTO `usage` (id,workspace_id,model,provider,input_tokens,output_tokens,cost,time_created) VALUES ('usg_ppu',?,'ppu','capture',1,2,200000000,'2026-01-21 00:00:00.000')",(to_id,)).await.unwrap();
    let report = run("black-stats", json!({"plan":"200"}), &fixture.db)
        .await
        .unwrap();
    assert_eq!(report["subscribers"], 1);
    assert_eq!(report["average"], "$3.00");
    let csv = report["csv"].as_str().unwrap();
    // Captured from the actual baseline script using matching query result fixtures.
    assert_eq!(
        csv,
        include_str!("console_workers_black_fixture.csv").replace("WORKSPACE", to_id)
    );
    assert!(csv.contains(
        "2026-01-19 sub,2026-01-19 ppu,2026-01-19 input,2026-01-19 cache,2026-01-19 output"
    ));
    assert!(
        csv.contains(&format!(
            "{to_id},2026-01-15,no,$3.00,$2.00,\"model,quoted\",100.0%,-,-,-,-,$3.00,$2.00,5,9,10\n"
        )),
        "{csv}"
    );
    // Cutoff excludes a newly added seat even though the workspace remains subscribed.
    conn.exec_drop("INSERT INTO subscription (id,workspace_id,user_id,time_created) VALUES ('sub_after',?,'usr_after','2026-02-01 00:00:00.000')",(to_id,)).await.unwrap();
    assert_eq!(
        run("black-stats", json!({"plan":"200"}), &fixture.db)
            .await
            .unwrap()["subscribers"],
        1
    );
    task.abort();
    let _ = task.await;
    drop(conn);
    fixture.drop_database().await.unwrap();
}
