//! Purpose: persist anonymous benchmark submissions accepted by the console public API.
//! Scope: MySQL benchmark repository.
//! Uses:
//! rust/crates/
//! |-- hosted-adapters/src/console/db.rs
//! `-- hosted-domain/src/console/common.rs
//! External:
//! `-- mysql_async::prelude::Queryable
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/public.rs
//! Invariants: use baseline benchmark IDs and parameterized values, no external side effects.
//! Contract tests: hosted/src/console/public/tests.rs

use super::db::{ConsoleDb, DbError};
use mysql_async::prelude::Queryable;
use opencode_hosted_domain::console::common::{IdKind, create_id};

#[derive(Debug, thiserror::Error)]
pub enum PublicError {
    #[error(transparent)]
    Database(#[from] DbError),
    #[error(transparent)]
    Mysql(#[from] mysql_async::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("Invalid benchmark tasks")]
    InvalidTasks,
}

/// Native backend for the baseline `benchmarks.list` Solid query.
pub async fn benchmarks(db: &ConsoleDb) -> Result<serde_json::Value, PublicError> {
    let rows: Vec<(String, String, String, String)> = db.conn().await?.query("SELECT `id`, `model`, `agent`, `result` FROM `benchmark` ORDER BY `time_created` DESC LIMIT 100").await?;
    let values = rows
        .into_iter()
        .map(|(id, model, agent, result)| {
            let parsed: serde_json::Value = serde_json::from_str(&result)?;
            let tasks = parsed
                .get("tasks")
                .and_then(serde_json::Value::as_array)
                .ok_or(PublicError::InvalidTasks)?;
            let task_scores = tasks
                .iter()
                .filter_map(|task| {
                    Some((
                        task.get("task")?.get("id")?.as_str()?.to_owned(),
                        task.get("averageScore")?.clone(),
                    ))
                })
                .collect::<serde_json::Map<_, _>>();
            let mut value =
                serde_json::json!({"id":id,"model":model,"agent":agent,"taskScores":task_scores});
            if let Some(score) = parsed.get("averageScore") {
                value["averageScore"] = score.clone();
            }
            Ok(value)
        })
        .collect::<Result<Vec<_>, PublicError>>()?;
    Ok(values.into())
}

/// Native backend for `benchmark.task.detail`; unknown benchmark/task returns JSON null.
pub async fn benchmark_task(
    db: &ConsoleDb,
    benchmark: &str,
    task: &str,
) -> Result<serde_json::Value, PublicError> {
    let result: Option<String> = db
        .conn()
        .await?
        .exec_first(
            "SELECT `result` FROM `benchmark` WHERE `id` = ? LIMIT 1",
            (benchmark,),
        )
        .await?;
    let Some(result) = result else {
        return Ok(serde_json::Value::Null);
    };
    let parsed: serde_json::Value = serde_json::from_str(&result)?;
    let tasks = parsed
        .get("tasks")
        .and_then(serde_json::Value::as_array)
        .ok_or(PublicError::InvalidTasks)?;
    Ok(tasks
        .iter()
        .find(|value| {
            value
                .get("task")
                .and_then(|value| value.get("id"))
                .and_then(serde_json::Value::as_str)
                == Some(task)
        })
        .cloned()
        .unwrap_or(serde_json::Value::Null))
}

pub async fn submit_benchmark(
    db: &ConsoleDb,
    model: &str,
    agent: &str,
    result: &str,
) -> Result<(), DbError> {
    let id = create_id(IdKind::Benchmark, None).expect("generated benchmark identifier");
    db.conn()
        .await?
        .exec_drop(
            "INSERT INTO `benchmark` (`id`, `model`, `agent`, `result`) VALUES (?, ?, ?, ?)",
            (id, model, agent, result),
        )
        .await?;
    Ok(())
}
