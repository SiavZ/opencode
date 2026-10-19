//! Purpose: account and auth-link repository: account create/lookup, issuer auth matches and
//! link upsert, Black-subscription checks used by issuer and admission, invitation claim, last
//! seen workspace, workspace picker and account erasure.
//! Scope: repository functions on `TenantStore`.
//! Uses:
//! rust/crates/
//! |-- hosted-adapters/src/console/tenant.rs
//! |-- hosted-domain/src/console/common.rs
//! `-- hosted-domain/src/console/identity.rs
//! External:
//! |-- mysql_async::TxOpts
//! |-- mysql_async::prelude::Queryable
//! |-- serde::Serialize
//! `-- time::PrimitiveDateTime
//! Used by:
//! rust/crates/
//! `-- hosted-adapters/src/console/tenant.rs
//! Invariants: erasure runs in one transaction and follows `core/src/account.ts:27-69` order;
//! invitation claim updates by email without workspace or deletion filters
//! (`core/src/user.ts:162-201`).
//! Contract tests: hosted-adapters/tests/console_identity.rs

use mysql_async::TxOpts;
use mysql_async::prelude::Queryable;
use opencode_hosted_domain::console::common::{Actor, ConsoleError, IdKind, Role, create_id};
use opencode_hosted_domain::console::identity::{AuthMatch, AuthProvider, WorkspaceMember};
use serde::Serialize;

use super::{TenantResult, TenantStore, iso_opt, placeholders};

/// `AccountTable` row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AccountRow {
    pub id: String,
    #[serde(rename = "timeCreated")]
    pub time_created: Option<String>,
    #[serde(rename = "timeUpdated")]
    pub time_updated: Option<String>,
    #[serde(rename = "timeDeleted")]
    pub time_deleted: Option<String>,
}

/// Workspace picker row (`workspace-picker.tsx:14-35`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorkspaceSummary {
    pub id: String,
    pub name: String,
    pub slug: Option<String>,
}

type Ts = Option<time::PrimitiveDateTime>;

impl TenantStore {
    /// `Account.create`.
    pub async fn create_account(&self, id: Option<&str>) -> TenantResult<String> {
        let id = match id {
            Some(id) => id.to_string(),
            None => create_id(IdKind::Account, None)?,
        };
        let mut conn = self.db().conn().await?;
        conn.exec_drop("INSERT INTO `account` (`id`) VALUES (?)", (&id,))
            .await?;
        Ok(id)
    }

    /// `Account.fromID` (no deletion filter).
    pub async fn account(&self, id: &str) -> TenantResult<Option<AccountRow>> {
        let mut conn = self.db().conn().await?;
        let row: Option<(String, Ts, Ts, Ts)> = conn
            .exec_first(
                "SELECT `id`, `time_created`, `time_updated`, `time_deleted` FROM `account` WHERE `id` = ?",
                (id,),
            )
            .await?;
        Ok(row.map(|(id, c, u, d)| AccountRow {
            id,
            time_created: iso_opt(c),
            time_updated: iso_opt(u),
            time_deleted: iso_opt(d),
        }))
    }

    /// Issuer lookup: provider subject or email subject (`function/src/auth.ts:162-173`).
    pub async fn auth_matches(
        &self,
        provider: AuthProvider,
        subject: &str,
        email: &str,
    ) -> TenantResult<Vec<AuthMatch>> {
        let mut conn = self.db().conn().await?;
        let rows: Vec<(String, String)> = conn
            .exec(
                "SELECT `provider`, `account_id` FROM `auth` WHERE (`provider` = ? AND `subject` = ?) OR (`provider` = 'email' AND `subject` = ?)",
                (provider.as_str(), subject, email),
            )
            .await?;
        Ok(rows
            .into_iter()
            .filter_map(|(p, account_id)| {
                AuthProvider::parse(&p).map(|provider| AuthMatch {
                    provider,
                    account_id,
                })
            })
            .collect())
    }

    /// Upserts the provider and email links, reviving soft-deleted rows (`function/src/auth.ts:196-205`).
    pub async fn upsert_auth_links(
        &self,
        account_id: &str,
        provider: AuthProvider,
        subject: &str,
        email: &str,
    ) -> TenantResult<()> {
        let mut conn = self.db().conn().await?;
        conn.exec_drop(
            "INSERT INTO `auth` (`id`, `account_id`, `provider`, `subject`) VALUES (?, ?, ?, ?), (?, ?, 'email', ?) ON DUPLICATE KEY UPDATE `time_deleted` = NULL",
            (
                create_id(IdKind::Auth, None)?,
                account_id,
                provider.as_str(),
                subject,
                create_id(IdKind::Auth, None)?,
                account_id,
                email,
            ),
        )
        .await?;
        Ok(())
    }

