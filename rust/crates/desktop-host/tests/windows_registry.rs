//! Purpose: contract tests for window registry focus, close and quit semantics.
//! Scope: tests the exact behavior ported from window-registry.test.ts.
//! Uses:
//! rust/crates/
//! `-- desktop-host/src/windows/registry.rs
//! External:
//! `-- serde_json::Value
//! Used by:
//! `-- none, entrypoint
//! Invariants: tests cover all registry edge cases exactly as TypeScript baseline.
//! Contract tests: this file

use opencode_desktop_host::windows::registry::{WindowPersistence, WindowRegistry};
use serde_json::Value;
use std::cell::RefCell;
use std::rc::Rc;

struct MockPersistence {
    stored: Rc<RefCell<Value>>,
    cleaned: Rc<RefCell<Vec<String>>>,
}

impl WindowPersistence for MockPersistence {
    fn read(&self) -> Value {
        self.stored.borrow().clone()
    }

    fn write(&self, ids: Vec<String>) {
        *self.stored.borrow_mut() = Value::Array(ids.into_iter().map(Value::String).collect());
    }

    fn cleanup(&self, id: &str) {
        self.cleaned.borrow_mut().push(id.to_string());
    }
}

#[allow(clippy::type_complexity)]
fn setup(
    initial: Value,
) -> (
    WindowRegistry<String>,
    Rc<RefCell<Value>>,
    Rc<RefCell<Vec<String>>>,
) {
    let stored = Rc::new(RefCell::new(initial));
    let cleaned = Rc::new(RefCell::new(Vec::new()));
    let persistence = MockPersistence {
        stored: stored.clone(),
        cleaned: cleaned.clone(),
    };
    let registry = WindowRegistry::new(persistence);
    (registry, stored, cleaned)
}

#[test]
fn restores_persisted_ids_and_ignores_malformed_entries() {
    let (registry, _, _) = setup(Value::Array(vec![
        Value::String("a".to_string()),
        Value::String("".to_string()),
        Value::Number(42.into()),
        Value::String("b".to_string()),
    ]));
    assert_eq!(registry.persisted(), vec!["a", "b"]);

    let (registry, _, _) = setup(Value::String("junk".to_string()));
    assert_eq!(registry.persisted(), Vec::<String>::new());

    let (registry, _, _) = setup(Value::Null);
    assert_eq!(registry.persisted(), Vec::<String>::new());
}

#[test]
fn registers_windows_and_persists_each_id_once() {
    let (mut registry, stored, _) = setup(Value::Array(vec![]));
    registry.register("a".to_string(), "a".to_string());
    registry.register("a".to_string(), "a".to_string());
    registry.register("b".to_string(), "b".to_string());
    assert_eq!(
        *stored.borrow(),
        Value::Array(vec![
            Value::String("a".to_string()),
            Value::String("b".to_string()),
        ])
    );
}

#[test]
fn forgets_a_deliberately_closed_window_while_others_remain_open() {
    let (mut registry, stored, cleaned) = setup(Value::Array(vec![]));
    registry.register("a".to_string(), "a".to_string());
    registry.register("b".to_string(), "b".to_string());
    registry.closed("a");
    assert_eq!(
        *stored.borrow(),
        Value::Array(vec![Value::String("b".to_string())])
    );
    assert_eq!(*cleaned.borrow(), vec!["a"]);
}

#[test]
fn keeps_the_id_when_the_last_window_closes_so_relaunch_restores_it() {
    let (mut registry, stored, cleaned) = setup(Value::Array(vec![]));
    registry.register("a".to_string(), "a".to_string());
    registry.closed("a");
    assert_eq!(
        *stored.borrow(),
        Value::Array(vec![Value::String("a".to_string())])
    );
    assert_eq!(*cleaned.borrow(), Vec::<String>::new());

    let stored_clone = stored.clone();
    let persistence = MockPersistence {
        stored: stored_clone,
        cleaned: Rc::new(RefCell::new(Vec::new())),
    };
    let restarted = WindowRegistry::<String>::new(persistence);
    assert_eq!(restarted.persisted(), vec!["a"]);
}

#[test]
fn keeps_every_id_when_windows_close_during_quit() {
    let (mut registry, stored, cleaned) = setup(Value::Array(vec![]));
    registry.register("a".to_string(), "a".to_string());
    registry.register("b".to_string(), "b".to_string());
    registry.set_quitting(true);
    registry.closed("a");
    registry.closed("b");
    assert_eq!(
        *stored.borrow(),
        Value::Array(vec![
            Value::String("a".to_string()),
            Value::String("b".to_string()),
        ])
    );
    assert_eq!(*cleaned.borrow(), Vec::<String>::new());
}

#[test]
fn tracks_the_last_focused_window_and_falls_back_on_close() {
    let (mut registry, _, _) = setup(Value::Array(vec![]));
    registry.register("a".to_string(), "a".to_string());
    registry.register("b".to_string(), "b".to_string());
    registry.focused("a");
    assert_eq!(registry.last_focused(), Some(&"a".to_string()));
    registry.closed("a");
    assert_eq!(registry.last_focused(), Some(&"b".to_string()));
    registry.closed("b");
    assert_eq!(registry.last_focused(), None);
}

#[test]
fn resumes_forgetting_closed_windows_after_the_quit_flag_resets() {
    let (mut registry, stored, cleaned) = setup(Value::Array(vec![]));
    registry.register("a".to_string(), "a".to_string());
    registry.register("b".to_string(), "b".to_string());
    registry.set_quitting(true);
    registry.set_quitting(false);
    registry.closed("a");
    assert_eq!(
        *stored.borrow(),
        Value::Array(vec![Value::String("b".to_string())])
    );
    assert_eq!(*cleaned.borrow(), vec!["a"]);
}
