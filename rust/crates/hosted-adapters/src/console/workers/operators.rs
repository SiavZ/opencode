//! Purpose: invoke baseline support and billing scripts against explicitly isolated local databases.
//! Scope: native operator command dispatch and durable MySQL mutations.
//! Uses:
//! rust/crates/
//! |-- hosted-adapters/src/console/db.rs
//! |-- hosted-adapters/src/console/support.rs
//! |-- hosted-adapters/src/console/workers.rs
//! |-- hosted-adapters/src/console/workers/operators/config.rs
//! |-- hosted-adapters/src/console/workers/operators/key.rs
//! |-- hosted-adapters/src/console/workers/operators/stats.rs
//! |-- hosted-adapters/src/console/workers/operators/stripe.rs
//! `-- hosted-domain/src/console/common.rs
//! External:
//! |-- mysql_async::{TxOpts, prelude::Queryable}
//! `-- serde_json::{Value, json}
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/workers.rs
//! Invariants: operators require loopback database and exact disposable database confirmation; no shell/cloud delegation.
//! Contract tests: hosted-adapters/tests/console_workers.rs

pub mod config;
mod key;
mod stats;
mod stripe;
use super::WorkerError;
use crate::console::{db::ConsoleDb, support::SupportStore};
use mysql_async::{TxOpts, prelude::Queryable};
use opencode_hosted_domain::console::common::{IdKind, create_id, microcents_to_cents};
use serde_json::{Value, json};

pub fn field<'a>(input: &'a Value, name: &str) -> Result<&'a str, WorkerError> {
    input
        .get(name)
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| WorkerError::Validation(format!("{name} is required")))
}

pub fn validate_local(url: &str, database: &str, confirmation: &str) -> Result<(), WorkerError> {
    let url = url::Url::parse(url).map_err(|e| WorkerError::Validation(e.to_string()))?;
    let local = url.host_str().is_some_and(|host| {
        host == "localhost"
            || host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    });
    if !local || !database.starts_with("p12_") || database != confirmation {
        return Err(WorkerError::Validation("operators require loopback MySQL, disposable p12_ database and matching OPENCODE_CONSOLE_OPERATOR_CONFIRM_DATABASE".into()));
    }
    Ok(())
}

pub fn inventory() -> Value {
    json!({
        "native":["lookup-user","disable-reload","create-coupon","credit-workspace","freeze-workspace","black-onboard-waitlist","black-cancel-waitlist","black-select-workspaces","create-api-key","reset-db","black-gift","black-transfer","black-stats","pull-models","update-models","promote-models","update-limits","promote-limits"],
        "unsupported":{},
        "transports":{"stripe":"explicit literal-loopback capture","secrets":"explicit marked local config store"}
    })
}

