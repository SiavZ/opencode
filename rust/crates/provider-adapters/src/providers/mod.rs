//! Purpose: provider facades and registrations (packages/llm/src/providers,
//! packages/core/src/plugin/provider/*): routes, auth, headers and model construction per ID,
//! plus the pure request-transform and error-classification rules from
//! packages/opencode/src/provider/{transform,error}.ts.
//! Scope: module root. Owner: child node p03.providers.
//! Uses:
//! rust/crates/
//! |-- provider-adapters/src/providers/azure.rs
//! |-- provider-adapters/src/providers/bedrock.rs
//! |-- provider-adapters/src/providers/cloud.rs
//! |-- provider-adapters/src/providers/compatible.rs
//! |-- provider-adapters/src/providers/config.rs
//! |-- provider-adapters/src/providers/error.rs
//! |-- provider-adapters/src/providers/gateways.rs
//! |-- provider-adapters/src/providers/inventory.rs
//! |-- provider-adapters/src/providers/loaders.rs
//! |-- provider-adapters/src/providers/native.rs
//! |-- provider-adapters/src/providers/openai.rs
//! |-- provider-adapters/src/providers/registry.rs
//! |-- provider-adapters/src/providers/registry_rules.rs
//! |-- provider-adapters/src/providers/transform.rs
//! `-- provider-adapters/src/providers/wrap.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! `-- provider-adapters/src/lib.rs
//! Invariants: every registered provider ID resolves or fails with a typed error.
//! Contract tests: provider-adapters/tests/providers_*.rs

pub mod azure;
pub mod bedrock;
pub mod cloud;
pub mod compatible;
pub mod config;
pub mod error;
pub mod gateways;
pub mod gitlab_dws_discovery;
pub mod inventory;
pub mod loaders;
pub mod native;
pub mod openai;
pub mod registry;
mod registry_rules;
pub mod transform;
pub mod transform_message;
pub mod wrap;

pub use config::{ProviderConfig, Transports};
pub use inventory::{Api, CatalogRoute, PLUGINS, Plugin, Support};
pub use registry::ProviderRegistry;
