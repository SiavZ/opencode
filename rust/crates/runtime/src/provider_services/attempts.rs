//! Purpose: own canonical OAuth attempts, callback execution, expiry and credential rotation.
//! Scope: Location-scoped in-memory attempt state with real plugin callbacks and SQLite persistence.
//! Uses:
//! rust/crates/
//! |-- integration-adapters/src/plugins/mod.rs
//! |-- runtime/src/provider_services.rs
//! `-- storage-sqlite/src/repositories/credentials.rs
//! External:
//! |-- tokio::task::JoinSet
//! `-- tokio_util::sync::CancellationToken
//! Used by:
//! rust/crates/
//! `-- runtime/src/provider_services/registry.rs
//! Invariants: every task is owned and joined; concurrent completion cannot run a callback twice.
//! Contract tests: runtime/tests/provider_services.rs

use super::ProviderServices;
use opencode_integration_adapters::plugins::Instance;
use opencode_schema::ids::IntegrationAttemptID;
use opencode_server::services::legacy::LegacyError;
use opencode_storage_api::{Database, DatabaseExt, TxOptions};
use opencode_storage_sqlite::repositories::credentials;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tokio::sync::Mutex;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
pub(super) struct State {
    entries: Arc<Mutex<BTreeMap<String, Attempt>>>,
    tasks: Mutex<JoinSet<()>>,
    started: AtomicBool,
    stop: CancellationToken,
}

#[derive(Clone)]
struct Attempt {
    plugin: Arc<Instance>,
    integration: String,
    method: String,
    label: Option<String>,
    mode: String,
    created: i64,
    expires: i64,
    status: String,
    message: Option<String>,
    remove_at: i64,
    completing: bool,
    cancel: CancellationToken,
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn authorization() -> LegacyError {
    LegacyError::Response {
        status: 400,
        body: json!({"_tag":"InvalidRequestError","message":"Authentication failed","kind":"integration_authorization"}),
    }
}

impl ProviderServices {
    pub async fn shutdown(&self) {
        self.attempts.stop.cancel();
        for attempt in self.attempts.entries.lock().await.values() {
            attempt.cancel.cancel();
        }
        let mut tasks = self.attempts.tasks.lock().await;
        tasks.abort_all();
        while tasks.join_next().await.is_some() {}
        let attempts = std::mem::take(&mut *self.attempts.entries.lock().await);
        for (id, attempt) in attempts {
            let _ = attempt
                .plugin
                .host
                .request(
                    "auth.cancel",
                    json!({"handle":attempt.plugin.handle,"attemptID":id}),
                )
                .await;
        }
    }

    pub(super) async fn start_attempt(
        &self,
        integration: &str,
        payload: Value,
    ) -> Result<Value, LegacyError> {
        if self.attempts.stop.is_cancelled() {
            return Err(LegacyError::Unavailable(
                "provider service has shut down".into(),
            ));
        }
        let method = payload
            .get("methodID")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let plugins = self.plugins.read().await.clone();
        let mut selected = None;
        for plugin in plugins.into_iter().rev() {
            let auth = plugin
                .host
                .request("auth.methods", json!({"handle":plugin.handle}))
                .await
                .map_err(super::plugin_error)?;
            let Some(provider) = auth.get("provider").and_then(Value::as_str) else {
                continue;
            };
            if self.integration(provider) != integration {
                continue;
            }
            let index = method.parse::<usize>().ok();
            if index
                .and_then(|index| auth["methods"].get(index))
                .is_some_and(|method| method["type"] == "oauth")
            {
                selected = index.map(|index| (plugin, index));
            }
            break;
        }
        let (plugin, index) = selected.ok_or_else(|| {
            LegacyError::Internal(format!("OAuth method not found: {integration}/{method}"))
        })?;
        let id = IntegrationAttemptID::create().as_str().to_owned();
        let result = plugin.host.request("auth.authorize",json!({"handle":plugin.handle,"method":index,"inputs":payload.get("inputs"),"attemptID":id})).await.map_err(|_|authorization())?;
        let created = now();
        let attempt = Attempt {
            plugin,
            integration: integration.to_owned(),
            method: method.to_owned(),
            label: payload
                .get("label")
                .and_then(Value::as_str)
                .map(str::to_owned),
            mode: result["method"].as_str().unwrap_or("code").to_owned(),
            created,
            expires: created + 600000,
            status: "pending".into(),
            message: None,
            remove_at: 0,
            completing: result["method"] == "auto",
            cancel: CancellationToken::new(),
        };
        self.attempts
            .entries
            .lock()
            .await
            .insert(id.clone(), attempt.clone());
        let mut tasks = self.attempts.tasks.lock().await;
        while tasks.try_join_next().is_some() {}
        if !self.attempts.started.swap(true, Ordering::AcqRel) {
            let entries = self.attempts.entries.clone();
            let stop = self.attempts.stop.clone();
            tasks.spawn(async move {
                loop {
                    tokio::select! { _=stop.cancelled()=>break, _=tokio::time::sleep(std::time::Duration::from_secs(30))=>{} }
                    let expired:Vec<_> = {
                        let mut entries = entries.lock().await;
                        let time = now();
                        let mut expired = Vec::new();
                        entries.retain(|id,attempt| {
                            if attempt.status == "pending" && attempt.expires<=time {
                                attempt.cancel.cancel(); attempt.status="expired".into(); attempt.remove_at=time+60000;
                                expired.push((id.clone(),attempt.plugin.clone()));
                            }
                            attempt.status == "pending" || attempt.remove_at>time
                        });
                        expired
                    };
                    for (id,plugin) in expired { let _ = plugin.host.request("auth.cancel",json!({"handle":plugin.handle,"attemptID":id})).await; }
                }
            });
        }
        if attempt.mode == "auto" {
            let entries = self.attempts.entries.clone();
            let database = self.database.clone();
            let id = id.clone();
            tasks.spawn(async move {
                let _ = complete(entries, database, id, attempt, None).await;
            });
        }
        Ok(
            json!({"attemptID":id,"url":result["url"],"instructions":result["instructions"],"mode":result["method"],"time":{"created":created,"expires":created+600000}}),
        )
    }

