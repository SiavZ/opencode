//! Purpose: membership repository: list members with auth email, member lookup, invite (pending
//! email or existing account + default key + swallowed mail), update role/limit, remove.
//! Scope: repository functions on `TenantStore`.
//! Uses:
//! rust/crates/
//! |-- hosted-adapters/src/console/mail.rs
//! |-- hosted-adapters/src/console/tenant.rs
//! |-- hosted-domain/src/console/common.rs
//! `-- hosted-domain/src/console/membership.rs
//! External:
//! |-- mysql_async::Row
//! |-- mysql_async::prelude::Queryable
//! |-- serde::Serialize
//! |-- time::PrimitiveDateTime
//! `-- tracing::error
//! Used by:
//! rust/crates/
//! `-- hosted-adapters/src/console/tenant.rs
//! Invariants: invite persists the membership before mail; mail failures are logged and
//! swallowed (`core/src/user.ts:126-158`); update/remove filter by actor workspace.
//! Contract tests: hosted-adapters/tests/console_identity.rs

use mysql_async::prelude::Queryable;
use opencode_hosted_domain::console::common::{Actor, IdKind, Role, create_id};
use opencode_hosted_domain::console::membership::{
    Invite, authorize_invite, authorize_member_remove, authorize_member_update,
};
use serde::Serialize;

use super::{TenantResult, TenantStore, iso_opt};
use crate::console::mail::{InviteMail, InviteMailer};

/// `User.list` row: every `UserTable` column plus `authEmail`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MemberRow {
    pub id: String,
    #[serde(rename = "workspaceID")]
    pub workspace_id: String,
    #[serde(rename = "timeCreated")]
    pub time_created: Option<String>,
    #[serde(rename = "timeUpdated")]
    pub time_updated: Option<String>,
    #[serde(rename = "timeDeleted")]
    pub time_deleted: Option<String>,
    #[serde(rename = "accountID")]
    pub account_id: Option<String>,
    pub email: Option<String>,
    pub name: String,
    #[serde(rename = "timeSeen")]
    pub time_seen: Option<String>,
    pub color: Option<i32>,
    pub role: Role,
    #[serde(rename = "monthlyLimit")]
    pub monthly_limit: Option<i32>,
    #[serde(rename = "monthlyUsage")]
    pub monthly_usage: Option<i64>,
    #[serde(rename = "timeMonthlyUsageUpdated")]
    pub time_monthly_usage_updated: Option<String>,
    #[serde(rename = "authEmail")]
    pub auth_email: Option<String>,
}

type Ts = Option<time::PrimitiveDateTime>;

const MEMBER_COLUMNS: &str = "`user`.`id`, `user`.`workspace_id`, `user`.`time_created`, `user`.`time_updated`, `user`.`time_deleted`, `user`.`account_id`, `user`.`email`, `user`.`name`, `user`.`time_seen`, `user`.`color`, `user`.`role`, `user`.`monthly_limit`, `user`.`monthly_usage`, `user`.`time_monthly_usage_updated`, `auth`.`subject`";

fn member_row(mut row: mysql_async::Row) -> MemberRow {
    let ts = |row: &mut mysql_async::Row, i: usize| iso_opt(row.take::<Ts, _>(i).flatten());
    MemberRow {
        id: row.take(0).unwrap_or_default(),
        workspace_id: row.take(1).unwrap_or_default(),
        time_created: ts(&mut row, 2),
        time_updated: ts(&mut row, 3),
        time_deleted: ts(&mut row, 4),
        account_id: row.take(5).flatten(),
        email: row.take(6).flatten(),
        name: row.take(7).unwrap_or_default(),
        time_seen: ts(&mut row, 8),
        color: row.take(9).flatten(),
        role: if row.take::<String, _>(10).as_deref() == Some("admin") {
            Role::Admin
        } else {
            Role::Member
        },
        monthly_limit: row.take(11).flatten(),
        monthly_usage: row.take(12).flatten(),
        time_monthly_usage_updated: ts(&mut row, 13),
        auth_email: row.take(14).flatten(),
    }
}

impl TenantStore {
    /// `User.list`: live members of the actor workspace with their email auth subject.
    pub async fn list_members(&self, actor: &Actor) -> TenantResult<Vec<MemberRow>> {
        let mut conn = self.db().conn().await?;
        let rows: Vec<mysql_async::Row> = conn
            .exec(
                format!("SELECT {MEMBER_COLUMNS} FROM `user` LEFT JOIN `auth` ON `user`.`account_id` = `auth`.`account_id` AND `auth`.`provider` = 'email' WHERE `user`.`workspace_id` = ? AND `user`.`time_deleted` IS NULL"),
                (actor.workspace()?,),
            )
            .await?;
        Ok(rows.into_iter().map(member_row).collect())
    }

