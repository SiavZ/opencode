//! Purpose: preserve distinct V1 default ordering and V2 small-model selection.
//! Scope: deterministic selection over the concrete catalog with an explicit clock.
//! Uses:
//! rust/crates/
//! |-- provider-adapters/src/catalog/mod.rs
//! |-- schema/src/ids.rs
//! `-- schema/src/provider.rs
//! External:
//! `-- regex::Regex
//! Used by:
//! rust/crates/
//! `-- provider-adapters/src/catalog/mod.rs
//! Invariants: V1 priority is not V2 release order; explicit defaults retain their exact identity.
//! Contract tests: provider-adapters/tests/catalog_selection.rs

use super::CatalogStore;
use opencode_schema::ids::{ModelID, ProviderID};
use opencode_schema::provider::ModelInfo;

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum SelectionError {
    #[error("No providers available")]
    NoProviders,
    #[error("No models available for provider: {0}")]
    NoModels(ProviderID),
}

impl CatalogStore {
    /// Configured selection is returned unchanged, including model IDs containing slashes.
    pub fn legacy_default(
        &self,
        configured: &[ProviderID],
        recent: &[(ProviderID, ModelID)],
    ) -> Result<(ProviderID, ModelID), SelectionError> {
        if let Some(selection) = &self.default {
            return Ok(selection.clone());
        }
        let available = self.available_models();
        if let Some(selection) = recent.iter().find(|(provider, model)| {
            available
                .iter()
                .any(|item| &item.provider_id == provider && &item.id == model)
        }) {
            return Ok(selection.clone());
        }
        let provider = self
            .available_providers()
            .into_iter()
            .find(|provider| configured.is_empty() || configured.contains(&provider.id))
            .ok_or(SelectionError::NoProviders)?;
        let model = self
            .legacy_default_for(&provider.id)
            .ok_or_else(|| SelectionError::NoModels(provider.id.clone()))?;
        Ok((provider.id, model.id))
    }

    /// V1 priority, then latest alias, then descending ID. V2 keeps release-date ordering.
    pub fn legacy_default_for(&self, provider: &ProviderID) -> Option<ModelInfo> {
        let mut models: Vec<_> = self
            .models_for(provider)
            .into_iter()
            .filter(|model| model.enabled)
            .collect();
        models.sort_by(|left, right| {
            let rank = |id: &str| {
                ["gpt-5", "claude-sonnet-4", "big-pickle", "gemini-3-pro"]
                    .iter()
                    .position(|filter| id.contains(filter))
                    .map(|index| index as i32)
                    .unwrap_or(-1)
            };
            rank(right.id.as_str())
                .cmp(&rank(left.id.as_str()))
                .then_with(|| {
                    right
                        .id
                        .as_str()
                        .contains("latest")
                        .cmp(&left.id.as_str().contains("latest"))
                })
                .then_with(|| right.id.as_str().cmp(left.id.as_str()))
        });
        models.into_iter().next()
    }

    pub fn small_model(&self, provider: &ProviderID, now_ms: f64) -> Option<ModelInfo> {
        if ["azure", "azure-cognitive-services"].contains(&provider.as_str()) {
            return None;
        }
        let models = self.models_for(provider);
        if provider.as_str() == "opencode"
            && let Some(model) = models.iter().find(|model| {
                model.id.as_str() == "gpt-5-nano"
                    && model.enabled
                    && model.status == opencode_schema::provider::ModelStatus::Active
            })
        {
            return Some(model.clone());
        }
        let pattern = regex::Regex::new(r"\b(nano|flash|lite|mini|haiku|small|fast)\b")
            .expect("static small-model pattern");
        let candidates: Vec<_> = models
            .into_iter()
            .filter(|model| {
                model.enabled
                    && model.status == opencode_schema::provider::ModelStatus::Active
                    && model
                        .capabilities
                        .input
                        .iter()
                        .any(|kind| kind.starts_with("text"))
                    && model
                        .capabilities
                        .output
                        .iter()
                        .any(|kind| kind.starts_with("text"))
            })
            .map(|model| {
                let cost = model
                    .cost
                    .first()
                    .map(|cost| cost.input.get() + cost.output.get())
                    .unwrap_or(999.0);
                let age =
                    (now_ms - model.time.released.get()) / (1000.0 * 60.0 * 60.0 * 24.0 * 30.0);
                let small = pattern.is_match(
                    &format!(
                        "{} {} {}",
                        model.id.as_str(),
                        model
                            .family
                            .as_ref()
                            .map(|family| family.as_str())
                            .unwrap_or_default(),
                        model.name
                    )
                    .to_lowercase(),
                );
                (model, cost, age, small)
            })
            .filter(|(_, cost, age, _)| *cost > 0.0 && *age <= 18.0)
            .collect();
        let prefer_small = candidates.iter().any(|(_, _, _, small)| *small);
        let candidates: Vec<_> = candidates
            .into_iter()
            .filter(|(_, _, _, small)| !prefer_small || *small)
            .collect();
        let max_cost = candidates
            .iter()
            .map(|(_, cost, _, _)| *cost)
            .fold(0.01, f64::max);
        let max_age = candidates
            .iter()
            .map(|(_, _, age, _)| *age)
            .fold(0.01, f64::max);
        candidates
            .into_iter()
            .min_by(|left, right| {
                let score = |item: &(ModelInfo, f64, f64, bool)| {
                    item.1 / max_cost * 0.8 + item.2 / max_age * 0.2
                };
                score(left).total_cmp(&score(right))
            })
            .map(|(model, _, _, _)| model)
    }
}
