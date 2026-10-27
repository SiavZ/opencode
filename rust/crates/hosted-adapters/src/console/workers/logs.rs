//! Purpose: enrich native Cloudflare-format tail traces using baseline console log semantics.
//! Scope: trace batch transformation and optional explicit capture.
//! Uses:
//! rust/crates/
//! `-- hosted-adapters/src/console/workers.rs
//! External:
//! |-- serde_json::{Value, json}
//! |-- time::OffsetDateTime
//! `-- url::Url
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/workers.rs
//! Invariants: Honeycomb stays disabled; error snapshots precede final merged completion data.
//! Contract tests: hosted-adapters/tests/console_workers.rs

use super::{Capture, WorkerError};
use serde_json::{Value, json};
use time::OffsetDateTime;

/// Matches the baseline string algorithm, including uncompressed noncanonical IPv6 input.
pub fn ip_prefix(ip: &str) -> Option<String> {
    if ip.is_empty() {
        return None;
    }
    if ip.contains('.') && !ip.contains(':') {
        return Some(format!("{ip}/32"));
    }
    if !ip.contains(':') {
        return None;
    }
    let mut split = ip.split("::");
    let head = split.next().unwrap_or_default();
    let tail = split.next();
    let head: Vec<_> = if head.is_empty() {
        vec![]
    } else {
        head.split(':').collect()
    };
    let tail: Vec<_> = tail
        .map(|value| value.split(':').collect())
        .unwrap_or_default();
    let missing = 8_usize.checked_sub(head.len() + tail.len())?;
    let full = head
        .into_iter()
        .chain(std::iter::repeat_n("0", missing))
        .chain(tail)
        .collect::<Vec<_>>();
    let prefix = full[..4]
        .iter()
        .map(|part| {
            let lower = part.to_lowercase();
            let trimmed = lower.trim_start_matches('0');
            if trimmed.is_empty() && !lower.is_empty() {
                "0".to_string()
            } else {
                trimmed.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(":");
    Some(format!("{prefix}::/64"))
}

fn parse_int(value: &str) -> Value {
    let value = value.trim_start();
    let (negative, value) = if let Some(value) = value.strip_prefix('-') {
        (true, value)
    } else {
        (false, value.strip_prefix('+').unwrap_or(value))
    };
    let (radix, value) = if let Some(value) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        (16, value)
    } else {
        (10, value)
    };
    let digits = value
        .chars()
        .take_while(|c| c.is_digit(radix))
        .collect::<String>();
    i64::from_str_radix(&digits, radix)
        .ok()
        .map(|n| json!(if negative { -n } else { n }))
        .unwrap_or(Value::Null)
}

fn timestamp(value: Option<&Value>, now: OffsetDateTime) -> Result<String, WorkerError> {
    let date = match value {
        None | Some(Value::Null) => now,
        Some(value) if value.is_number() => {
            let millis = value
                .as_f64()
                .ok_or_else(|| WorkerError::Validation("Invalid eventTimestamp".into()))?;
            OffsetDateTime::from_unix_timestamp_nanos((millis.trunc() as i128) * 1_000_000)
                .map_err(|e| WorkerError::Validation(e.to_string()))?
        }
        Some(Value::String(value)) => {
            OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339)
                .map_err(|e| WorkerError::Validation(e.to_string()))?
        }
        _ => return Err(WorkerError::Validation("Invalid eventTimestamp".into())),
    }
    .to_offset(time::UtcOffset::UTC);
    Ok(format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        date.year(),
        u8::from(date.month()),
        date.day(),
        date.hour(),
        date.minute(),
        date.second(),
        date.millisecond()
    ))
}

/// Returns console data separately from disabled Honeycomb batch records.
pub fn enrich(traces: &[Value], now: OffsetDateTime) -> Result<Value, WorkerError> {
    let mut console = Vec::new();
    let mut records = Vec::new();
    for trace in traces {
        let Some(request) = trace.pointer("/event/request").filter(|v| v.is_object()) else {
            continue;
        };
        if request.get("method").and_then(Value::as_str) != Some("POST") {
            continue;
        }
        let url = url::Url::parse(
            request
                .get("url")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        )
        .map_err(|e| WorkerError::Validation(e.to_string()))?;
        let path = url.path();
        if ![
            "/zen/v1/chat/completions",
            "/zen/v1/messages",
            "/zen/v1/responses",
            "/zen/go/v1/chat/completions",
            "/zen/go/v1/messages",
            "/zen/go/v1/responses",
        ]
        .contains(&path)
            && !path.starts_with("/zen/v1/models/")
            && !path.starts_with("/zen/go/v1/models/")
        {
            continue;
        }
        let mut data = serde_json::Map::new();
        for field in [
            "continent",
            "country",
            "city",
            "region",
            "latitude",
            "longitude",
            "timezone",
        ] {
            if let Some(value) = request.get("cf").and_then(|cf| cf.get(field)) {
                data.insert(format!("cf.{field}"), value.clone());
            }
        }
        if let Some(duration) = trace.get("wallTime") {
            data.insert("duration".into(), duration.clone());
        }
        data.insert(
            "request_length".into(),
            parse_int(
                request
                    .pointer("/headers/content-length")
                    .and_then(Value::as_str)
                    .unwrap_or("0"),
            ),
        );
        data.insert(
            "status".into(),
            trace
                .pointer("/event/response/status")
                .filter(|v| !v.is_null())
                .cloned()
                .unwrap_or(json!(0)),
        );
        if let Some(ip) = request.pointer("/headers/x-real-ip") {
            data.insert("ip".into(), ip.clone());
            if let Some(prefix) = ip.as_str().and_then(ip_prefix) {
                data.insert("ip.prefix".into(), json!(prefix));
            }
        }
        let time = timestamp(trace.get("eventTimestamp"), now)?;
        for log in trace
            .get("logs")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            for message in log
                .get("message")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let message = message.as_str().ok_or_else(|| {
                    WorkerError::Validation("log message must be a string".into())
                })?;
                let Some(metric) = message.strip_prefix("_metric:") else {
                    continue;
                };
                let metric: Value = serde_json::from_str(metric)?;
                let metric = metric
                    .as_object()
                    .ok_or_else(|| WorkerError::Validation("metric must be an object".into()))?;
                data.extend(metric.clone());
                if metric.contains_key("llm.error.code") {
                    let mut error = data.clone();
                    error.insert("event_type".into(), json!("llm.error"));
                    records.push(json!({"time":time,"data":error}));
                }
            }
        }
        console.push(Value::Object(data.clone()));
        data.insert("event_type".into(), json!("completions"));
        records.push(json!({"time":time,"data":data}));
    }
    Ok(json!({"console":console,"events":records}))
}

/// Explicit capture is a native transport extension, not automatic Honeycomb delivery.
pub async fn capture(
    traces: &[Value],
    now: OffsetDateTime,
    sink: &Capture,
) -> Result<Value, WorkerError> {
    let result = enrich(traces, now)?;
    sink.send(&result).await?;
    Ok(result)
}
