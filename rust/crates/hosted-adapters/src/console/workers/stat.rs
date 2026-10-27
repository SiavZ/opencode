//! Purpose: query thirty descending UTC minute TPS buckets and publish scheduled snapshots.
//! Scope: native stat worker and explicit capture scheduler.
//! Uses:
//! rust/crates/
//! |-- hosted-adapters/src/console/db.rs
//! `-- hosted-adapters/src/console/workers.rs
//! External:
//! |-- mysql_async::{Params, Value, prelude::Queryable}
//! |-- serde::{Deserialize, Serialize}
//! |-- time::OffsetDateTime
//! `-- tokio_util::sync::CancellationToken
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/workers.rs
//! Invariants: always thirty buckets, newest first, UTC calendar transitions and missing rows zero-filled.
//! Contract tests: hosted-adapters/tests/console_workers.rs

use super::{Capture, WorkerError};
use crate::console::db::ConsoleDb;
use mysql_async::{Params, prelude::Queryable};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::HashMap;
use time::OffsetDateTime;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Deserialize)]
pub struct StatRequest {
    pub ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bucket {
    pub interval: i64,
    pub qualify: i64,
    pub unqualify: i64,
}

pub fn intervals(now: OffsetDateTime) -> [i64; 30] {
    std::array::from_fn(|index| {
        let date = (now - time::Duration::minutes(index as i64)).to_offset(time::UtcOffset::UTC);
        i64::from(date.year()) * 100_000_000
            + i64::from(u8::from(date.month())) * 1_000_000
            + i64::from(date.day()) * 10_000
            + i64::from(date.hour()) * 100
            + i64::from(date.minute())
    })
}

#[derive(Clone)]
pub struct StatStore {
    db: ConsoleDb,
}

impl StatStore {
    pub fn new(db: ConsoleDb) -> Self {
        Self { db }
    }

    pub async fn aggregate(
        &self,
        ids: &[String],
        now: OffsetDateTime,
    ) -> Result<Value, WorkerError> {
        if ids.is_empty() {
            return Ok(Value::Object(Map::new()));
        }
        let intervals = intervals(now);
        let marks = |count| {
            std::iter::repeat_n("?", count)
                .collect::<Vec<_>>()
                .join(",")
        };
        let sql = format!(
            "SELECT id, `interval`, `qualify`, `unqualify` FROM model_tps_rate_limit WHERE id IN ({}) AND `interval` IN ({})",
            marks(ids.len()),
            marks(intervals.len())
        );
        let params = ids
            .iter()
            .cloned()
            .map(mysql_async::Value::from)
            .chain(intervals.iter().copied().map(mysql_async::Value::from))
            .collect::<Vec<_>>();
        let rows: Vec<(String, i64, i64, i64)> = self
            .db
            .conn()
            .await?
            .exec(sql, Params::Positional(params))
            .await?;
        let rows: HashMap<_, _> = rows
            .into_iter()
            .map(|(id, interval, qualify, unqualify)| ((id, interval), (qualify, unqualify)))
            .collect();
        let mut result = Map::new();
        for id in ids {
            let buckets = intervals
                .iter()
                .map(|interval| {
                    let (qualify, unqualify) = rows
                        .get(&(id.clone(), *interval))
                        .copied()
                        .unwrap_or_default();
                    Bucket {
                        interval: *interval,
                        qualify,
                        unqualify,
                    }
                })
                .collect::<Vec<_>>();
            result.insert(id.clone(), serde_json::to_value(buckets)?);
        }
        Ok(Value::Object(result))
    }

    /// One explicit scheduler tick; no synthetic counters are persisted or inferred.
    pub async fn capture(
        &self,
        ids: &[String],
        now: OffsetDateTime,
        sink: &Capture,
    ) -> Result<Value, WorkerError> {
        let value = self.aggregate(ids, now).await?;
        sink.send(&value).await?;
        Ok(value)
    }

    /// Runs immediately, then once at each UTC minute boundary. Cancellation owns the loop.
    pub async fn scheduled(
        &self,
        ids: &[String],
        sink: &Capture,
        cancel: CancellationToken,
    ) -> Result<(), WorkerError> {
        loop {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => return Ok(()),
                result = self.capture(ids, OffsetDateTime::now_utc(), sink) => { result?; }
            }
            let now = OffsetDateTime::now_utc();
            let next = 60_000
                - now
                    .unix_timestamp_nanos()
                    .div_euclid(1_000_000)
                    .rem_euclid(60_000);
            tokio::select! {
                _ = cancel.cancelled() => return Ok(()),
                _ = tokio::time::sleep(std::time::Duration::from_millis(next as u64)) => {}
            }
        }
    }
}
