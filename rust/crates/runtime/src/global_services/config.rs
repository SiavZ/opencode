//! Purpose: legacy configuration loading and real HTTP remote source transport.
//! Scope: captured Environment and blocking configuration boundary.
//! Uses:
//! rust/crates/
//! |-- config/src/legacy/mod.rs
//! `-- runtime/src/global_services.rs
//! External:
//! |-- reqwest::Client
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! `-- runtime/src/global_services.rs
//! Invariants: network sources are explicit; the host environment is never mutated.
//! Contract tests: runtime/tests/global_services.rs

use super::{GlobalServices, LegacyError, Value, internal};
use opencode_config::legacy::{
    self, AccountSource, LegacyState, RemoteFetcher, RemoteResponse, WellKnownAuth,
};
use opencode_config::{Environment, RemoteError};
use std::collections::BTreeMap;
use std::time::Duration;

/// Synchronous loader transport backed by a private Tokio runtime on its blocking caller.
/// This is deliberately not reqwest::blocking, which can panic on an async runtime thread.
#[derive(Clone)]
pub struct RemoteConfiguration {
    environment: Environment,
}

impl RemoteConfiguration {
    pub fn new(environment: Environment) -> Self {
        Self { environment }
    }
}

impl RemoteFetcher for RemoteConfiguration {
    fn get(
        &self,
        url: &str,
        headers: &BTreeMap<String, String>,
    ) -> Result<RemoteResponse, RemoteError> {
        let failure = |message: String| RemoteError::Fetch {
            url: url.into(),
            message,
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| failure(error.to_string()))?;
        runtime.block_on(async {
            let mut builder = reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(30))
                .redirect(reqwest::redirect::Policy::limited(10));
            for key in ["HTTPS_PROXY", "HTTP_PROXY", "ALL_PROXY"] {
                if let Some(proxy) = self.environment.var(key) {
                    builder = builder.proxy(
                        reqwest::Proxy::all(proxy).map_err(|error| failure(error.to_string()))?,
                    );
                    break;
                }
            }
            let client = builder
                .build()
                .map_err(|error| failure(error.to_string()))?;
            let mut last = None;
            for attempt in 0..3 {
                let request = headers.iter().fold(
                    client.get(url).header("accept", "application/json"),
                    |request, (key, value)| request.header(key, value),
                );
                match request.send().await {
                    Ok(response) => {
                        let status = response.status().as_u16();
                        let content_type = response
                            .headers()
                            .get("content-type")
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or("")
                            .to_owned();
                        let body = response
                            .text()
                            .await
                            .map_err(|error| failure(error.to_string()))?;
                        return Ok(RemoteResponse {
                            status,
                            content_type,
                            body,
                        });
                    }
                    Err(error) => last = Some(error.to_string()),
                }
                if attempt < 2 {
                    tokio::time::sleep(Duration::from_millis(100 * (attempt + 1))).await;
                }
            }
            Err(failure(last.unwrap_or_else(|| "Remote read failed".into())))
        })
    }
}

struct Accounts(Value);
impl AccountSource for Accounts {
    fn wellknown(&self) -> Vec<WellKnownAuth> {
        self.0
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(url, entry)| {
                (entry.get("type")?.as_str()? == "wellknown").then_some(WellKnownAuth {
                    url: url.clone(),
                    key: entry.get("key")?.as_str()?.into(),
                    token: entry.get("token")?.as_str()?.into(),
                })
            })
            .collect()
    }
}

impl GlobalServices {
    pub(super) async fn config(&self, directory: &str) -> Result<LegacyState, LegacyError> {
        let project = self
            .projects
            .from_directory(directory)
            .await
            .map_err(internal)?;
        let instance = legacy::Instance {
            directory: directory.into(),
            worktree: project.sandbox.into(),
        };
        let env = self.options.environment.clone();
        let auth = self.options.paths.data.join("auth.json");
        tokio::task::spawn_blocking(move || {
            let accounts = Accounts(
                std::fs::read(auth)
                    .ok()
                    .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                    .unwrap_or(Value::Null),
            );
            legacy::load(
                &env,
                &instance,
                &legacy::Sources {
                    accounts: &accounts,
                    remote: &RemoteConfiguration::new(env.clone()),
                    managed: &legacy::SystemManagedPreferences::default(),
                },
            )
            .map_err(error)
        })
        .await
        .map_err(internal)?
    }

    pub(super) async fn global_config(&self) -> Result<Value, LegacyError> {
        let env = self.options.environment.clone();
        tokio::task::spawn_blocking(move || {
            legacy::parse::load_global(&env, None, &mut legacy::Diagnostics::default())
                .map_err(error)
        })
        .await
        .map_err(internal)?
    }

    pub(super) async fn update_global(&self, payload: Value) -> Result<Value, LegacyError> {
        let env = self.options.environment.clone();
        let result = tokio::task::spawn_blocking(move || {
            legacy::update_global(&env, &payload).map_err(error)
        })
        .await
        .map_err(internal)??;
        if result.changed {
            self.locations.invalidate_all().await;
            self.publish("global.disposed", "global", serde_json::json!({}))
                .await?;
        }
        Ok(result.info)
    }
}

pub(super) fn error(error: opencode_config::ConfigError) -> LegacyError {
    LegacyError::Response {
        status: 400,
        body: error.to_named_json(),
    }
}
