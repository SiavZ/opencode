//! Purpose: reset active subscription usage without changing quota window timestamps.
//! Scope: one MySQL transaction per reset.
//! Uses:
//! rust/crates/
//! |-- hosted-adapters/src/console/support.rs
//! `-- hosted-domain/src/console/quotas.rs
//! External:
//! `-- mysql_async::{TxOpts, prelude::Queryable}
//! Used by:
//! rust/crates/
//! `-- hosted-adapters/src/console/support.rs
//! Invariants: sum counters before reset under row locks; deleted subscriptions stay untouched.
//! Contract tests: hosted-adapters/tests/console_support.rs

use mysql_async::TxOpts;
use mysql_async::prelude::Queryable;
use opencode_hosted_domain::console::quotas::{self, Plan, ResetResult};

use super::{SupportResult, SupportStore};

impl SupportStore {
    pub async fn reset_quota(&self, workspace_id: &str, plan: Plan) -> SupportResult<ResetResult> {
        let mut tx = self.db.pool().start_transaction(TxOpts::default()).await?;
        let before = match plan {
            Plan::Lite => {
                let rows = tx.exec("SELECT rolling_usage, weekly_usage, monthly_usage FROM lite WHERE workspace_id = ? AND time_deleted IS NULL FOR UPDATE", (workspace_id,)).await?;
                quotas::sum_lite(&rows)
            }
            Plan::Subscription => {
                let rows = tx.exec("SELECT rolling_usage, fixed_usage FROM subscription WHERE workspace_id = ? AND time_deleted IS NULL FOR UPDATE", (workspace_id,)).await?;
                quotas::sum_subscription(&rows)
            }
        };
        let Some(before) = before else {
            tx.rollback().await?;
            return Err(quotas::missing_rows(plan).into());
        };
        tx.exec_drop(match plan {
            Plan::Lite => "UPDATE lite SET rolling_usage = 0, weekly_usage = 0, monthly_usage = 0 WHERE workspace_id = ? AND time_deleted IS NULL",
            Plan::Subscription => "UPDATE subscription SET rolling_usage = 0, fixed_usage = 0 WHERE workspace_id = ? AND time_deleted IS NULL",
        }, (workspace_id,)).await?;
        tx.commit().await?;
        Ok(ResetResult { plan, before })
    }
}
