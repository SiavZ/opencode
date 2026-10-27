//! Purpose: workspace repository: create (workspace + admin member + zero billing + default key),
//! info, update, default region, soft delete, block/unblock, disabled models and usage graph.
//! Scope: repository functions on `TenantStore`.
//! Uses:
//! rust/crates/
//! |-- hosted-adapters/src/console/tenant.rs
//! |-- hosted-domain/src/console/common.rs
//! `-- hosted-domain/src/console/membership.rs
//! External:
//! |-- mysql_async::TxOpts
//! |-- mysql_async::Value
//! |-- mysql_async::prelude::Queryable
//! |-- serde::Serialize
//! `-- serde_json::to_string
//! Used by:
//! rust/crates/
//! `-- hosted-adapters/src/console/tenant.rs
//! Invariants: create inserts workspace, admin user and billing in one transaction, then the
//! default key outside it (`core/src/workspace.ts:17-63`); batch block uses 500-id chunks and
//! reports existence via a follow-up select (`core/src/workspace.ts:142-164`).
//! Contract tests: hosted-adapters/tests/console_identity.rs

use mysql_async::TxOpts;
use mysql_async::prelude::Queryable;
use opencode_hosted_domain::console::common::{Actor, ConsoleError, IdKind, create_id};
use opencode_hosted_domain::console::membership::{
    Region, WorkspaceUpdate, authorize_workspace_update, default_regions,
    validate_workspace_create_name,
};
use serde::Serialize;

use super::{TenantResult, TenantStore, placeholders};

/// Settings view row (`settings-section.tsx:12-29`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorkspaceInfo {
    pub id: String,
    pub name: String,
    pub slug: Option<String>,
}

/// Batch block result keys follow the baseline: `blocked`/`unblocked` plus `notFound`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockBatch {
    pub applied: Vec<String>,
    pub not_found: Vec<String>,
}

impl TenantStore {
    /// `Workspace.create` for an account actor. Returns the workspace id.
    pub async fn create_workspace(&self, actor: &Actor, name: &str) -> TenantResult<String> {
        validate_workspace_create_name(name)?;
        let Actor::Account { account_id, .. } = actor else {
            return Err(ConsoleError::Actor(format!(
                "Expected actor type account, got {}",
                actor_type(actor)
            ))
            .into());
        };
        let workspace_id = create_id(IdKind::Workspace, None)?;
        let user_id = create_id(IdKind::User, None)?;
        let mut tx = self
            .db()
            .pool()
            .start_transaction(TxOpts::default())
            .await?;
        let active: Option<String> = tx
            .exec_first(
                "SELECT `id` FROM `account` WHERE `id` = ? AND `time_deleted` IS NULL",
                (account_id,),
            )
            .await?;
        if active.is_none() {
            tx.rollback().await?;
            return Err(ConsoleError::Validation("Account is not active".into()).into());
        }
        tx.exec_drop(
            "INSERT INTO `workspace` (`id`, `name`) VALUES (?, ?)",
            (&workspace_id, name),
        )
        .await?;
        tx.exec_drop(
            "INSERT INTO `user` (`workspace_id`, `id`, `account_id`, `name`, `role`) VALUES (?, ?, ?, '', 'admin')",
            (&workspace_id, &user_id, account_id),
        )
        .await?;
        tx.exec_drop(
            "INSERT INTO `billing` (`workspace_id`, `id`, `balance`) VALUES (?, ?, 0)",
            (&workspace_id, create_id(IdKind::Billing, None)?),
        )
        .await?;
        tx.commit().await?;
        let system = Actor::System {
            workspace_id: workspace_id.clone(),
        };
        self.create_key(&system, &user_id, "Default API Key")
            .await?;
        Ok(workspace_id)
    }

    /// Settings query (no membership filter beyond actor admission, like the baseline).
    pub async fn workspace_info(&self, workspace_id: &str) -> TenantResult<Option<WorkspaceInfo>> {
        let mut conn = self.db().conn().await?;
        let row: Option<(String, String, Option<String>)> = conn
            .exec_first(
                "SELECT `id`, `name`, `slug` FROM `workspace` WHERE `id` = ?",
                (workspace_id,),
            )
            .await?;
        Ok(row.map(|(id, name, slug)| WorkspaceInfo { id, name, slug }))
    }

    /// `Workspace.update`.
    pub async fn update_workspace(
        &self,
        actor: &Actor,
        update: &WorkspaceUpdate,
    ) -> TenantResult<()> {
        let workspace_id = authorize_workspace_update(actor, update)?;
        let mut sets = Vec::new();
        let mut params: Vec<mysql_async::Value> = Vec::new();
        if let Some(name) = &update.name {
            sets.push("`name` = ?");
            params.push(name.clone().into());
        }
        if let Some(region) = &update.region {
            sets.push("`region` = ?");
            params.push(serde_json::to_string(region).unwrap_or_default().into());
        }
        if let Some(allow) = update.allow_training {
            sets.push("`allow_training` = ?");
            params.push(allow.into());
        }
        if sets.is_empty() {
            return Ok(());
        }
        params.push(workspace_id.into());
        let mut conn = self.db().conn().await?;
        conn.exec_drop(
            format!("UPDATE `workspace` SET {} WHERE `id` = ?", sets.join(", ")),
            params,
        )
        .await?;
        Ok(())
    }

