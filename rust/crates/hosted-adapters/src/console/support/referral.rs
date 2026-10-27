//! Purpose: create operator referral links and exactly two five-dollar rewards idempotently.
//! Scope: one MySQL transaction per support action.
//! Uses:
//! rust/crates/
//! |-- hosted-adapters/src/console/support.rs
//! |-- hosted-domain/src/console/common.rs
//! `-- hosted-domain/src/console/referral.rs
//! External:
//! |-- mysql_async::{TxOpts, prelude::Queryable}
//! `-- serde_json::{Value, json}
//! Used by:
//! rust/crates/
//! `-- hosted-adapters/src/console/support.rs
//! Invariants: no self referrals; conflicting inviter is rejected; duplicate requests never duplicate rewards.
//! Contract tests: hosted-adapters/tests/console_support.rs

use mysql_async::TxOpts;
use mysql_async::prelude::Queryable;
use opencode_hosted_domain::console::common::{ConsoleError, IdKind, create_id};
use opencode_hosted_domain::console::referral::REWARD_AMOUNT;
use serde_json::{Value, json};

use super::{SupportResult, SupportStore};

impl SupportStore {
    pub async fn create_referral(&self, inviter: &str, invitee: &str) -> SupportResult<Value> {
        if inviter == invitee {
            return Err(ConsoleError::Validation("Self-referral workspace mismatch".into()).into());
        }
        let mut tx = self.db.pool().start_transaction(TxOpts::default()).await?;
        // Lock workspaces in a consistent order to serialize competing operator requests.
        let mut ids = [inviter, invitee];
        ids.sort();
        let existing: Vec<String> = tx.exec("SELECT id FROM workspace WHERE id IN (?, ?) AND time_deleted IS NULL ORDER BY id FOR UPDATE", (ids[0], ids[1])).await?;
        for (id, role) in [(inviter, "Inviter"), (invitee, "Invitee")] {
            if !existing.iter().any(|existing| existing == id) {
                return Err(
                    ConsoleError::NotFound(format!("{role} workspace not found: {id}")).into(),
                );
            }
        }
        let owner: Option<(Option<String>, String, String)> = tx.exec_first("SELECT u.account_id, u.id, l.id FROM `user` u INNER JOIN lite l ON l.workspace_id = u.workspace_id AND l.user_id = u.id AND l.time_deleted IS NULL WHERE u.workspace_id = ? AND u.role = 'admin' AND u.time_deleted IS NULL LIMIT 1 FOR UPDATE", (invitee,)).await?;
        let Some((Some(account_id), user_id, lite_id)) = owner else {
            return Err(ConsoleError::NotFound(format!(
                "Invitee Lite workspace owner not found: {invitee}"
            ))
            .into());
        };
        let self_referral: Option<String> = tx.exec_first("SELECT id FROM `user` WHERE workspace_id = ? AND account_id = ? AND time_deleted IS NULL LIMIT 1", (inviter, &account_id)).await?;
        if self_referral.is_some() {
            return Err(ConsoleError::Validation(format!(
                "Self-referral is not allowed: {account_id}"
            ))
            .into());
        }
        let existing: Option<(String, String)> = tx.exec_first("SELECT id, workspace_id FROM referral WHERE invitee_account_id = ? AND time_deleted IS NULL LIMIT 1 FOR UPDATE", (&account_id,)).await?;
        if let Some((id, workspace)) = &existing
            && workspace != inviter
        {
            return Err(ConsoleError::Conflict(format!(
                "Referral already belongs to {workspace}: {id}"
            ))
            .into());
        }
        let created = existing.is_none();
        let referral_id = match existing {
            Some((id, _)) => id,
            None => create_id(IdKind::Referral, None)?,
        };
        if created {
            tx.exec_drop("INSERT IGNORE INTO referral (workspace_id, id, invitee_account_id) VALUES (?, ?, ?)", (inviter, &referral_id, &account_id)).await?;
            let actual: Option<String> = tx
                .exec_first(
                    "SELECT id FROM referral WHERE invitee_account_id = ? AND time_deleted IS NULL",
                    (&account_id,),
                )
                .await?;
            match actual {
                None => {
                    return Err(ConsoleError::Conflict(format!(
                        "Referral not created: {account_id}"
                    ))
                    .into());
                }
                Some(actual) if actual != referral_id => {
                    return Err(ConsoleError::Conflict(format!(
                        "Referral already redeemed: {actual}"
                    ))
                    .into());
                }
                _ => {}
            }
        }
        tx.exec_drop("INSERT IGNORE INTO referral_reward (workspace_id, referral_id, amount) VALUES (?, ?, ?), (?, ?, ?)", (inviter, &referral_id, REWARD_AMOUNT, invitee, &referral_id, REWARD_AMOUNT)).await?;
        let created_rewards = tx.affected_rows();
        let rewards: Vec<(String, i64)> = tx.exec("SELECT workspace_id, amount FROM referral_reward WHERE referral_id = ? AND workspace_id IN (?, ?) AND time_deleted IS NULL", (&referral_id, inviter, invitee)).await?;
        if rewards.len() != 2 {
            return Err(ConsoleError::Conflict(format!(
                "Referral rewards not created: {referral_id}"
            ))
            .into());
        }
        if rewards.iter().any(|(_, amount)| *amount != REWARD_AMOUNT) {
            return Err(ConsoleError::Conflict(format!(
                "Referral reward amount mismatch: {referral_id}"
            ))
            .into());
        }
        tx.commit().await?;
        Ok(
            json!({"referralID":referral_id,"createdReferral":created,"createdRewards":created_rewards,"inviteeAccountID":account_id,"inviteeUserID":user_id,"liteID":lite_id,"rewardWorkspaces":rewards.into_iter().map(|(workspace, _)|workspace).collect::<Vec<_>>()}),
        )
    }
}
