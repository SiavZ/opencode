//! Purpose: provision baseline manual API keys in one durable MySQL transaction.
//! Scope: local operator key bootstrap.
//! Uses:
//! rust/crates/
//! |-- hosted-adapters/src/console/db.rs
//! |-- hosted-adapters/src/console/workers.rs
//! |-- hosted-adapters/src/console/workers/operators.rs
//! `-- hosted-domain/src/console/common.rs
//! External:
//! |-- mysql_async::{TxOpts, prelude::Queryable}
//! |-- rand::RngExt
//! `-- serde_json::{Value, json}
//! Used by:
//! rust/crates/
//! `-- hosted-adapters/src/console/workers/operators.rs
//! Invariants: key secrets use OS-seeded randomness; account, membership, balance and key commit together.
//! Contract tests: hosted-adapters/tests/console_workers.rs

use super::field;
use crate::console::{db::ConsoleDb, workers::WorkerError};
use mysql_async::{TxOpts, prelude::Queryable};
use opencode_hosted_domain::console::common::{IdKind, create_id};
use rand::RngExt;
use serde_json::{Value, json};

pub(super) async fn create(input: &Value, db: &ConsoleDb) -> Result<Value, WorkerError> {
    let email = field(input, "email")?;
    let dollars = input
        .get("balanceDollars")
        .and_then(Value::as_f64)
        .unwrap_or(100.0);
    if !dollars.is_finite() || dollars < 0.0 || dollars * 100_000_000.0 >= i64::MAX as f64 {
        return Err(WorkerError::Validation("Invalid balanceDollars".into()));
    }
    let id = |kind| create_id(kind, None).map_err(|e| WorkerError::Operation(e.to_string()));
    let mut tx = db.pool().start_transaction(TxOpts::default()).await?;
    let account: Option<String> = tx
        .exec_first(
            "SELECT account_id FROM auth WHERE provider = 'email' AND subject = ?",
            (email,),
        )
        .await?;
    let account = match account {
        Some(id) => id,
        None => {
            let account = id(IdKind::Account)?;
            tx.exec_drop("INSERT INTO account (id) VALUES (?)", (&account,))
                .await?;
            tx.exec_drop(
                "INSERT INTO auth (id,provider,subject,account_id) VALUES (?, 'email', ?, ?)",
                (id(IdKind::Auth)?, email, &account),
            )
            .await?;
            account
        }
    };
    let requested = input
        .get("workspaceID")
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty());
    let workspace: Option<String> = match requested {
        Some(workspace) => tx.exec_first("SELECT id FROM workspace WHERE id = ?",(workspace,)).await?,
        None => tx.exec_first("SELECT w.id FROM `user` u INNER JOIN workspace w ON w.id = u.workspace_id WHERE u.account_id = ? AND u.time_deleted IS NULL LIMIT 1",(&account,)).await?,
    };
    if requested.is_some() && workspace.is_none() {
        return Err(WorkerError::Validation(format!(
            "Workspace not found: {}",
            requested.unwrap_or_default()
        )));
    }
    let workspace = match workspace {
        Some(id) => id,
        None => {
            let workspace = id(IdKind::Workspace)?;
            let name = input
                .get("workspaceName")
                .and_then(Value::as_str)
                .filter(|v| !v.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| format!("{email} manual"));
            tx.exec_drop(
                "INSERT INTO workspace (id,slug,name) VALUES (?, NULL, ?)",
                (&workspace, name),
            )
            .await?;
            workspace
        }
    };
    let user: Option<String> = tx.exec_first("SELECT id FROM `user` WHERE workspace_id = ? AND account_id = ? AND time_deleted IS NULL LIMIT 1",(&workspace,&account)).await?;
    let user = match user {
        Some(id) => id,
        None => {
            let user = id(IdKind::User)?;
            tx.exec_drop("INSERT INTO `user` (id,workspace_id,account_id,email,name,role) VALUES (?, ?, ?, ?, ?, 'admin')",(&user,&workspace,&account,email,email)).await?;
            user
        }
    };
    let balance = (dollars * 100_000_000.0 + 0.5).floor() as i64;
    let billing: Option<i64> = tx
        .exec_first(
            "SELECT balance FROM billing WHERE workspace_id = ? FOR UPDATE",
            (&workspace,),
        )
        .await?;
    if let Some(previous) = billing {
        if previous < balance {
            tx.exec_drop(
                "UPDATE billing SET balance = ? WHERE workspace_id = ?",
                (balance, &workspace),
            )
            .await?;
        }
    } else {
        tx.exec_drop(
            "INSERT INTO billing (id,workspace_id,balance) VALUES (?, ?, ?)",
            (id(IdKind::Billing)?, &workspace, balance),
        )
        .await?;
    }
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let secret = format!(
        "sk-{}",
        (0..64)
            .map(|_| CHARS[(rand::rng().random::<u32>() as usize) % CHARS.len()] as char)
            .collect::<String>()
    );
    let key = id(IdKind::Key)?;
    let name = input
        .get("keyName")
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())
        .unwrap_or("Manual API Key");
    tx.exec_drop("INSERT INTO `key` (id,workspace_id,user_id,name,`key`,time_used) VALUES (?, ?, ?, ?, ?, NULL)",(&key,&workspace,&user,name,&secret)).await?;
    tx.commit().await?;
    Ok(
        json!({"stage":input.get("stage").and_then(Value::as_str).unwrap_or("fixture"),"accountID":account,"workspaceID":workspace,"userID":user,"keyID":key,"secretKey":secret}),
    )
}
