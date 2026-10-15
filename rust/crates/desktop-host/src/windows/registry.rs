//! Purpose: window registry with focus, close and quit semantics.
//! Scope: tracks open windows and persisted window IDs across app launches.
//! Uses:
//! rust/crates/
//! `-- desktop-host/src/windows.rs
//! External:
//! `-- std::collections::HashMap
//! Used by:
//! rust/crates/
//! `-- desktop-host/src/windows.rs
//! Invariants: quitting flag prevents cleanup of last window, focus fallback works.
//! Contract tests: desktop-host/tests/windows_registry.rs

use std::collections::HashMap;

/// Opaque window identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct WindowId(pub String);

impl WindowId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

pub trait WindowPersistence {
    fn read(&self) -> serde_json::Value;
    fn write(&self, ids: Vec<String>);
    fn cleanup(&self, id: &str);
}

pub struct WindowRegistry<W> {
    windows: HashMap<String, W>,
    quitting: bool,
    last_focused_id: Option<String>,
    persistence: Box<dyn WindowPersistence>,
}

impl<W> WindowRegistry<W> {
    pub fn new<P: WindowPersistence + 'static>(persistence: P) -> Self {
        Self {
            windows: HashMap::new(),
            quitting: false,
            last_focused_id: None,
            persistence: Box::new(persistence),
        }
    }

    pub fn persisted(&self) -> Vec<String> {
        let value = self.persistence.read();
        if let serde_json::Value::Array(arr) = value {
            arr.into_iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .filter(|s| !s.is_empty())
                .collect()
        } else {
            Vec::new()
        }
    }

    pub fn set_quitting(&mut self, value: bool) {
        self.quitting = value;
    }

    pub fn register(&mut self, id: String, window: W) {
        self.windows.insert(id.clone(), window);
        let mut ids = self.persisted();
        if !ids.contains(&id) {
            ids.push(id);
            self.persistence.write(ids);
        }
    }

    pub fn focused(&mut self, id: &str) {
        self.last_focused_id = Some(id.to_string());
    }

    pub fn last_focused(&self) -> Option<&W> {
        self.last_focused_id
            .as_ref()
            .and_then(|id| self.windows.get(id))
    }

    pub fn closed(&mut self, id: &str) {
        self.windows.remove(id);
        if self.last_focused_id.as_deref() == Some(id) {
            self.last_focused_id = self.windows.keys().next().cloned();
        }
        if self.quitting || self.windows.is_empty() {
            return;
        }
        let ids = self.persisted();
        let filtered: Vec<String> = ids.into_iter().filter(|item| item != id).collect();
        self.persistence.write(filtered);
        self.persistence.cleanup(id);
    }
}
