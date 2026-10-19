//! Purpose: port of packages/core/test/system-context/{registry,builtins}.test.ts.
//! Scope: integration tests, in-memory.
//! Uses:
//! rust/crates/
//! |-- context/src/builtins.rs
//! |-- context/src/registry.rs
//! `-- context/src/source/mod.rs
//! External:
//! |-- builtins::register
//! |-- builtins::system_today
//! `-- tokio::runtime::Builder::new_current_thread
//! Used by:
//! `-- none, entrypoint
//! Invariants: same cases as the baseline tests; scope close is modeled by dropping the guard.
//! Contract tests: this file

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use opencode_context::builtins::{self, LocationFacts};
use opencode_context::registry::{Entry, Registry};
use opencode_context::source::{
    ContextError, Generation, Key, Observation, ReconcileResult, Snapshot, Source, SystemContext,
    initialize, make, reconcile,
};

fn key(k: &str) -> Key {
    Key::parse(k).unwrap()
}

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(f)
}

fn entry(entry_key: &str, text: &'static str, source_key: &str) -> Entry {
    let ctx = make(Source::<String>::new(
        key(source_key),
        move || async move { Ok(Observation::Available(text.to_string())) },
        |v| v.clone(),
        |_, c| c.clone(),
    ));
    Entry::constant(key(entry_key), ctx)
}

fn init(registry: &Registry) -> Generation {
    block_on(async { initialize(&registry.load().await?).await }).unwrap()
}

#[test]
fn loads_empty_context_without_entries() {
    assert_eq!(
        init(&Registry::new()),
        Generation {
            baseline: String::new(),
            snapshot: Snapshot::new()
        }
    );
}

#[test]
fn loads_entries_in_stable_key_order() {
    let registry = Registry::new();
    let _a = registry
        .register(entry("test/second", "second", "test/second"))
        .unwrap();
    let _b = registry
        .register(entry("test/first", "first", "test/first"))
        .unwrap();
    assert_eq!(init(&registry).baseline, "first\n\nsecond");
}

#[test]
fn re_evaluates_producers_on_each_load() {
    let registry = Registry::new();
    let loads = Arc::new(AtomicUsize::new(0));
    let counter = loads.clone();
    let _g = registry
        .register(Entry::new(key("test/dynamic"), move || {
            counter.fetch_add(1, Ordering::SeqCst);
            async { Ok(SystemContext::empty()) }
        }))
        .unwrap();
    block_on(registry.load()).unwrap();
    block_on(registry.load()).unwrap();
    assert_eq!(loads.load(Ordering::SeqCst), 2);
}

#[test]
fn propagates_producer_failures() {
    let registry = Registry::new();
    let _g = registry
        .register(Entry::new(key("test/failure"), || async {
            Err(ContextError::Source("entry failed".into()))
        }))
        .unwrap();
    let err = block_on(registry.load()).unwrap_err();
    assert_eq!(err.to_string(), "entry failed");
}

#[test]
fn rejects_duplicate_source_keys_from_separate_entries() {
    let registry = Registry::new();
    let _a = registry
        .register(entry("test/first", "first", "test/duplicate"))
        .unwrap();
    let _b = registry
        .register(entry("test/second", "second", "test/duplicate"))
        .unwrap();
    let err = block_on(registry.load()).unwrap_err();
    assert!(matches!(err, ContextError::DuplicateKey { key: k } if k.as_str() == "test/duplicate"));
}

#[test]
fn rejects_duplicate_entry_keys() {
    let registry = Registry::new();
    let _a = registry
        .register(entry("test/duplicate", "first", "test/duplicate"))
        .unwrap();
    let err = registry
        .register(entry("test/duplicate", "second", "test/other"))
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("Duplicate system context entry key")
    );
}

#[test]
fn removes_entry_when_scope_closes() {
    let registry = Registry::new();
    let guard = registry
        .register(entry("test/scoped", "scoped", "test/scoped"))
        .unwrap();
    assert_eq!(init(&registry).baseline, "scoped");
    drop(guard);
    assert_eq!(
        init(&registry),
        Generation {
            baseline: String::new(),
            snapshot: Snapshot::new()
        }
    );
    // The key is free again after release.
    let _again = registry
        .register(entry("test/scoped", "again", "test/scoped"))
        .unwrap();
}

fn location() -> LocationFacts {
    LocationFacts {
        directory: "/repo/packages/core".into(),
        project_directory: "/repo".into(),
        git: true,
    }
}

fn env_block() -> String {
    [
        "Here is some useful information about the environment you are running in:",
        "<env>",
        "  Working directory: /repo/packages/core",
        "  Workspace root folder: /repo",
        "  Is directory a git repo: yes",
        &format!("  Platform: {}", builtins::platform()),
        "</env>",
    ]
    .join("\n")
}

#[test]
fn builtins_load_environment_and_local_date() {
    let registry = Registry::new();
    let _g = builtins::register(
        &registry,
        &location(),
        Arc::new(|| "Wed Jun 03 2026".to_string()),
    )
    .unwrap();
    assert_eq!(
        init(&registry).baseline,
        format!("{}\n\nToday's date: Wed Jun 03 2026", env_block())
    );
}

#[test]
fn builtins_reconcile_date_without_repeating_environment() {
    let registry = Registry::new();
    let today = Arc::new(Mutex::new("Wed Jun 03 2026".to_string()));
    let clock = today.clone();
    let _g = builtins::register(
        &registry,
        &location(),
        Arc::new(move || clock.lock().unwrap().clone()),
    )
    .unwrap();
    let initialized = init(&registry);

    // Same calendar day: unchanged.
    let same = block_on(async { reconcile(&registry.load().await?, &initialized.snapshot).await })
        .unwrap();
    assert_eq!(same, ReconcileResult::Unchanged);

    *today.lock().unwrap() = "Thu Jun 04 2026".into();
    let next = block_on(async { reconcile(&registry.load().await?, &initialized.snapshot).await })
        .unwrap();
    let ReconcileResult::Updated { text, .. } = next else {
        panic!("{next:?}")
    };
    assert_eq!(text, "Today's date is now: Thu Jun 04 2026");
}

#[test]
fn builtins_compose_with_domain_entries_after_builtins() {
    // Instruction context lives with its domain; it registers under a key sorting after core/builtins.
    let registry = Registry::new();
    let _g = builtins::register(
        &registry,
        &location(),
        Arc::new(|| "Wed Jun 03 2026".to_string()),
    )
    .unwrap();
    let _i = registry
        .register(entry(
            "core/instructions",
            "Instructions from: /repo/AGENTS.md\nBe precise.",
            "core/instructions",
        ))
        .unwrap();
    assert_eq!(
        init(&registry).baseline,
        format!(
            "{}\n\nToday's date: Wed Jun 03 2026\n\nInstructions from: /repo/AGENTS.md\nBe precise.",
            env_block()
        )
    );
}

#[test]
fn system_today_renders_to_date_string_shape() {
    let s = (builtins::system_today())();
    let parts: Vec<&str> = s.split(' ').collect();
    assert_eq!(parts.len(), 4, "{s}");
    assert_eq!(parts[0].len(), 3);
    assert_eq!(parts[1].len(), 3);
    assert_eq!(parts[2].len(), 2);
    assert_eq!(parts[3].len(), 4);
}
