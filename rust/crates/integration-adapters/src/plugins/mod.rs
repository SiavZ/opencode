//! Purpose: load external plugins through the Rust-defined compatibility bridge.
//! Scope: one Location, with deterministic module export and hook order.
//! Uses:
//! rust/crates/
//! |-- integration-adapters/src/plugins/instance.rs
//! |-- integration-adapters/src/plugins/process.rs
//! `-- integration-api/src/plugin_abi/mod.rs
//! External:
//! |-- serde_json::Value
//! `-- tokio::fs
//! Used by:
//! rust/crates/
//! `-- integration-adapters/src/lib.rs
//! Invariants: only an explicitly supplied interpreter executes foreign code; no package installation.
//! Contract tests: integration-adapters/tests/plugin_host.rs

mod instance;
mod process;
pub use instance::Instance;
pub use process::Callbacks;

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use opencode_integration_api::IntegrationError;
use opencode_integration_api::plugin::{PluginId, PluginInstance};
use opencode_integration_api::plugin_abi::{
    self, ImportResult, InitMember, InitParams, InitResult, PluginInputWire,
};
use serde_json::{Value, json};

pub struct Options {
    pub interpreter: PathBuf,
    pub bridge_directory: PathBuf,
    pub directory: PathBuf,
    pub environment: BTreeMap<String, String>,
    pub zod_fallback: Option<PathBuf>,
    pub callbacks: Arc<dyn Callbacks>,
}

pub struct Host {
    process: process::Process,
}

impl Host {
    pub async fn start(options: Options) -> Result<Arc<Self>, IntegrationError> {
        tokio::fs::create_dir_all(&options.bridge_directory)
            .await
            .map_err(io_error)?;
        let bridge = options.bridge_directory.join("plugin-bridge-v1.mjs");
        tokio::fs::write(&bridge, plugin_abi::bridge_script())
            .await
            .map_err(io_error)?;
        let host = Arc::new(Self {
            process: process::Process::spawn(
                &options.interpreter,
                &bridge,
                &options.directory,
                &options.environment,
                options.callbacks,
            )?,
        });
        let initialized = host
            .request(
                plugin_abi::INITIALIZE,
                json!({"abiVersion":plugin_abi::ABI_VERSION,"zodFallback":options.zod_fallback}),
            )
            .await;
        if let Err(error) = initialized {
            host.close().await;
            return Err(error);
        }
        Ok(host)
    }

    pub async fn request(&self, method: &str, params: Value) -> Result<Value, IntegrationError> {
        self.process
            .request(method, params, Duration::from_secs(30))
            .await
    }

    pub async fn notify(&self, method: &str, params: Value) -> Result<(), IntegrationError> {
        self.process.notify(method, params).await
    }

    /// Rust owns module shape selection and export de-duplication; the bridge only executes.
    pub async fn load(
        self: &Arc<Self>,
        entry: &Path,
        options: Option<Value>,
        input: PluginInputWire,
    ) -> Result<Vec<Arc<Instance>>, IntegrationError> {
        let entry = url::Url::from_file_path(entry)
            .map_err(|()| IntegrationError::Validation("plugin entry must be absolute".into()))?;
        let imported: ImportResult = serde_json::from_value(
            self.request(plugin_abi::MODULE_IMPORT, json!({"entry":entry.as_str()}))
                .await?,
        )
        .map_err(decode_error)?;
        let identified: Vec<_> = imported
            .exports
            .iter()
            .filter(|export| export.id.is_some() && export.server.as_deref() == Some("function"))
            .collect();
        if identified.len() > 1 {
            return Err(IntegrationError::Validation(
                "multiple v1 plugin exports".into(),
            ));
        }
        let selected = if let Some(export) = identified.first() {
            vec![(*export, InitMember::Server)]
        } else {
            let mut seen = HashSet::new();
            imported
                .exports
                .iter()
                .filter(|export| seen.insert(export.identity))
                .map(|export| {
                    if export.kind == "function" {
                        return Ok((export, InitMember::Itself));
                    }
                    if export.server.as_deref() == Some("function") {
                        return Ok((export, InitMember::Server));
                    }
                    Err(IntegrationError::Validation(
                        "Plugin export is not a function".into(),
                    ))
                })
                .collect::<Result<Vec<_>, _>>()?
        };
        let mut instances = Vec::new();
        for (export, member) in selected {
            let params = InitParams {
                module: imported.module,
                export: export.name.clone(),
                member,
                options: options.clone(),
                input: input.clone(),
            };
            let result: InitResult = serde_json::from_value(
                self.request(
                    plugin_abi::PLUGIN_INIT,
                    serde_json::to_value(params).map_err(decode_error)?,
                )
                .await?,
            )
            .map_err(decode_error)?;
            instances.push(Arc::new(Instance::new(
                self.clone(),
                PluginId(
                    export
                        .id
                        .clone()
                        .unwrap_or_else(|| format!("{}:{}", entry, export.name)),
                ),
                result,
            )?));
        }
        Ok(instances)
    }

    pub async fn dispose(&self, plugins: &[Arc<dyn PluginInstance>]) {
        for plugin in plugins {
            if let Err(error) = plugin.dispose().await {
                tracing::warn!(%error, "plugin dispose failed");
            }
        }
        let _ = self
            .process
            .request(plugin_abi::SHUTDOWN, Value::Null, Duration::from_secs(2))
            .await;
        self.close().await;
    }

    pub async fn close(&self) {
        self.process.close().await;
    }
}

fn io_error(error: std::io::Error) -> IntegrationError {
    IntegrationError::Failed {
        name: "plugin bridge".into(),
        message: error.to_string(),
    }
}
fn decode_error(error: serde_json::Error) -> IntegrationError {
    IntegrationError::Validation(error.to_string())
}
