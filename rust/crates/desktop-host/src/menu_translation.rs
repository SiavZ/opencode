//! Purpose: desktop translation bundles, English fallback, and placeholder interpolation.
//! Scope: toolkit-independent locale state and message formatting, not menu construction.
//! Uses:
//! rust/crates/
//! `-- desktop-host/src/menu_english.rs
//! External:
//! |-- serde::Deserialize
//! `-- serde::Serialize
//! Used by:
//! rust/crates/
//! `-- desktop-host/src/menu.rs
//! Invariants: missing messages fall back to English, then the key; bundle equality controls updates.
//! Contract tests: desktop-host/tests/menu_translations.rs, menu.rs unit tests

use super::menu_english::english_messages;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Translation bundle for menu labels.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranslationBundle {
    /// Locale identifier.
    pub locale: String,
    /// Translation messages keyed by DesktopNativeKey.
    pub messages: HashMap<String, String>,
}

/// Translation manager with English fallback.
#[derive(Debug, Clone)]
pub struct TranslationManager {
    /// Current bundle.
    bundle: TranslationBundle,
    /// English fallback messages.
    english: HashMap<String, String>,
}

impl TranslationManager {
    /// Create new translation manager with English fallback.
    pub fn new(bundle: TranslationBundle) -> Self {
        let english = english_messages();
        Self { bundle, english }
    }

    /// Update translation bundle. Returns true if bundle changed.
    pub fn set_bundle(&mut self, bundle: TranslationBundle) -> bool {
        if bundle.locale == self.bundle.locale && bundle.messages == self.bundle.messages {
            return false;
        }
        self.bundle = bundle;
        true
    }

    /// Translate a key with English fallback for missing keys.
    pub fn translate(&self, key: &str) -> String {
        self.bundle
            .messages
            .get(key)
            .or_else(|| self.english.get(key))
            .cloned()
            .unwrap_or_else(|| key.to_string())
    }

    /// Translate a key with parameter substitution and English fallback.
    pub fn translate_with_params(&self, key: &str, params: &HashMap<String, String>) -> String {
        let template = self.translate(key);
        format_message(&template, params)
    }

    /// Get current locale.
    pub fn locale(&self) -> &str {
        &self.bundle.locale
    }

    /// Get the current bundle.
    pub fn bundle(&self) -> &TranslationBundle {
        &self.bundle
    }
}

/// Format a message template by replacing `{{key}}` placeholders.
/// Matches TypeScript formatDesktopNativeMessage behavior.
pub fn format_message(template: &str, params: &HashMap<String, String>) -> String {
    let mut result = template.to_string();
    for (key, value) in params {
        let placeholder = format!("{{{{{}}}}}", key);
        result = result.replace(&placeholder, value);
    }
    result
}
