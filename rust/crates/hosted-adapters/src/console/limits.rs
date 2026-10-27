//! Purpose: enforce baseline Redis request caps and MySQL provider routing counters.
//! Scope: request limits, checkout caps, trial tokens, sticky placement, TPM/TPS and budgets.
//! Uses:
//! rust/crates/
//! |-- hosted-adapters/src/console/limits/provider.rs
//! `-- hosted-adapters/src/console/limits/store.rs
//! External:
//! |-- reqwest::Client
//! |-- serde_json::Value
//! `-- time::OffsetDateTime
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/zen/proxy.rs
//! Invariants: request caps track completed responses, checkout caps reserve before payment attempts.
//! Contract tests: hosted-adapters/tests/console_gateway_limits.rs

pub mod provider;
pub mod store;

use serde_json::{Value, json};
use time::OffsetDateTime;

#[derive(Debug, thiserror::Error)]
pub enum LimitError {
    #[error("Rate limit exceeded")]
    Exceeded {
        kind: &'static str,
        retry_after: i64,
    },
    #[error("Rate limit backend unavailable")]
    Backend,
}

#[derive(Clone)]
pub struct RedisCounters {
    client: reqwest::Client,
    url: String,
    token: String,
    stage: String,
}

impl std::fmt::Debug for RedisCounters {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RedisCounters")
            .field("stage", &self.stage)
            .finish_non_exhaustive()
    }
}

impl RedisCounters {
    pub fn new(url: &str, token: &str, stage: &str) -> Result<Self, LimitError> {
        let parsed = url::Url::parse(url).map_err(|_| LimitError::Backend)?;
        if !matches!(parsed.scheme(), "https" | "http") || parsed.host_str().is_none() {
            return Err(LimitError::Backend);
        }
        Ok(Self {
            client: reqwest::Client::new(),
            url: url.trim_end_matches('/').into(),
            token: token.into(),
            stage: stage.into(),
        })
    }

    pub fn usage_key(&self, kind: &str, id: &str) -> String {
        format!("{}:usage:{kind}:{id}", self.stage)
    }

    pub fn key(&self, kind: &str, id: &str, interval: Option<&str>) -> String {
        format!(
            "{}:ratelimit:{kind}:{id}{}",
            self.stage,
            interval
                .map(|value| format!(":{value}"))
                .unwrap_or_default()
        )
    }

    pub async fn commands(&self, commands: Vec<Value>) -> Result<Vec<Value>, LimitError> {
        let values: Vec<Value> = self
            .client
            .post(format!("{}/pipeline", self.url))
            .timeout(std::time::Duration::from_secs(30))
            .bearer_auth(&self.token)
            .json(&commands)
            .send()
            .await
            .map_err(|_| LimitError::Backend)?
            .error_for_status()
            .map_err(|_| LimitError::Backend)?
            .json()
            .await
            .map_err(|_| LimitError::Backend)?;
        if values.len() != commands.len() || values.iter().any(|v| v.get("error").is_some()) {
            return Err(LimitError::Backend);
        }
        values
            .into_iter()
            .map(|value| value.get("result").cloned().ok_or(LimitError::Backend))
            .collect()
    }

    pub async fn values(&self, keys: &[String]) -> Result<Vec<i64>, LimitError> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        let command = std::iter::once(json!("MGET"))
            .chain(keys.iter().map(|key| json!(key)))
            .collect::<Vec<_>>();
        let values = self.commands(vec![json!(command)]).await?;
        let values = values
            .first()
            .and_then(Value::as_array)
            .ok_or(LimitError::Backend)?;
        if values.len() != keys.len() {
            return Err(LimitError::Backend);
        }
        values
            .iter()
            .map(|value| match value {
                Value::Null => Ok(0),
                Value::String(value) => value.parse().map_err(|_| LimitError::Backend),
                _ => value.as_i64().ok_or(LimitError::Backend),
            })
            .collect()
    }

    pub async fn checkout(&self, account: &str) -> Result<(), LimitError> {
        let key = self.key("checkout", account, None);
        let count = self
            .commands(vec![json!(["INCR", key])])
            .await?
            .first()
            .and_then(Value::as_i64)
            .ok_or(LimitError::Backend)?;
        if count == 1 {
            self.commands(vec![json!(["EXPIRE", key, 3600])]).await?;
        }
        if count > 5 {
            return Err(LimitError::Exceeded {
                kind: "CheckoutRateLimitError",
                retry_after: 3600,
            });
        }
        Ok(())
    }

    pub async fn request(
        &self,
        model: &str,
        rate_limit: Option<i64>,
        anonymous: bool,
        identity: &str,
        free_daily: i64,
        now: OffsetDateTime,
    ) -> Result<RequestLimit, LimitError> {
        let date = interval(now);
        let seconds_left = 86_400 - now.unix_timestamp().rem_euclid(86_400);
        let (daily, lifetime, expiry, limit, kind) = if anonymous {
            let identity = if identity.is_empty() {
                "unknown"
            } else {
                identity
            };
            let custom = rate_limit.filter(|limit| *limit != 0);
            let suffix = if custom.is_some() {
                format!(
                    "{}{}",
                    &date[..8],
                    model.chars().take(2).collect::<String>()
                )
            } else {
                date[..8].into()
            };
            (
                self.key("ip", identity, Some(&suffix)),
                custom.is_none().then(|| self.key("ip", identity, None)),
                seconds_left,
                custom.unwrap_or(free_daily),
                "FreeUsageLimitError",
            )
        } else {
            (
                self.key(
                    "key",
                    identity,
                    Some(&format!(
                        "{}-{date}",
                        model.chars().take(27).collect::<String>()
                    )),
                ),
                None,
                60,
                rate_limit.unwrap_or(1000),
                "RateLimitError",
            )
        };
        let keys = lifetime
            .iter()
            .cloned()
            .chain(std::iter::once(daily.clone()))
            .collect::<Vec<_>>();
        let counts = self.values(&keys).await?;
        let is_new = lifetime.is_some() && counts[0] < limit * 7;
        let count = *counts.last().ok_or(LimitError::Backend)?;
        if count >= if is_new { limit * 2 } else { limit } {
            return Err(LimitError::Exceeded {
                kind,
                retry_after: expiry,
            });
        }
        Ok(RequestLimit {
            redis: self.clone(),
            daily,
            lifetime: lifetime.filter(|_| is_new),
            expiry,
        })
    }
}

pub struct RequestLimit {
    redis: RedisCounters,
    daily: String,
    lifetime: Option<String>,
    expiry: i64,
}

impl RequestLimit {
    pub async fn track(&self) -> Result<(), LimitError> {
        let mut commands = vec![
            json!(["INCR", self.daily]),
            json!(["EXPIRE", self.daily, self.expiry]),
        ];
        if let Some(key) = &self.lifetime {
            commands.push(json!(["INCR", key]));
        }
        self.redis.commands(commands).await?;
        Ok(())
    }
}

pub fn interval(now: OffsetDateTime) -> String {
    format!(
        "{:04}{:02}{:02}{:02}{:02}",
        now.year(),
        now.month() as u8,
        now.day(),
        now.hour(),
        now.minute()
    )
}

pub fn normalized_ip(ip: &str) -> String {
    if ip.contains(':') {
        ip.split(':').take(4).collect::<Vec<_>>().join(":")
    } else {
        ip.into()
    }
}
