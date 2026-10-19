//! Purpose: `provider.ts` (Provider.Api, Request, Info) and `model.ts` (Model.Ref,
//! Capabilities, Cost, Api, Info) provider and model catalog values.
//! Scope: pure values.
//! Uses:
//! rust/crates/
//! |-- schema/src/ids.rs
//! |-- schema/src/lib.rs
//! `-- schema/src/values.rs
//! External:
//! |-- indexmap::IndexMap
//! |-- serde::Deserialize
//! |-- serde::Serialize
//! `-- serde_json::from_str
//! Used by:
//! rust/crates/
//! |-- config/src/provider.rs
//! |-- config/tests/markdown.rs
//! |-- provider-adapters/src/catalog/cache.rs
//! |-- provider-adapters/src/catalog/mod.rs
//! |-- provider-adapters/tests/catalog.rs
//! |-- runtime/src/location.rs
//! |-- runtime/src/location_agents.rs
//! |-- runtime/src/location_models.rs
//! |-- runtime/src/process.rs
//! |-- schema/src/agent.rs
//! |-- schema/src/lib.rs
//! |-- schema/src/session.rs
//! |-- schema/src/session_event.rs
//! |-- schema/src/session_event/stream.rs
//! |-- schema/src/session_message.rs
//! |-- session-api/src/ids.rs
//! |-- session-runtime/src/runner/model.rs
//! |-- session-runtime/src/runner/publish.rs
//! |-- session-runtime/src/runner/turn/attempt.rs
//! |-- session-runtime/src/store.rs
//! |-- session-runtime/tests/runner_model_location.rs
//! |-- session-runtime/tests/support/model.rs
//! `-- session-runtime/tests/support/publish.rs
//! Invariants: Provider.Api is tagged by `type` (tag first); Model.Api puts `id` before the
//! spread provider api fields; `settings` is required for `native` and optional for `aisdk`.
//! Contract tests: schema/src/provider.rs (unit tests), schema/tests/baseline_fixtures.rs

use crate::ids::{IntegrationID, ModelFamily, ModelID, ProviderID, VariantID};
use crate::values::{Finite, Int, JsonObject, optional};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

/// `Provider.Api` (`Provider.AISDK` | `Provider.Native`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ProviderApi {
    #[serde(rename = "aisdk")]
    Aisdk {
        package: String,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "optional::deserialize"
        )]
        url: Option<String>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "optional::deserialize"
        )]
        settings: Option<JsonObject>,
    },
    #[serde(rename = "native")]
    Native {
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "optional::deserialize"
        )]
        url: Option<String>,
        settings: JsonObject,
    },
}

/// `Provider.Request`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ProviderRequest {
    pub headers: IndexMap<String, String>,
    pub body: JsonObject,
}

/// `ProviderV2.Info`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderInfo {
    pub id: ProviderID,
    #[serde(
        rename = "integrationID",
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub integration_id: Option<IntegrationID>,
    pub name: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub disabled: Option<bool>,
    pub api: ProviderApi,
    pub request: ProviderRequest,
}

impl ProviderInfo {
    /// `Provider.Info.empty(id)`.
    pub fn empty(id: ProviderID) -> Self {
        Self {
            name: id.as_str().to_string(),
            id,
            integration_id: None,
            disabled: None,
            api: ProviderApi::Native {
                url: None,
                settings: JsonObject::new(),
            },
            request: ProviderRequest::default(),
        }
    }
}

/// `Model.Ref`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ModelRef {
    pub id: ModelID,
    #[serde(rename = "providerID")]
    pub provider_id: ProviderID,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub variant: Option<VariantID>,
}

/// `Model.Capabilities`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ModelCapabilities {
    pub tools: bool,
    pub input: Vec<String>,
    pub output: Vec<String>,
}

crate::literal!(
    /// `Schema.Literal("context")` for `Model.Cost.tier.type`.
    ContextTier = "context"
);

/// `Model.Cost.tier`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelCostTier {
    pub r#type: ContextTier,
    pub size: Int,
}

