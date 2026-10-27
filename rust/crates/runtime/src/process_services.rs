//! Purpose: public host facade over the same Location services used by Session execution.
//! Scope: CLI and terminal access to typed agents, models, permission requests and legacy operations.
//! Uses:
//! rust/crates/
//! |-- runtime/src/process.rs
//! |-- runtime/src/location.rs
//! |-- permissions/src/lib.rs
//! |-- session-runtime/src/runner/model.rs
//! `-- server/src/services/legacy.rs
//! External:
//! `-- tokio::fs::canonicalize
//! Used by:
//! `-- none, reserved contract (owner: phase-90, CLI and terminal integration)
//! Invariants: a consumer never constructs a second permission service or bypasses Location placement.
//! Contract tests: runtime/tests/permissions.rs, runtime/tests/production.rs

use std::path::PathBuf;
use std::sync::Arc;

use opencode_schema::location::Ref;
use opencode_schema::provider::ModelRef;
use opencode_schema::values::AbsolutePath;
use opencode_server::middleware::legacy_routing::RouteContext;
use opencode_server::services::legacy::LegacySessions;
use opencode_session_runtime::runner::model::ModelCatalog;
use opencode_storage_api::{DatabaseExt, TxOptions};
use serde_json::{Map, Value};

use crate::{LocationBundle, LocationGlobals, Process, ProcessError};

impl Process {
    pub(crate) async fn location_services(
        &self,
        directory: PathBuf,
    ) -> Result<opencode_locations::LocationLease<LocationBundle, LocationGlobals>, ProcessError>
    {
        let directory = tokio::fs::canonicalize(directory).await?;
        Ok(self
            .locations
            .get(&Ref {
                directory: AbsolutePath::new(directory.to_string_lossy()),
                workspace_id: None,
            })
            .await?)
    }

    pub async fn available_agents(
        &self,
        directory: PathBuf,
    ) -> Result<Vec<opencode_schema::agent::Info>, ProcessError> {
        Ok(self.location_services(directory).await?.agents.all())
    }

    pub async fn default_model(
        &self,
        directory: PathBuf,
    ) -> Result<Option<ModelRef>, ProcessError> {
        Ok(self
            .location_services(directory)
            .await?
            .catalog
            .default_model()
            .await
            .map(|model| ModelRef {
                id: model.id,
                provider_id: model.provider_id,
                variant: None,
            }))
    }

    pub async fn pending_permissions(
        &self,
        directory: PathBuf,
    ) -> Result<Vec<opencode_permissions::PermissionRequest>, ProcessError> {
        Ok(self
            .location_services(directory)
            .await?
            .permissions
            .service
            .list())
    }

    pub async fn reply_permission(
        &self,
        directory: PathBuf,
        input: opencode_permissions::ReplyInput,
    ) -> Result<(), ProcessError> {
        self.location_services(directory)
            .await?
            .permissions
            .service
            .reply(input)
            .await?;
        Ok(())
    }

    pub fn legacy_sessions(&self) -> Arc<dyn LegacySessions> {
        self.legacy.clone()
    }

    pub async fn pending_questions(
        &self,
        directory: PathBuf,
    ) -> Result<Vec<crate::QuestionRequest>, ProcessError> {
        Ok(self.location_services(directory).await?.questions.list())
    }

    pub async fn reply_question(
        &self,
        directory: PathBuf,
        request_id: &str,
        answers: Vec<crate::QuestionAnswer>,
    ) -> Result<(), ProcessError> {
        self.location_services(directory)
            .await?
            .questions
            .reply(request_id, answers)
            .await
            .map_err(|error| ProcessError::Service(error.to_string()))
    }

    pub async fn reject_question(
        &self,
        directory: PathBuf,
        request_id: &str,
    ) -> Result<(), ProcessError> {
        self.location_services(directory)
            .await?
            .questions
            .reject(request_id)
            .await
            .map_err(|error| ProcessError::Service(error.to_string()))
    }

    pub async fn legacy_command(
        &self,
        operation: &str,
        directory: PathBuf,
        params: Map<String, Value>,
        query: Map<String, Value>,
        payload: Option<Value>,
    ) -> Result<Value, ProcessError> {
        let directory = tokio::fs::canonicalize(directory).await?;
        let services = self.services();
        let service = crate::service_router::domain(&services, operation)
            .ok_or_else(|| ProcessError::Service(format!("service unavailable for {operation}")))?;
        Ok(service
            .handle(
                operation,
                &RouteContext {
                    directory: directory.to_string_lossy().into_owned(),
                    workspace_id: None,
                },
                &params,
                &query,
                payload,
            )
            .await?)
    }

    pub fn database_path(&self) -> PathBuf {
        opencode_storage_sqlite::database_path(
            &self.options.paths.data,
            &self.options.channel,
            &self.options.db,
        )
    }