    /// `User.joinInvitedWorkspaces` for an account actor.
    pub async fn join_invited_workspaces(&self, account_id: &str, email: &str) -> TenantResult<()> {
        let mut conn = self.db().conn().await?;
        let active: Option<String> = conn
            .exec_first(
                "SELECT `id` FROM `account` WHERE `id` = ? AND `time_deleted` IS NULL",
                (account_id,),
            )
            .await?;
        if active.is_none() {
            return Err(ConsoleError::Validation("Account is not active".into()).into());
        }
        let invitations: Vec<(String, String)> = conn
            .exec(
                "SELECT `id`, `workspace_id` FROM `user` WHERE `email` = ?",
                (email,),
            )
            .await?;
        conn.exec_drop(
            "UPDATE `user` SET `account_id` = ?, `email` = NULL WHERE `email` = ?",
            (account_id, email),
        )
        .await?;
        drop(conn);
        for (user_id, workspace_id) in invitations {
            let system = Actor::System { workspace_id };
            self.create_key(&system, &user_id, "Default API Key")
                .await?;
        }
        Ok(())
    }

    /// Issuer Black check: live membership in a live workspace with a Black subscription.
    pub async fn has_black_workspace(&self, account_id: &str) -> TenantResult<bool> {
        let mut conn = self.db().conn().await?;
        let row: Option<String> = conn
            .exec_first(
                "SELECT `user`.`id` FROM `user` INNER JOIN `workspace` ON `workspace`.`id` = `user`.`workspace_id` INNER JOIN `billing` ON `billing`.`workspace_id` = `user`.`workspace_id` WHERE `user`.`account_id` = ? AND `user`.`time_deleted` IS NULL AND `workspace`.`time_deleted` IS NULL AND `billing`.`subscription_id` IS NOT NULL LIMIT 1",
                (account_id,),
            )
            .await?;
        Ok(row.is_some())
    }

    /// `requireBlackAccount` (`context/auth.ts:140-157`): no workspace deletion filter.
    pub async fn account_has_black_membership(&self, account_id: &str) -> TenantResult<bool> {
        Ok(!self
            .black_accounts(&[account_id.to_string()])
            .await?
            .is_empty())
    }

    /// Distinct session accounts with a Black membership (`context/auth.ts:66-80`).
    pub async fn black_accounts(&self, accounts: &[String]) -> TenantResult<Vec<String>> {
        if accounts.is_empty() {
            return Ok(Vec::new());
        }
        let mut conn = self.db().conn().await?;
        let sql = format!(
            "SELECT DISTINCT `user`.`account_id` FROM `user` INNER JOIN `billing` ON `billing`.`workspace_id` = `user`.`workspace_id` WHERE `user`.`account_id` IN ({}) AND `user`.`time_deleted` IS NULL AND `billing`.`subscription_id` IS NOT NULL",
            placeholders(accounts.len())
        );
        let rows: Vec<Option<String>> = conn.exec(sql, accounts.to_vec()).await?;
        Ok(rows
            .into_iter()
            .flatten()
            .filter(|a| !a.is_empty())
            .collect())
    }

    /// Workspace member among Black accounts (`context/auth.ts:83-104`).
    pub async fn admission_member(
        &self,
        workspace_id: &str,
        black_accounts: &[String],
    ) -> TenantResult<Option<WorkspaceMember>> {
        if black_accounts.is_empty() {
            return Ok(None);
        }
        let mut conn = self.db().conn().await?;
        let sql = format!(
            "SELECT `user`.`id`, `user`.`workspace_id`, `user`.`account_id`, `user`.`role`, `workspace`.`migrated_at` FROM `user` INNER JOIN `workspace` ON `workspace`.`id` = `user`.`workspace_id` WHERE `user`.`workspace_id` = ? AND `user`.`time_deleted` IS NULL AND `user`.`account_id` IN ({}) LIMIT 1",
            placeholders(black_accounts.len())
        );
        let mut params = vec![workspace_id.to_string()];
        params.extend(black_accounts.iter().cloned());
        let row: Option<(String, String, Option<String>, String, Ts)> =
            conn.exec_first(sql, params).await?;
        Ok(row.map(
            |(user_id, workspace_id, account_id, role, migrated)| WorkspaceMember {
                user_id,
                workspace_id,
                account_id,
                role: if role == "admin" {
                    Role::Admin
                } else {
                    Role::Member
                },
                migrated: migrated.is_some(),
            },
        ))
    }