/// `{ read, write }` cache cost or token pair.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CacheCost {
    pub read: Finite,
    pub write: Finite,
}

/// `Model.Cost`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelCost {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub tier: Option<ModelCostTier>,
    pub input: Finite,
    pub output: Finite,
    pub cache: CacheCost,
}

/// `Model.Api`: `{ id, ...Provider.AISDK | Provider.Native }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelApi {
    pub id: ModelID,
    #[serde(flatten)]
    pub api: ProviderApi,
}

/// `ModelV2.Info.request`: provider request plus optional `variant`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ModelRequest {
    pub headers: IndexMap<String, String>,
    pub body: JsonObject,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub variant: Option<String>,
}

/// `ModelV2.Info.variants[]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelVariant {
    pub id: VariantID,
    pub headers: IndexMap<String, String>,
    pub body: JsonObject,
}

/// `ModelV2.Info.time`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ModelTime {
    pub released: Finite,
}

/// `ModelV2.Info.status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelStatus {
    Alpha,
    Beta,
    Deprecated,
    Active,
}

/// `ModelV2.Info.limit`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelLimit {
    pub context: Int,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub input: Option<Int>,
    pub output: Int,
}

/// `ModelV2.Info`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: ModelID,
    #[serde(rename = "providerID")]
    pub provider_id: ProviderID,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub family: Option<ModelFamily>,
    pub name: String,
    pub api: ModelApi,
    pub capabilities: ModelCapabilities,
    pub request: ModelRequest,
    pub variants: Vec<ModelVariant>,
    pub time: ModelTime,
    pub cost: Vec<ModelCost>,
    pub status: ModelStatus,
    pub enabled: bool,
    pub limit: ModelLimit,
}

impl ModelInfo {
    /// `Model.Info.empty(providerID, modelID)`.
    pub fn empty(provider_id: ProviderID, model_id: ModelID) -> Self {
        let zero = Int::ZERO;
        Self {
            name: model_id.as_str().to_string(),
            api: ModelApi {
                id: model_id.clone(),
                api: ProviderApi::Native {
                    url: None,
                    settings: JsonObject::new(),
                },
            },
            id: model_id,
            provider_id,
            family: None,
            capabilities: ModelCapabilities::default(),
            request: ModelRequest::default(),
            variants: Vec::new(),
            time: ModelTime::default(),
            cost: Vec::new(),
            status: ModelStatus::Active,
            enabled: true,
            limit: ModelLimit {
                context: zero,
                input: None,
                output: zero,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_api_puts_id_first() {
        let api: ModelApi =
            serde_json::from_str(r#"{"settings":{"b":1,"a":2},"type":"native","id":"x"}"#).unwrap();
        // Observed baseline: {"id":"x","type":"native","settings":{"b":1,"a":2}}
        assert_eq!(
            serde_json::to_string(&api).unwrap(),
            r#"{"id":"x","type":"native","settings":{"b":1,"a":2}}"#
        );
        assert!(serde_json::from_str::<ModelApi>(r#"{"id":"x","type":"native"}"#).is_err());
    }

    #[test]
    fn model_ref_order() {
        let r: ModelRef =
            serde_json::from_str(r#"{"variant":"v","providerID":"p","id":"x"}"#).unwrap();
        assert_eq!(
            serde_json::to_string(&r).unwrap(),
            r#"{"id":"x","providerID":"p","variant":"v"}"#
        );
    }

    #[test]
    fn empty_model_matches_baseline_shape() {
        let info = ModelInfo::empty(ProviderID::new("p"), ModelID::new("m"));
        assert_eq!(
            serde_json::to_string(&info).unwrap(),
            r#"{"id":"m","providerID":"p","name":"m","api":{"id":"m","type":"native","settings":{}},"capabilities":{"tools":false,"input":[],"output":[]},"request":{"headers":{},"body":{}},"variants":[],"time":{"released":0},"cost":[],"status":"active","enabled":true,"limit":{"context":0,"output":0}}"#
        );
    }
}
