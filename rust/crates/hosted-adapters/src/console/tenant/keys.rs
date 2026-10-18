//! Purpose: API key and BYOK provider repository: key create/list/remove with own-vs-admin
//! scope, BYOK provider upsert/list/remove.
//! Scope: repository functions on `TenantStore`.
//! Uses:
//! rust/crates/
//! |-- hosted-adapters/src/console/tenant.rs
//! |-- hosted-domain/src/console/common.rs
//! `-- hosted-domain/src/console/credentials.rs
//! External:
//! `-- mysql_async::prelude::Queryable
//! Used by:
//! rust/crates/
//! |-- hosted-adapters/src/console/tenant.rs
//! |-- hosted-adapters/src/console/tenant/account.rs
//! |-- hosted-adapters/src/console/tenant/member.rs
//! `-- hosted-adapters/src/console/tenant/workspace.rs
//! Invariants: keys are workspace filtered, list joins the owner's email auth row and orders by
//! name DESC (`core/src/key.ts:11-40`); members only see/remove their own keys; provider list
//! returns full rows including credentials, matching the baseline (recorded as a blocker).
//! Contract tests: hosted-adapters/tests/console_identity.rs

use mysql_async::prelude::Queryable;
use opencode_hosted_domain::console::common::{Actor, IdKind, create_id};
use opencode_hosted_domain::console::credentials::{
    KeyRow, KeyView, authorize_provider_create, authorize_provider_remove, generate_secret,
    key_scope_user, project_keys, validate_key_name,
};
use serde::Serialize;

use super::{TenantResult, TenantStore, iso_opt};

/// `ProviderTable` row as returned by `Provider.list`.
#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct ProviderRow {
    pub id: String,
    #[serde(rename = "workspaceID")]
    pub workspace_id: String,
    #[serde(rename = "timeCreated")]
    pub time_created: Option<String>,
    #[serde(rename = "timeUpdated")]
    pub time_updated: Option<String>,
    #[serde(rename = "timeDeleted")]
    pub time_deleted: Option<String>,
    pub provider: String,
    pub credentials: String,
}

impl std::fmt::Debug for ProviderRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderRow")
            .field("id", &self.id)
            .field("workspace_id", &self.workspace_id)
            .field("provider", &self.provider)
            .finish_non_exhaustive()
    }
}

type Ts = Option<time::PrimitiveDateTime>;

impl TenantStore {
    /// `Key.create`: workspace from the actor (user or system). Returns the key id.
    pub async fn create_key(
        &self,
        actor: &Actor,
        user_id: &str,
        name: &str,
    ) -> TenantResult<String> {
        validate_key_name(name)?;
        let key_id = create_id(IdKind::Key, None)?;
        let mut conn = self.db().conn().await?;
        conn.exec_drop(
            "INSERT INTO `key` (`id`, `workspace_id`, `user_id`, `name`, `key`, `time_used`) VALUES (?, ?, ?, ?, ?, NULL)",
            (&key_id, actor.workspace()?, user_id, name, generate_secret()),
        )
        .await?;
        Ok(key_id)
    }

    /// `Key.list` projected for the actor.
    pub async fn list_keys(&self, actor: &Actor) -> TenantResult<Vec<KeyView>> {
        let scope = key_scope_user(actor)?;
        let mut sql = String::from(
            "SELECT `key`.`id`, `key`.`name`, `key`.`key`, `key`.`time_used`, `key`.`user_id`, `auth`.`subject` FROM `key` INNER JOIN `user` ON `key`.`user_id` = `user`.`id` AND `key`.`workspace_id` = `user`.`workspace_id` INNER JOIN `auth` ON `user`.`account_id` = `auth`.`account_id` AND `auth`.`provider` = 'email' WHERE `key`.`workspace_id` = ? AND `key`.`time_deleted` IS NULL",
        );
        let mut params = vec![actor.workspace()?.to_string()];
        if let Some(user) = scope {
            sql.push_str(" AND `key`.`user_id` = ?");
            params.push(user);
        }
        sql.push_str(" ORDER BY `key`.`name` DESC");
        let mut conn = self.db().conn().await?;
        let rows: Vec<(String, String, String, Ts, String, String)> =
            conn.exec(sql, params).await?;
        let rows = rows
            .into_iter()
            .map(|(id, name, key, used, user_id, email)| KeyRow {
                id,
                name,
                key,
                time_used: iso_opt(used),
                user_id,
                email,
            })
            .collect();
        Ok(project_keys(actor, rows)?)
    }

    /// `Key.remove`: soft delete; members can only remove their own keys.
    pub async fn remove_key(&self, actor: &Actor, key_id: &str) -> TenantResult<()> {
        let scope = key_scope_user(actor)?;
        let mut sql = String::from(
            "UPDATE `key` SET `time_deleted` = now() WHERE `id` = ? AND `workspace_id` = ?",
        );
        let mut params = vec![key_id.to_string(), actor.workspace()?.to_string()];
        if let Some(user) = scope {
            sql.push_str(" AND `user_id` = ?");
            params.push(user);
        }
        let mut conn = self.db().conn().await?;
        conn.exec_drop(sql, params).await?;
        Ok(())
    }

    /// `Provider.list`: live BYOK rows of the actor workspace.
    pub async fn list_providers(&self, actor: &Actor) -> TenantResult<Vec<ProviderRow>> {
        let mut conn = self.db().conn().await?;
        let rows: Vec<(String, String, Ts, Ts, Ts, String, String)> = conn
            .exec(
                "SELECT `id`, `workspace_id`, `time_created`, `time_updated`, `time_deleted`, `provider`, `credentials` FROM `provider` WHERE `workspace_id` = ? AND `time_deleted` IS NULL",
                (actor.workspace()?,),
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(
                |(id, workspace_id, c, u, d, provider, credentials)| ProviderRow {
                    id,
                    workspace_id,
                    time_created: iso_opt(c),
                    time_updated: iso_opt(u),
                    time_deleted: iso_opt(d),
                    provider,
                    credentials,
                },
            )
            .collect())
    }

    /// `Provider.create` (admin): upsert by (workspace, provider).
    pub async fn save_provider(
        &self,
        actor: &Actor,
        provider: &str,
        credentials: &str,
    ) -> TenantResult<()> {
        let workspace_id = authorize_provider_create(actor, provider)?;
        let mut conn = self.db().conn().await?;
        conn.exec_drop(
            "INSERT INTO `provider` (`id`, `workspace_id`, `provider`, `credentials`) VALUES (?, ?, ?, ?) ON DUPLICATE KEY UPDATE `credentials` = VALUES(`credentials`), `time_deleted` = NULL",
            (create_id(IdKind::Provider, None)?, workspace_id, provider, credentials),
        )
        .await?;
        Ok(())
    }

    /// `Provider.remove` (admin): hard delete.
    pub async fn remove_provider(&self, actor: &Actor, provider: &str) -> TenantResult<()> {
        let workspace_id = authorize_provider_remove(actor)?;
        let mut conn = self.db().conn().await?;
        conn.exec_drop(
            "DELETE FROM `provider` WHERE `provider` = ? AND `workspace_id` = ?",
            (provider, workspace_id),
        )
        .await?;
        Ok(())
    }
}