    /// Records `time_seen = now()` for the admitted member.
    pub async fn touch_seen(&self, workspace_id: &str, user_id: &str) -> TenantResult<()> {
        let mut conn = self.db().conn().await?;
        conn.exec_drop(
            "UPDATE `user` SET `time_seen` = now() WHERE `workspace_id` = ? AND `id` = ?",
            (workspace_id, user_id),
        )
        .await?;
        Ok(())
    }

    /// `getLastSeenWorkspaceID` (`workspace/common.tsx:42-63`): excludes migrated workspaces.
    pub async fn last_seen_workspace(&self, account_id: &str) -> TenantResult<Option<String>> {
        let mut conn = self.db().conn().await?;
        Ok(conn
            .exec_first(
                "SELECT `workspace`.`id` FROM `user` INNER JOIN `workspace` ON `user`.`workspace_id` = `workspace`.`id` WHERE `user`.`account_id` = ? AND `user`.`time_deleted` IS NULL AND `workspace`.`time_deleted` IS NULL AND `workspace`.`migrated_at` IS NULL ORDER BY `user`.`time_seen` DESC LIMIT 1",
                (account_id,),
            )
            .await?)
    }

    /// Workspace picker list for the account.
    pub async fn account_workspaces(
        &self,
        account_id: &str,
    ) -> TenantResult<Vec<WorkspaceSummary>> {
        let mut conn = self.db().conn().await?;
        let rows: Vec<(String, String, Option<String>)> = conn
            .exec(
                "SELECT `workspace`.`id`, `workspace`.`name`, `workspace`.`slug` FROM `user` INNER JOIN `workspace` ON `user`.`workspace_id` = `workspace`.`id` WHERE `user`.`account_id` = ? AND `workspace`.`time_deleted` IS NULL AND `user`.`time_deleted` IS NULL",
                (account_id,),
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|(id, name, slug)| WorkspaceSummary { id, name, slug })
            .collect())
    }

    /// `Account.remove(email)`: revoke keys, anonymize memberships, delete email coupons and
    /// auth links, soft-delete the account, all in one transaction.
    pub async fn erase_account(&self, email: &str) -> TenantResult<()> {
        let mut tx = self
            .db()
            .pool()
            .start_transaction(TxOpts::default())
            .await?;
        let account: Option<String> = tx
            .exec_first(
                "SELECT `account`.`id` FROM `auth` INNER JOIN `account` ON `account`.`id` = `auth`.`account_id` WHERE `auth`.`provider` = 'email' AND `auth`.`subject` = ?",
                (email,),
            )
            .await?;
        let Some(account) = account else {
            tx.rollback().await?;
            return Err(ConsoleError::NotFound("Account not found".into()).into());
        };
        let emails: Vec<String> = tx
            .exec(
                "SELECT `subject` FROM `auth` WHERE `account_id` = ? AND `provider` = 'email'",
                (&account,),
            )
            .await?;
        let users: Vec<String> = tx
            .exec(
                "SELECT `id` FROM `user` WHERE `account_id` = ?",
                (&account,),
            )
            .await?;
        if !users.is_empty() {
            let sql = format!(
                "UPDATE `key` SET `time_deleted` = now() WHERE `user_id` IN ({})",
                placeholders(users.len())
            );
            tx.exec_drop(sql, users).await?;
        }
        tx.exec_drop(
            "UPDATE `user` SET `account_id` = NULL, `email` = NULL, `name` = '', `time_deleted` = now() WHERE `account_id` = ?",
            (&account,),
        )
        .await?;
        if !emails.is_empty() {
            let sql = format!(
                "DELETE FROM `coupon` WHERE `email` IN ({})",
                placeholders(emails.len())
            );
            tx.exec_drop(sql, emails).await?;
        }
        tx.exec_drop("DELETE FROM `auth` WHERE `account_id` = ?", (&account,))
            .await?;
        tx.exec_drop(
            "UPDATE `account` SET `time_deleted` = now() WHERE `id` = ?",
            (&account,),
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }
}
