//! Purpose: persist referral attribution and complete subscription referral rewards.
//! Scope: MySQL referral lifecycle, summary and reward application repository.
//! Uses:
//! rust/crates/
//! |-- hosted-adapters/src/console/db.rs
//! `-- hosted-domain/src/console/referral.rs
//! External:
//! |-- mysql_async::{TxOpts, prelude::Queryable}
//! `-- thiserror::Error
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/billing/webhook.rs
//! Invariants: completion locks attribution; each invitee earns one reward workspace; retries never duplicate rewards.
//! Contract tests: hosted-adapters/tests/console_commerce_referral.rs

use super::db::{ConsoleDb, DbError};
use mysql_async::{TxOpts, prelude::Queryable};
use opencode_hosted_domain::console::common::ConsoleError;
use opencode_hosted_domain::console::referral::REWARD_AMOUNT;

#[path = "referral/attribution.rs"]
mod attribution;
#[path = "referral/reward.rs"]
mod reward;
#[path = "referral/summary.rs"]
mod summary;

#[derive(Debug, thiserror::Error)]
pub enum ReferralError {
    #[error(transparent)]
    Console(#[from] ConsoleError),
    #[error(transparent)]
    Db(#[from] DbError),
    #[error(transparent)]
    Mysql(#[from] mysql_async::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Tenant(#[from] super::tenant::TenantError),
}
pub type ReferralResult<T> = Result<T, ReferralError>;

#[derive(Debug, Clone)]
pub struct ReferralStore {
    db: ConsoleDb,
}

impl ReferralStore {
    pub fn new(db: ConsoleDb) -> Self {
        Self { db }
    }

    pub async fn create(&self, inviter: &str, invitee: &str) -> ReferralResult<serde_json::Value> {
        Ok(super::support::SupportStore::new(self.db.clone())
            .create_referral(inviter, invitee)
            .await?)
    }

    pub async fn complete_from_lite_subscription(
        &self,
        workspace: &str,
        user: &str,
    ) -> ReferralResult<()> {
        let mut tx = self.db.pool().start_transaction(TxOpts::default()).await?;
        let account:Option<Option<String>>=tx.exec_first("SELECT account_id FROM user WHERE workspace_id=? AND id=? AND time_deleted IS NULL",(workspace,user)).await?;
        let account = account
            .flatten()
            .ok_or_else(|| ConsoleError::NotFound("Referral invitee account missing".into()))?;
        let referral:Option<(String,String)>=tx.exec_first("SELECT id,workspace_id FROM referral WHERE invitee_account_id=? AND time_deleted IS NULL FOR UPDATE",(&account,)).await?;
        let Some((id, inviter)) = referral else {
            tx.commit().await?;
            return Ok(());
        };
        tx.exec_drop(
            "INSERT IGNORE INTO referral_reward(workspace_id,referral_id,amount) VALUES (?,?,?)",
            (&inviter, &id, REWARD_AMOUNT),
        )
        .await?;
        let existing:Option<String>=tx.exec_first("SELECT workspace_id FROM referral_reward WHERE referral_id=? AND workspace_id<>? AND time_deleted IS NULL LIMIT 1 FOR UPDATE",(&id,&inviter)).await?;
        if existing.is_none() {
            tx.exec_drop("INSERT IGNORE INTO referral_reward(workspace_id,referral_id,amount) VALUES (?,?,?)",(workspace,&id,REWARD_AMOUNT)).await?;
        }
        tx.commit().await?;
        Ok(())
    }
}