/// Caller is responsible for local-target validation before obtaining this adapter.
pub async fn run(command: &str, input: &Value, db: &ConsoleDb) -> Result<Value, WorkerError> {
    if command == "inventory" {
        return Ok(inventory());
    }
    if command == "lookup-user" {
        return SupportStore::new(db.clone())
            .lookup(
                field(input, "identifier")?,
                input.get("limits").unwrap_or(&json!({})),
            )
            .await
            .map_err(|e| WorkerError::Operation(e.to_string()));
    }
    if !db.database().starts_with("p12_")
        || input.get("confirmDatabase").and_then(Value::as_str) != Some(db.database())
    {
        return Err(WorkerError::Validation(
            "operator requires exact disposable confirmDatabase".into(),
        ));
    }
    if config::handles(command) {
        return config::run(command, input).await;
    }
    if command == "black-gift" {
        return stripe::gift(input, db).await;
    }
    if command == "black-transfer" {
        return stripe::transfer(input, db).await;
    }
    if command == "black-stats" {
        return stats::run(input, db).await;
    }
    if command == "create-api-key" {
        return key::create(input, db).await;
    }
    if let Some(reason) = inventory()
        .get("unsupported")
        .and_then(|value| value.get(command))
        .and_then(Value::as_str)
    {
        return Err(WorkerError::Unsupported(format!("{command}: {reason}")));
    }
    if command == "reset-db" {
        if field(input, "stage")? != "frank"
            || field(input, "confirmDatabase")? != db.database()
            || !db.database().starts_with("p12_")
        {
            return Err(WorkerError::Validation(
                "reset-db requires stage frank and exact disposable database confirmation".into(),
            ));
        }
        let mut tx = db.pool().start_transaction(TxOpts::default()).await?;
        for table in [
            "account",
            "billing",
            "key",
            "payment",
            "usage",
            "user",
            "workspace",
        ] {
            tx.query_drop(format!("DELETE FROM `{table}`")).await?;
        }
        tx.commit().await?;
        return Ok(json!({"reset":true}));
    }
    let mut conn = db.conn().await?;
    if command == "create-coupon" {
        let email = field(input, "email")?;
        let kind = field(input, "type")?;
        if ![
            "BUILDATHON",
            "GO1MONTH50",
            "GOFREEMONTH",
            "GO3MONTHS100",
            "GO6MONTHS100",
            "GO12MONTHS100",
        ]
        .contains(&kind)
        {
            return Err(WorkerError::Validation("Invalid coupon type".into()));
        }
        conn.exec_drop(
            "INSERT INTO coupon (email, `type`) VALUES (?, ?)",
            (email, kind),
        )
        .await?;
        return Ok(json!({"email":email,"type":kind}));
    }
    if command == "black-select-workspaces" {
        let plan = field(input, "plan")?;
        if !["20", "100", "200"].contains(&plan) {
            return Err(WorkerError::Validation("Invalid Black plan".into()));
        }
        let ids: Vec<String> = conn.exec("SELECT workspace_id FROM billing WHERE subscription_plan = ? AND time_subscription_selected IS NULL ORDER BY RAND() LIMIT 100",(plan,)).await?;
        let mut emails = Vec::new();
        for id in &ids {
            let rows: Vec<String> = conn.exec("SELECT a.subject FROM `user` u INNER JOIN auth a ON u.account_id = a.account_id AND a.provider = 'email' WHERE u.workspace_id = ?",(id,)).await?;
            for email in rows {
                if !emails.contains(&email) {
                    emails.push(email);
                }
            }
        }
        return Ok(json!({"workspaceIDs":ids,"emails":emails}));
    }
    if ![
        "disable-reload",
        "credit-workspace",
        "freeze-workspace",
        "black-onboard-waitlist",
        "black-cancel-waitlist",
    ]
    .contains(&command)
    {
        return Err(WorkerError::Unsupported(command.into()));
    }
    let workspace = field(input, "workspaceID")?;
    if matches!(command, "black-onboard-waitlist" | "black-cancel-waitlist") {
        let booked: Option<Option<time::PrimitiveDateTime>> = conn
            .exec_first(
                "SELECT time_subscription_booked FROM billing WHERE workspace_id = ?",
                (workspace,),
            )
            .await?;
        if booked.flatten().is_none() {
            return Err(WorkerError::Validation(
                "Workspace is not on the waitlist".into(),
            ));
        }
        let sql = if command == "black-onboard-waitlist" {
            "UPDATE billing SET time_subscription_selected = NOW(3) WHERE workspace_id = ?"
        } else {
            "UPDATE billing SET subscription_plan = NULL, time_subscription_booked = NULL WHERE workspace_id = ?"
        };
        conn.exec_drop(sql, (workspace,)).await?;
        return Ok(json!({"workspaceID":workspace}));
    }
    let exists: Option<String> = conn
        .exec_first("SELECT id FROM workspace WHERE id = ?", (workspace,))
        .await?;
    if exists.is_none() {
        return Err(WorkerError::Validation("Workspace not found".into()));
    }
    if command == "disable-reload" {
        let reload: Option<Option<bool>> = conn
            .exec_first(
                "SELECT reload FROM billing WHERE workspace_id = ?",
                (workspace,),
            )
            .await?;
        let reload = reload
            .ok_or_else(|| WorkerError::Validation("Workspace or billing record not found".into()))?
            .unwrap_or(false);
        if reload {
            conn.exec_drop(
                "UPDATE billing SET reload = FALSE WHERE workspace_id = ?",
                (workspace,),
            )
            .await?;
        }
        return Ok(json!({"workspaceID":workspace,"alreadyDisabled":!reload}));
    }
    drop(conn);
    let mut tx = db.pool().start_transaction(TxOpts::default()).await?;
    let amount = if command == "freeze-workspace" {
        let balance: i64 = tx
            .exec_first(
                "SELECT balance FROM billing WHERE workspace_id = ? FOR UPDATE",
                (workspace,),
            )
            .await?
            .ok_or_else(|| WorkerError::Validation("Billing record not found".into()))?;
        -microcents_to_cents(balance) * 1_000_000
    } else {
        let dollars = input
            .get("dollarAmount")
            .and_then(|v| {
                v.as_f64()
                    .or_else(|| v.as_str().and_then(|v| v.parse().ok()))
            })
            .filter(|v| v.is_finite() && *v > 0.0)
            .ok_or_else(|| {
                WorkerError::Validation("dollarAmount must be a positive number".into())
            })?;
        let amount = (dollars * 100_000_000.0 + 0.5).floor();
        if amount >= i64::MAX as f64 {
            return Err(WorkerError::Validation("dollarAmount out of range".into()));
        }
        amount as i64
    };
    tx.exec_drop(
        "UPDATE billing SET balance = balance + ? WHERE workspace_id = ?",
        (amount, workspace),
    )
    .await?;
    let id = create_id(IdKind::Payment, None).map_err(|e| WorkerError::Operation(e.to_string()))?;
    tx.exec_drop("INSERT INTO payment (id,workspace_id,amount,enrichment) VALUES (?, ?, ?, JSON_OBJECT('type','credit'))",(&id,workspace,amount)).await?;
    tx.commit().await?;
    Ok(json!({"workspaceID":workspace,"amount":amount,"paymentID":id}))
}