    pub(super) async fn attempt_operation(
        &self,
        operation: &str,
        params: &Map<String, Value>,
        payload: Option<Value>,
    ) -> Result<Value, LegacyError> {
        let id = params
            .get("attemptID")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if operation == "v2.integration.attempt.cancel" {
            let attempt = {
                let mut entries = self.attempts.entries.lock().await;
                if entries
                    .get(id)
                    .is_some_and(|attempt| attempt.status == "pending")
                {
                    entries.remove(id)
                } else {
                    None
                }
            };
            if let Some(attempt) = attempt {
                attempt.cancel.cancel();
                let _ = attempt
                    .plugin
                    .host
                    .request(
                        "auth.cancel",
                        json!({"handle":attempt.plugin.handle,"attemptID":id}),
                    )
                    .await;
            }
            return Ok(Value::Null);
        }
        let mut entries = self.attempts.entries.lock().await;
        let attempt = entries
            .get_mut(id)
            .ok_or_else(|| LegacyError::Internal(format!("OAuth attempt not found: {id}")))?;
        if attempt.status == "pending" && attempt.expires <= now() {
            attempt.cancel.cancel();
            attempt.status = "expired".into();
            attempt.remove_at = now() + 60000;
        }
        if operation == "v2.integration.attempt.status" {
            let mut result = json!({"status":attempt.status,"time":{"created":attempt.created,"expires":attempt.expires}});
            if let Some(message) = &attempt.message {
                result["message"] = json!(message);
            }
            return Ok(result);
        }
        if attempt.status != "pending" {
            return Ok(Value::Null);
        }
        let code = payload
            .as_ref()
            .and_then(|payload| payload.get("code"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        if attempt.mode == "code" && code.as_deref().is_none_or(str::is_empty) {
            return Err(LegacyError::Response {
                status: 400,
                body: json!({"_tag":"InvalidRequestError","message":"Authorization code is required","kind":"integration_code_required"}),
            });
        }
        if attempt.completing {
            return Err(LegacyError::Internal(format!(
                "OAuth attempt already completing: {id}"
            )));
        }
        attempt.completing = true;
        let attempt = attempt.clone();
        drop(entries);
        complete(
            self.attempts.entries.clone(),
            self.database.clone(),
            id.to_owned(),
            attempt,
            code,
        )
        .await?;
        Ok(Value::Null)
    }
}

async fn complete(
    entries: Arc<Mutex<BTreeMap<String, Attempt>>>,
    database: Arc<dyn Database>,
    id: String,
    attempt: Attempt,
    code: Option<String>,
) -> Result<(), LegacyError> {
    let result = tokio::select! {
        _=attempt.cancel.cancelled()=>return Ok(()),
        result=attempt.plugin.host.request("auth.callback",json!({"handle":attempt.plugin.handle,"attemptID":id,"code":code}))=>result,
    };
    let mut entries = entries.lock().await;
    let Some(current) = entries
        .get_mut(&id)
        .filter(|current| current.status == "pending")
    else {
        return Ok(());
    };
    let value = result.ok().and_then(|mut value| {
        value.get("refresh")?;
        value["type"] = json!("oauth");
        value["methodID"] = json!(attempt.method);
        serde_json::from_value::<credentials::CredentialValue>(value).ok()
    });
    let outcome = match value {
        Some(value) => database
            .transaction(TxOptions::default(), move |sql| {
                credentials::create(sql, &attempt.integration, &value, attempt.label.as_deref())
                    .map(|_| ())
            })
            .await
            .map_err(|_| ()),
        None => Err(()),
    };
    current.status = if outcome.is_ok() {
        "complete"
    } else {
        "failed"
    }
    .into();
    current.remove_at = now() + 60000;
    if outcome.is_err() {
        current.message = Some("Authentication failed".into());
    }
    drop(entries);
    let _ = attempt
        .plugin
        .host
        .request(
            "auth.cancel",
            json!({"handle":attempt.plugin.handle,"attemptID":id}),
        )
        .await;
    outcome.map_err(|_| authorization())
}