    /// `Workspace.setDefaultRegion`: only fills a null region.
    pub async fn set_default_region(
        &self,
        actor: &Actor,
        country: Option<&str>,
    ) -> TenantResult<Vec<Region>> {
        let region = default_regions(country);
        let mut conn = self.db().conn().await?;
        conn.exec_drop(
            "UPDATE `workspace` SET `region` = ? WHERE `id` = ? AND `region` IS NULL",
            (
                serde_json::to_string(&region).unwrap_or_default(),
                actor.workspace()?,
            ),
        )
        .await?;
        Ok(region)
    }

    /// `Workspace.remove`: soft delete (no admin check in the baseline domain).
    pub async fn remove_workspace(&self, actor: &Actor) -> TenantResult<()> {
        let mut conn = self.db().conn().await?;
        conn.exec_drop(
            "UPDATE `workspace` SET `time_deleted` = now() WHERE `id` = ?",
            (actor.workspace()?,),
        )
        .await?;
        Ok(())
    }

    /// `Workspace.unblock`: "Workspace not found" when no row matched.
    pub async fn unblock_workspace(&self, workspace_id: &str) -> TenantResult<()> {
        let mut conn = self.db().conn().await?;
        conn.exec_drop(
            "UPDATE `workspace` SET `is_blocked` = false WHERE `id` = ?",
            (workspace_id,),
        )
        .await?;
        // PlanetScale reports matched rows; MySQL reports changed rows, so check existence.
        let exists: Option<String> = conn
            .exec_first(
                "SELECT `id` FROM `workspace` WHERE `id` = ?",
                (workspace_id,),
            )
            .await?;
        if exists.is_none() {
            return Err(ConsoleError::NotFound("Workspace not found".into()).into());
        }
        Ok(())
    }

    /// `setBlockedBatch`: deduplicated ids, 500-id chunks, sequential on one connection.
    pub async fn set_blocked_batch(
        &self,
        ids: &[String],
        blocked: bool,
    ) -> TenantResult<BlockBatch> {
        let mut unique: Vec<String> = Vec::new();
        for id in ids {
            if !unique.contains(id) {
                unique.push(id.clone());
            }
        }
        let mut conn = self.db().conn().await?;
        let mut applied = std::collections::HashSet::new();
        for chunk in unique.chunks(500) {
            let ph = placeholders(chunk.len());
            let mut params: Vec<mysql_async::Value> = vec![blocked.into()];
            params.extend(chunk.iter().map(|c| c.clone().into()));
            conn.exec_drop(
                format!("UPDATE `workspace` SET `is_blocked` = ? WHERE `id` IN ({ph})"),
                params,
            )
            .await?;
            let rows: Vec<String> = conn
                .exec(
                    format!("SELECT `id` FROM `workspace` WHERE `id` IN ({ph})"),
                    chunk.to_vec(),
                )
                .await?;
            applied.extend(rows);
        }
        let (applied, not_found) = unique.into_iter().partition(|id| applied.contains(id));
        Ok(BlockBatch { applied, not_found })
    }

