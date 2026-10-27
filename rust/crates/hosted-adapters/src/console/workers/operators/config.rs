//! Purpose: run baseline model and limit secret scripts through an explicit durable local config store.
//! Scope: isolated filesystem stage configuration operations, never cloud promotion.
//! Uses:
//! rust/crates/
//! |-- hosted-adapters/src/console/workers.rs
//! |-- hosted-adapters/src/console/workers/operators.rs
//! `-- hosted-adapters/src/console/workers/operators/config/validate.rs
//! External:
//! |-- serde_json::{Value,json}
//! `-- std::fs::OpenOptions
//! Used by:
//! rust/crates/
//! `-- hosted-adapters/src/console/workers/operators.rs
//! Invariants: stores require fixture marker and exact confirmation; atomic replace after validation, no shells or provider calls.
//! Contract tests: hosted-adapters/tests/console_workers_config.rs

mod validate;
use super::field;
use crate::console::workers::WorkerError;
use serde_json::{Value, json};
use std::{
    io::Write,
    path::{Path, PathBuf},
};

pub fn handles(command: &str) -> bool {
    [
        "pull-models",
        "update-models",
        "promote-models",
        "update-limits",
        "promote-limits",
    ]
    .contains(&command)
}

/// Stage keys contain only strings, exactly like `sst secret list` values.
/// The marker prevents confusing this native capture format with production infrastructure state.
pub async fn run(command: &str, input: &Value) -> Result<Value, WorkerError> {
    let command = command.to_owned();
    let input = input.clone();
    tokio::task::spawn_blocking(move || execute(&command, &input)).await?
}

fn execute(command: &str, input: &Value) -> Result<Value, WorkerError> {
    let root = PathBuf::from(field(input, "configStore")?).canonicalize()?;
    if !root.is_dir()
        || field(input, "confirmConfigStore")? != root.to_string_lossy()
        || std::fs::read_to_string(root.join(".console-worker-fixture"))?.trim()
            != "p12-console-config-v1"
    {
        return Err(WorkerError::Validation("config store requires exact canonical confirmation and p12-console-config-v1 fixture marker".into()));
    }
    let lock = root.join(".lock");
    let lock_file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock)?;
    let outcome = operate(command, input, &root);
    drop(lock_file);
    let cleanup = std::fs::remove_file(lock);
    let value = outcome?;
    cleanup?;
    Ok(value)
}

fn stage(input: &Value, key: &str) -> Result<String, WorkerError> {
    let stage = field(input, key)?;
    if !stage
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(WorkerError::Validation("Invalid stage".into()));
    }
    Ok(stage.into())
}
fn read(root: &Path, stage: &str) -> Result<serde_json::Map<String, Value>, WorkerError> {
    let path = root.join(format!("{stage}.json"));
    if std::fs::symlink_metadata(&path)?.file_type().is_symlink() {
        return Err(WorkerError::Validation(
            "stage secret file must not be a symlink".into(),
        ));
    }
    let value: Value = serde_json::from_slice(&std::fs::read(path)?)?;
    value
        .as_object()
        .filter(|v| v.values().all(Value::is_string))
        .cloned()
        .ok_or_else(|| {
            WorkerError::Validation("stage secrets must be a string-valued object".into())
        })
}
fn model_parts(
    values: &serde_json::Map<String, Value>,
    count: usize,
) -> Result<Vec<String>, WorkerError> {
    (1..=count)
        .map(|index| {
            values
                .get(&format!("ZEN_MODELS{index}"))
                .and_then(Value::as_str)
                .filter(|v| !v.is_empty())
                .map(str::to_string)
                .ok_or_else(|| WorkerError::Validation(format!("ZEN_MODELS{index} not found")))
        })
        .collect()
}

fn operate(command: &str, input: &Value, root: &Path) -> Result<Value, WorkerError> {
    let source = match command {
        "pull-models" => stage(input, "stage")?,
        "update-models" | "promote-models" => "frank".into(),
        "update-limits" | "promote-limits" => "fallback".into(),
        _ => return Err(WorkerError::Unsupported(command.into())),
    };
    let destination = match command {
        "pull-models" => stage(input, "localStage")?,
        "update-models" => "frank".into(),
        "update-limits" => "fallback".into(),
        _ => stage(input, "stage")?,
    };
    let values = read(root, &source)?;
    let updates = if command.ends_with("models") {
        let count = if command == "pull-models" { 20 } else { 30 };
        let old = model_parts(&values, count)?;
        let old_json: Value = serde_json::from_str(&old.join(""))?;
        let parts = if command == "update-models" {
            // Explicit edited JSON replaces the baseline interactive editor. Validate old JSON first.
            let _ = old_json;
            let edited = input
                .get("edited")
                .ok_or_else(|| WorkerError::Validation("edited JSON is required".into()))?;
            validate::models(edited)?;
            let text = serde_json::to_string(edited)?;
            let chars = text.chars().collect::<Vec<_>>();
            let size = chars.len().div_ceil(count);
            (0..count)
                .map(|index| {
                    chars
                        .iter()
                        .skip(index * size)
                        .take(size)
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
        } else {
            validate::models(&old_json)?;
            old
        };
        parts
            .into_iter()
            .enumerate()
            .map(|(index, value)| (format!("ZEN_MODELS{}", index + 1), Value::String(value)))
            .collect::<serde_json::Map<_, _>>()
    } else {
        let value = values
            .get("ZEN_LIMITS")
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| WorkerError::Validation("ZEN_LIMITS not found".into()))?;
        let old: Value = serde_json::from_str(value)?;
        let value = if command == "update-limits" {
            let _ = old;
            let edited = input
                .get("edited")
                .ok_or_else(|| WorkerError::Validation("edited JSON is required".into()))?;
            validate::limits(edited)?;
            serde_json::to_string(edited)?
        } else {
            validate::limits(&old)?;
            value.to_string()
        };
        [("ZEN_LIMITS".to_string(), Value::String(value))]
            .into_iter()
            .collect()
    };
    let path = root.join(format!("{destination}.json"));
    let mut target = if path.exists() {
        read(root, &destination)?
    } else {
        serde_json::Map::new()
    };
    let keys = updates.keys().cloned().collect::<Vec<_>>();
    target.extend(updates);
    let pending = root.join(format!(".pending-{}", uuid::Uuid::new_v4()));
    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&pending)?;
    file.write_all(&serde_json::to_vec_pretty(&target)?)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&pending, path)?;
    std::fs::File::open(root)?.sync_all()?;
    Ok(
        json!({"source":source,"destination":destination,"keys":keys,"transport":"local-config-store"}),
    )
}
