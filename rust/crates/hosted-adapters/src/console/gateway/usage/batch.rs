//! Purpose: batch hot-workspace balance and user accounting through baseline Redis keys.
//! Scope: awaited INCRBY and probabilistic GETDEL, no timers, background tasks or expiry.
//! Uses:
//! rust/crates/
//! |-- hosted-adapters/src/console/gateway/usage.rs
//! `-- hosted-adapters/src/console/limits.rs
//! External:
//! |-- rand::random
//! `-- serde_json::{Value, json}
//! Used by:
//! rust/crates/
//! `-- hosted-adapters/src/console/gateway/usage/record.rs
//! Invariants: workspace and user counters flush independently, preserving baseline failure and pending-counter semantics.
//! Contract tests: hosted-adapters/tests/console_gateway_usage_batch.rs

use super::UsageError;
use crate::console::limits::RedisCounters;
use serde_json::{Value, json};

pub(super) const HOT_WORKSPACE: &str = "wrk_01KJ8PX5CH50Y4YNGNS9ZR8YDC";

#[derive(Clone, Debug)]
pub struct UsageBatcher {
    redis: RedisCounters,
    pub(super) flush_probability: f64,
}

impl UsageBatcher {
    pub fn new(redis: RedisCounters) -> Self {
        Self {
            redis,
            flush_probability: 0.01,
        }
    }

    pub(super) async fn accumulate(
        &self,
        workspace: &str,
        user: &str,
        workspace_cost: i64,
        user_cost: i64,
    ) -> Result<Option<(i64, i64)>, UsageError> {
        let workspace_key = self.redis.usage_key("wrk", workspace);
        let user_key = self.redis.usage_key("usr", &format!("{workspace}:{user}"));
        // Separate requests mirror Promise.all, not a Lua transaction or coupled flush.
        let (workspace_result, user_result) = tokio::join!(
            self.redis
                .commands(vec![json!(["INCRBY", workspace_key, workspace_cost])]),
            self.redis
                .commands(vec![json!(["INCRBY", user_key, user_cost])])
        );
        workspace_result.map_err(UsageError::internal)?;
        user_result.map_err(UsageError::internal)?;
        if self.flush_probability == 0.0 || rand::random::<f64>() > self.flush_probability {
            return Ok(None);
        }
        let (workspace_total, user_total) = tokio::join!(
            self.redis.commands(vec![json!(["GETDEL", workspace_key])]),
            self.redis.commands(vec![json!(["GETDEL", user_key])])
        );
        let value = |values: Vec<Value>| -> Result<i64, UsageError> {
            match values.first() {
                Some(Value::Null) => Ok(0),
                Some(Value::String(value)) => value.parse::<i64>().map_err(UsageError::internal),
                Some(value) => value
                    .as_i64()
                    .ok_or_else(|| UsageError::internal("invalid usage counter")),
                None => Err(UsageError::internal("missing usage counter")),
            }
        };
        let workspace_total = value(workspace_total.map_err(UsageError::internal)?)?;
        let user_total = value(user_total.map_err(UsageError::internal)?)?;
        Ok((workspace_total != 0 || user_total != 0).then_some((workspace_total, user_total)))
    }
}