    /// Execute explicit local SQL with the same write-capable semantics as the baseline CLI.
    pub async fn database_query(&self, query: String) -> Result<Vec<Value>, ProcessError> {
        let rows = self
            .database
            .transaction::<_, opencode_storage_api::StorageError, _>(
                TxOptions::default(),
                move |sql| sql.query(&query, &[]),
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|row| {
                Value::Object(
                    row.columns()
                        .iter()
                        .cloned()
                        .zip(row.values().iter().map(|value| {
                            match value {
                                opencode_storage_api::Value::Null => Value::Null,
                                opencode_storage_api::Value::Integer(value) => Value::from(*value),
                                opencode_storage_api::Value::Real(value) => Value::from(*value),
                                opencode_storage_api::Value::Text(value) => {
                                    Value::String(value.clone())
                                }
                                opencode_storage_api::Value::Blob(bytes) => Value::Object(
                                    bytes
                                        .iter()
                                        .enumerate()
                                        .map(|(index, byte)| {
                                            (index.to_string(), Value::from(*byte))
                                        })
                                        .collect(),
                                ),
                            }
                        }))
                        .collect(),
                )
            })
            .collect())
    }

    pub async fn resolved_config(&self, directory: PathBuf) -> Result<Value, ProcessError> {
        let directory = tokio::fs::canonicalize(directory).await?;
        Ok(self
            .global
            .resolved_config(&directory.to_string_lossy())
            .await?)
    }

    pub async fn refresh_models(
        &self,
        directory: PathBuf,
        force: bool,
    ) -> Result<bool, ProcessError> {
        self.location_services(directory)
            .await?
            .providers
            .refresh_models(force)
            .await
            .map_err(ProcessError::Service)
    }

    pub async fn models_catalog(&self) -> Result<Value, ProcessError> {
        let catalog = self
            .models
            .get()
            .await
            .map_err(|error| ProcessError::Service(error.to_string()))?;
        Ok(Value::Object(
            catalog
                .providers()
                .into_iter()
                .filter_map(|provider| {
                    catalog
                        .metadata(&provider.id)
                        .cloned()
                        .map(|value| (provider.id.to_string(), value))
                })
                .collect(),
        ))
    }

    pub async fn global_auth_list(&self) -> Result<Value, ProcessError> {
        self.providers
            .auth_list()
            .await
            .map_err(ProcessError::Service)
    }

    pub async fn auth_list(&self, _directory: PathBuf) -> Result<Value, ProcessError> {
        self.global_auth_list().await
    }

    pub async fn auth_set(
        &self,
        _directory: PathBuf,
        provider: &str,
        credential: Value,
    ) -> Result<(), ProcessError> {
        self.providers
            .auth_set(provider, credential)
            .await
            .map_err(ProcessError::Service)
    }

    pub async fn auth_remove(
        &self,
        _directory: PathBuf,
        provider: &str,
    ) -> Result<(), ProcessError> {
        self.global_auth_remove(provider).await
    }

    pub async fn global_auth_remove(&self, provider: &str) -> Result<(), ProcessError> {
        self.providers
            .auth_remove(provider)
            .await
            .map_err(ProcessError::Service)
    }
}

pub(crate) struct LocationProviders {
    pub locations: crate::location::Locations,
    pub global: std::sync::Arc<crate::provider_services::ProviderServices>,
}

#[async_trait::async_trait]
impl LegacySessions for LocationProviders {
    async fn handle(
        &self,
        operation: &str,
        context: &RouteContext,
        params: &Map<String, Value>,
        query: &Map<String, Value>,
        payload: Option<Value>,
    ) -> Result<Value, opencode_server::services::legacy::LegacyError> {
        if matches!(operation, "auth.set" | "auth.remove") {
            return self
                .global
                .handle(operation, context, params, query, payload)
                .await;
        }
        let location = Ref {
            directory: AbsolutePath::new(context.directory.clone()),
            workspace_id: context
                .workspace_id
                .as_deref()
                .map(opencode_schema::WorkspaceID::new)
                .transpose()
                .map_err(|error| {
                    opencode_server::services::legacy::LegacyError::BadRequest(error.to_string())
                })?,
        };
        let services = self.locations.get(&location).await.map_err(|error| {
            opencode_server::services::legacy::LegacyError::Internal(error.to_string())
        })?;
        if !operation.starts_with("v2.") {
            let providers = services.legacy_config.resolve(context).await?;
            providers
                .set_plugins(services.integrations.plugins.clone())
                .await;
            let result = providers
                .handle(operation, context, params, query, payload)
                .await;
            providers.shutdown().await;
            return result;
        }
        services
            .providers
            .handle(operation, context, params, query, payload)
            .await
    }
}