    /// `usage/graph-section.tsx:getCosts`: month is zero-based, offset is the browser's
    /// fixed mid-month UTC offset. This intentionally includes every plan and all users.
    pub async fn usage_graph(
        &self,
        actor: &Actor,
        year: i32,
        month: i32,
        tz_offset: &str,
    ) -> TenantResult<serde_json::Value> {
        let workspace = actor.workspace()?;
        let year = if (0..100).contains(&year) {
            year + 1900
        } else {
            year
        };
        let months = i64::from(year) * 12 + i64::from(month);
        let date = |months: i64| {
            i32::try_from(months.div_euclid(12))
                .ok()
                .and_then(|year| {
                    time::Month::try_from((months.rem_euclid(12) + 1) as u8)
                        .ok()
                        .and_then(|month| time::Date::from_calendar_date(year, month, 1).ok())
                })
                .ok_or_else(|| ConsoleError::Validation("Invalid graph month".into()))
        };
        let bytes = tz_offset.as_bytes();
        let minutes = if bytes.len() == 6
            && matches!(bytes[0], b'+' | b'-')
            && bytes[3] == b':'
            && [bytes[1], bytes[2], bytes[4], bytes[5]]
                .iter()
                .all(u8::is_ascii_digit)
        {
            let hours = i64::from(bytes[1] - b'0') * 10 + i64::from(bytes[2] - b'0');
            let minutes = i64::from(bytes[4] - b'0') * 10 + i64::from(bytes[5] - b'0');
            (hours * 60 + minutes) * if bytes[0] == b'-' { -1 } else { 1 }
        } else {
            0
        };
        let offset = time::Duration::minutes(minutes);
        let start = date(months)?
            .midnight()
            .checked_sub(offset)
            .ok_or_else(|| ConsoleError::Validation("Invalid graph month".into()))?;
        let end = date(months + 1)?
            .midnight()
            .checked_sub(offset)
            .ok_or_else(|| ConsoleError::Validation("Invalid graph month".into()))?;
        let mut conn = self.db().conn().await?;
        type UsageGraphRow = (
            Option<String>,
            String,
            String,
            Option<String>,
            Option<String>,
        );
        let rows: Vec<UsageGraphRow> = conn.exec(
            "SELECT CAST(DATE(CONVERT_TZ(`time_created`, '+00:00', ?)) AS CHAR) AS `date`, `model`, CAST(SUM(`cost`) AS CHAR), `key_id`, JSON_EXTRACT(`enrichment`, '$.plan') AS `plan` FROM `usage` WHERE `workspace_id` = ? AND `time_created` >= ? AND `time_created` < ? GROUP BY `date`, `model`, `key_id`, `plan`",
            (tz_offset, workspace, start, end),
        ).await?;
        let usage = rows.into_iter().map(|(date, model, total, key_id, plan)| {
            let total = total.parse::<i64>().map_err(|_| ConsoleError::Validation("Usage cost exceeds supported integer range".into()))?;
            // MySQL JSON_EXTRACT returns JSON text; baseline's JSON column decoder unwraps it.
            let plan = plan.and_then(|value| serde_json::from_str::<serde_json::Value>(&value).ok());
            Ok(serde_json::json!({"date": date, "model": model, "totalCost": total, "keyId": key_id, "plan": plan}))
        }).collect::<Result<Vec<_>, ConsoleError>>()?;
        let keys: Vec<(String, String, String, Option<time::PrimitiveDateTime>)> = conn.exec(
            "SELECT `key`.`id`, `key`.`name`, `auth`.`subject`, `key`.`time_deleted` FROM `key` INNER JOIN `user` ON `key`.`user_id` = `user`.`id` AND `key`.`workspace_id` = `user`.`workspace_id` INNER JOIN `auth` ON `user`.`account_id` = `auth`.`account_id` AND `auth`.`provider` = 'email' WHERE `key`.`workspace_id` = ? AND (`key`.`time_deleted` IS NULL OR EXISTS (SELECT 1 FROM `usage` WHERE `usage`.`workspace_id` = ? AND `usage`.`key_id` = `key`.`id` AND `usage`.`time_created` >= ? AND `usage`.`time_created` < ?)) ORDER BY `auth`.`subject`, `key`.`name`",
            (workspace, workspace, start, end),
        ).await?;
        Ok(
            serde_json::json!({"usage": usage, "keys": keys.into_iter().map(|(id, name, email, deleted)| serde_json::json!({"id": id, "displayName": format!("{email} - {name}"), "deleted": deleted.is_some()})).collect::<Vec<_>>()}),
        )
    }

    /// `Model.listDisabled`.
    pub async fn disabled_models(&self, actor: &Actor) -> TenantResult<Vec<String>> {
        let mut conn = self.db().conn().await?;
        Ok(conn
            .exec(
                "SELECT `model` FROM `model` WHERE `workspace_id` = ?",
                (actor.workspace()?,),
            )
            .await?)
    }

    /// `Model.disable` (admin): upsert reviving soft-deleted rows.
    pub async fn disable_model(&self, actor: &Actor, model: &str) -> TenantResult<()> {
        actor.assert_admin()?;
        let mut conn = self.db().conn().await?;
        conn.exec_drop(
            "INSERT INTO `model` (`id`, `workspace_id`, `model`) VALUES (?, ?, ?) ON DUPLICATE KEY UPDATE `time_deleted` = NULL",
            (create_id(IdKind::Model, None)?, actor.workspace()?, model),
        )
        .await?;
        Ok(())
    }

    /// `Model.enable` (admin): hard delete.
    pub async fn enable_model(&self, actor: &Actor, model: &str) -> TenantResult<()> {
        actor.assert_admin()?;
        let mut conn = self.db().conn().await?;
        conn.exec_drop(
            "DELETE FROM `model` WHERE `workspace_id` = ? AND `model` = ?",
            (actor.workspace()?, model),
        )
        .await?;
        Ok(())
    }
}

fn actor_type(actor: &Actor) -> &'static str {
    match actor {
        Actor::Account { .. } => "account",
        Actor::Public {} => "public",
        Actor::User { .. } => "user",
        Actor::System { .. } => "system",
    }
}
