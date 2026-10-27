//! Purpose: attribute newly eligible accounts to normalized referral invite codes.
//! Scope: account-level referral admission and prior Lite subscription eligibility checks.
//! Uses:
//! rust/crates/
//! |-- hosted-adapters/src/console/referral.rs
//! `-- hosted-domain/src/console/referral.rs
//! External:
//! `-- mysql_async::{TxOpts, prelude::Queryable}
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/referral.rs
//! Invariants: one active attribution per invitee account; existing Lite members or historic Lite payments cannot create attribution.
//! Contract tests: hosted-adapters/tests/console_commerce_referral.rs

use super::*;
use opencode_hosted_domain::console::common::{IdKind, create_id};
use opencode_hosted_domain::console::referral::normalize_code;

impl ReferralStore {
    pub async fn create_from_account(
        &self,
        account: &str,
        code: Option<&str>,
    ) -> ReferralResult<()> {
        let Some(code) = normalize_code(code) else {
            return Ok(());
        };
        let mut tx = self.db.pool().start_transaction(TxOpts::default()).await?;
        let inviter:Option<String>=tx.exec_first("SELECT c.workspace_id FROM referral_code c JOIN workspace w ON w.id=c.workspace_id WHERE c.code=? AND w.time_deleted IS NULL",(&code,)).await?;
        let inviter =
            inviter.ok_or_else(|| ConsoleError::Validation("Referral code invalid".into()))?;
        // Account lock serializes attribution races without inventing another durable event ledger.
        let _: Option<String> = tx
            .exec_first("SELECT id FROM account WHERE id=? FOR UPDATE", (account,))
            .await?;
        let existing: Option<String> = tx
            .exec_first(
                "SELECT id FROM referral WHERE invitee_account_id=? AND time_deleted IS NULL",
                (account,),
            )
            .await?;
        if existing.is_some() {
            return Err(ConsoleError::Conflict("Referral already redeemed".into()).into());
        }
        let self_referral:Option<String>=tx.exec_first("SELECT id FROM user WHERE workspace_id=? AND account_id=? AND time_deleted IS NULL LIMIT 1",(&inviter,account)).await?;
        if self_referral.is_some() {
            return Err(ConsoleError::Validation("Self-referral is not allowed".into()).into());
        }
        let workspace: Option<String> = tx
            .exec_first(
                "SELECT workspace_id FROM user WHERE account_id=? AND time_deleted IS NULL LIMIT 1",
                (account,),
            )
            .await?;
        if workspace.is_none() {
            tx.commit().await?;
            return Ok(());
        }
        let lite:Option<String>=tx.exec_first("SELECT l.id FROM lite l JOIN user u ON u.workspace_id=l.workspace_id WHERE u.account_id=? AND u.time_deleted IS NULL AND l.time_deleted IS NULL LIMIT 1",(account,)).await?;
        if lite.is_some() {
            tx.commit().await?;
            return Ok(());
        }
        let payment:Option<String>=tx.exec_first("SELECT p.id FROM payment p JOIN user u ON u.workspace_id=p.workspace_id WHERE u.account_id=? AND u.time_deleted IS NULL AND p.time_deleted IS NULL AND JSON_UNQUOTE(JSON_EXTRACT(p.enrichment,'$.type'))='lite' LIMIT 1",(account,)).await?;
        if payment.is_some() {
            tx.commit().await?;
            return Ok(());
        }
        let id = create_id(IdKind::Referral, None)?;
        tx.exec_drop(
            "INSERT IGNORE INTO referral(id,workspace_id,invitee_account_id) VALUES (?,?,?)",
            (&id, &inviter, account),
        )
        .await?;
        let created: Option<String> = tx
            .exec_first(
                "SELECT id FROM referral WHERE invitee_account_id=? AND time_deleted IS NULL",
                (account,),
            )
            .await?;
        match created {
            None => return Err(ConsoleError::Conflict("Referral not created".into()).into()),
            Some(created) if created != id => {
                return Err(ConsoleError::Conflict("Referral already redeemed".into()).into());
            }
            _ => {}
        }
        tx.commit().await?;
        Ok(())
    }
}