    /// `User.getAuthEmail` (no deletion filter).
    pub async fn member_auth_email(
        &self,
        actor: &Actor,
        user_id: &str,
    ) -> TenantResult<Option<String>> {
        let mut conn = self.db().conn().await?;
        let row: Option<Option<String>> = conn
            .exec_first(
                "SELECT `auth`.`subject` FROM `user` LEFT JOIN `auth` ON `user`.`account_id` = `auth`.`account_id` AND `auth`.`provider` = 'email' WHERE `user`.`workspace_id` = ? AND `user`.`id` = ?",
                (actor.workspace()?, user_id),
            )
            .await?;
        Ok(row.flatten())
    }

    /// `User.invite`. `mailer` receives the invite after persistence; errors are swallowed.
    pub async fn invite_member(
        &self,
        actor: &Actor,
        invite: &Invite,
        mailer: &dyn InviteMailer,
    ) -> TenantResult<()> {
        let workspace_id = authorize_invite(actor)?;
        let mut conn = self.db().conn().await?;
        let account_id: Option<String> = conn
            .exec_first(
                "SELECT `account_id` FROM `auth` WHERE `provider` = 'email' AND `subject` = ?",
                (&invite.email,),
            )
            .await?;
        let (account_col, email_col) = match &account_id {
            Some(a) => (Some(a.clone()), None),
            None => (None, Some(invite.email.clone())),
        };
        conn.exec_drop(
            "INSERT INTO `user` (`id`, `name`, `account_id`, `email`, `workspace_id`, `role`, `monthly_limit`) VALUES (?, '', ?, ?, ?, ?, ?) ON DUPLICATE KEY UPDATE `role` = VALUES(`role`), `monthly_limit` = VALUES(`monthly_limit`), `time_deleted` = NULL",
            (
                create_id(IdKind::User, None)?,
                account_col,
                email_col,
                &workspace_id,
                invite.role.as_str(),
                invite.monthly_limit,
            ),
        )
        .await?;
        if let Some(account_id) = &account_id {
            let user_id: Option<String> = conn
                .exec_first(
                    "SELECT `id` FROM `user` WHERE `workspace_id` = ? AND `account_id` = ?",
                    (&workspace_id, account_id),
                )
                .await?;
            if let Some(user_id) = user_id {
                let key: Option<String> = conn
                    .exec_first(
                        "SELECT `id` FROM `key` WHERE `workspace_id` = ? AND `user_id` = ?",
                        (&workspace_id, &user_id),
                    )
                    .await?;
                if key.is_none() {
                    self.create_key(actor, &user_id, "Default API Key").await?;
                }
            }
        }
        // Mail info and delivery: any failure is logged and ignored, like the baseline.
        let inviter_user = match actor {
            Actor::User { user_id, .. } => user_id.clone(),
            _ => String::new(),
        };
        let info: Result<Option<(String, String)>, _> = conn
            .exec_first(
                "SELECT `auth`.`subject`, `workspace`.`name` FROM `user` INNER JOIN `auth` ON `user`.`account_id` = `auth`.`account_id` AND `auth`.`provider` = 'email' INNER JOIN `workspace` ON `workspace`.`id` = ? WHERE `user`.`workspace_id` = ? AND `user`.`id` = ?",
                (&workspace_id, &workspace_id, &inviter_user),
            )
            .await;
        drop(conn);
        match info {
            Ok(Some((inviter, workspace_name))) => {
                let mail = InviteMail {
                    to: invite.email.clone(),
                    inviter,
                    workspace_id: workspace_id.clone(),
                    workspace_name,
                };
                if let Err(e) = mailer.send_invite(&mail).await {
                    tracing::error!(error = %e, "invite email failed");
                }
            }
            Ok(None) => tracing::error!("invite email skipped: inviter info not found"),
            Err(e) => tracing::error!(error = %e, "invite email info query failed"),
        }
        Ok(())
    }

    /// `User.update`.
    pub async fn update_member(
        &self,
        actor: &Actor,
        user_id: &str,
        role: Role,
        monthly_limit: Option<i64>,
    ) -> TenantResult<()> {
        let workspace_id = authorize_member_update(actor, user_id, role)?;
        let mut conn = self.db().conn().await?;
        conn.exec_drop(
            "UPDATE `user` SET `role` = ?, `monthly_limit` = ? WHERE `id` = ? AND `workspace_id` = ?",
            (role.as_str(), monthly_limit, user_id, workspace_id),
        )
        .await?;
        Ok(())
    }

    /// `User.remove`: soft delete within the actor workspace.
    pub async fn remove_member(&self, actor: &Actor, user_id: &str) -> TenantResult<()> {
        let workspace_id = authorize_member_remove(actor, user_id)?;
        let mut conn = self.db().conn().await?;
        conn.exec_drop(
            "UPDATE `user` SET `time_deleted` = now() WHERE `id` = ? AND `workspace_id` = ?",
            (user_id, workspace_id),
        )
        .await?;
        Ok(())
    }
}
