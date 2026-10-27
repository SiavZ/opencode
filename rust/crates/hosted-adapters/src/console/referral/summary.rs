//! Purpose: generate stable referral codes and project baseline pending and earned rewards.
//! Scope: referral summary repository over account emails, invitations and reward ledger.
//! Uses:
//! rust/crates/
//! |-- hosted-adapters/src/console/referral.rs
//! `-- hosted-domain/src/console/referral.rs
//! External:
//! |-- mysql_async::prelude::Queryable
//! `-- time::PrimitiveDateTime
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/referral.rs
//! Invariants: pending rows disappear after rewards exist; invitee reward email belongs to earliest inviter admin with an email.
//! Contract tests: hosted-adapters/tests/console_commerce_referral.rs

use super::*;
use opencode_hosted_domain::console::common::{IdKind, create_id, microcents_to_cents};
use opencode_hosted_domain::console::referral::{Reward, RewardSource, RewardStatus, Summary};
use time::PrimitiveDateTime;

impl ReferralStore {
    pub async fn summary(&self, workspace: &str, account: &str) -> ReferralResult<Summary> {
        let mut conn = self.db.conn().await?;
        let code: Option<String> = conn
            .exec_first(
                "SELECT code FROM referral_code WHERE workspace_id=?",
                (workspace,),
            )
            .await?;
        let code = if let Some(code) = code {
            code
        } else {
            let id = create_id(IdKind::Referral, None)?;
            let code = &id[id.len() - 10..];
            conn.exec_drop(
                "INSERT IGNORE INTO referral_code(workspace_id,code) VALUES (?,?)",
                (workspace, code),
            )
            .await?;
            conn.exec_first(
                "SELECT code FROM referral_code WHERE workspace_id=?",
                (workspace,),
            )
            .await?
            .ok_or_else(|| ConsoleError::Conflict("Failed to generate referral code".into()))?
        };
        let rewards:Vec<mysql_async::Row>=conn.exec("SELECT r.referral_id,r.workspace_id,f.workspace_id,a.subject,r.amount,r.time_created,r.time_applied FROM referral_reward r JOIN referral f ON f.id=r.referral_id JOIN auth a ON a.account_id=f.invitee_account_id AND a.provider='email' WHERE r.workspace_id=? AND r.time_deleted IS NULL AND f.time_deleted IS NULL",(workspace,)).await?;
        let invites:Vec<(String,String,PrimitiveDateTime)>=conn.exec("SELECT f.id,a.subject,f.time_created FROM referral f JOIN auth a ON a.account_id=f.invitee_account_id AND a.provider='email' WHERE f.workspace_id=? AND f.time_deleted IS NULL",(workspace,)).await?;
        let invitee:Vec<(String,Option<String>,PrimitiveDateTime)>=conn.exec("SELECT f.id,a.subject,f.time_created FROM referral f LEFT JOIN user u ON u.workspace_id=f.workspace_id AND u.role='admin' AND u.time_deleted IS NULL LEFT JOIN auth a ON a.account_id=u.account_id AND a.provider='email' WHERE f.invitee_account_id=? AND f.time_deleted IS NULL ORDER BY u.time_created ASC",(account,)).await?;
        let invitee = invitee
            .iter()
            .find(|row| row.1.as_ref().is_some_and(|email| !email.is_empty()))
            .or_else(|| invitee.first());
        let invitee_rewards:Vec<String>=conn.exec("SELECT r.referral_id FROM referral_reward r JOIN referral f ON f.id=r.referral_id WHERE f.invitee_account_id=? AND r.time_deleted IS NULL AND f.time_deleted IS NULL",(account,)).await?;
        let mut earned = Vec::new();
        for mut row in rewards {
            let id: String = row.take(0).unwrap_or_default();
            let reward_workspace: String = row.take(1).unwrap_or_default();
            let inviter_workspace: String = row.take(2).unwrap_or_default();
            let email: String = row.take(3).unwrap_or_default();
            let amount: i64 = row.take(4).unwrap_or_default();
            let created: PrimitiveDateTime = row.take(5).unwrap_or(PrimitiveDateTime::MIN);
            let applied: Option<PrimitiveDateTime> = row.take(6).flatten();
            let source = if reward_workspace == inviter_workspace {
                RewardSource::Inviter
            } else {
                RewardSource::Invitee
            };
            earned.push(Reward {
                id,
                source,
                status: if applied.is_some() {
                    RewardStatus::Applied
                } else {
                    RewardStatus::Available
                },
                email: if source == RewardSource::Invitee {
                    invitee.and_then(|row| row.1.clone())
                } else {
                    Some(email)
                },
                amount: microcents_to_cents(amount),
                time_created: iso(created),
                time_applied: applied.map(iso),
            });
        }
        let mut pending = invites
            .into_iter()
            .filter(|row| !earned.iter().any(|reward| reward.id == row.0))
            .map(|(id, email, created)| Reward {
                id: format!("{id}:inviter"),
                source: RewardSource::Inviter,
                status: RewardStatus::Pending,
                email: Some(email),
                amount: microcents_to_cents(REWARD_AMOUNT),
                time_created: iso(created),
                time_applied: None,
            })
            .collect::<Vec<_>>();
        if let Some((id, email, created)) = invitee
            && !invitee_rewards.contains(id)
        {
            pending.push(Reward {
                id: format!("{id}:invitee"),
                source: RewardSource::Invitee,
                status: RewardStatus::Pending,
                email: email.clone(),
                amount: microcents_to_cents(REWARD_AMOUNT),
                time_created: iso(*created),
                time_applied: None,
            });
        }
        pending.extend(earned);
        pending.sort_by(|a, b| b.time_created.cmp(&a.time_created));
        Ok(Summary {
            referral_code: code,
            has_referral: !pending.is_empty(),
            reward_amount: microcents_to_cents(REWARD_AMOUNT),
            rewards: pending,
        })
    }
}

fn iso(value: PrimitiveDateTime) -> String {
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        value.year(),
        u8::from(value.month()),
        value.day(),
        value.hour(),
        value.minute(),
        value.second(),
        value.millisecond()
    )
}
