//! Purpose: port of packages/core/test/system-context/index.test.ts against the Rust algebra.
//! Scope: integration tests, pure (no storage).
//! Uses:
//! rust/crates/
//! `-- context/src/source/mod.rs
//! External:
//! |-- futures::FutureExt
//! |-- futures::future::pending
//! |-- serde_json::Value
//! |-- serde_json::from_value
//! |-- serde_json::json
//! `-- tokio::runtime::Builder::new_current_thread
//! Used by:
//! `-- none, entrypoint
//! Invariants: same cases and expected values as the baseline test file.
//! Contract tests: this file

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use opencode_context::source::{
    ContextError, Key, Observation, ReconcileResult, ReplacementResult, Snapshot, Source,
    SourceSnapshot, SystemContext, combine, initialize, make, reconcile, replace,
};
use serde_json::json;

fn key(k: &str) -> Key {
    Key::parse(k).unwrap()
}

struct Spec {
    key: &'static str,
    value: Option<&'static str>,
    baseline: Option<fn(&String) -> String>,
    update: Option<fn(&String, &String) -> String>,
    removed: Option<fn(&String) -> String>,
}

fn spec(key: &'static str, value: Option<&'static str>) -> Spec {
    Spec {
        key,
        value,
        baseline: None,
        update: None,
        removed: None,
    }
}

fn string_context(s: Spec) -> SystemContext {
    let value = s.value.map(String::from);
    let baseline = s.baseline.unwrap_or(|v| v.clone());
    let update = s.update.unwrap_or(|_, c| c.clone());
    let source = Source::<String>::new(
        key(s.key),
        move || {
            let value = value.clone();
            async move { Ok(value.map_or(Observation::Unavailable, Observation::Available)) }
        },
        baseline,
        update,
    );
    make(match s.removed {
        Some(r) => source.with_removed(r),
        None => source,
    })
}

fn snap(v: serde_json::Value) -> Snapshot {
    serde_json::from_value(v).unwrap()
}

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(f)
}

#[test]
fn stores_canonical_json_encoding() {
    #[derive(serde::Serialize, serde::Deserialize, PartialEq)]
    struct Date(String);
    let ctx = make(
        Source::<Date>::new(
            key("core/date"),
            || async {
                Ok(Observation::Available(Date(
                    "2026-06-03T12:00:00.000Z".into(),
                )))
            },
            |d| d.0.clone(),
            |_, d| d.0.clone(),
        )
        .with_removed(|_| "Date removed".into()),
    );
    let g = block_on(initialize(&ctx)).unwrap();
    assert_eq!(
        g.snapshot[&key("core/date")].value,
        json!("2026-06-03T12:00:00.000Z")
    );
}

#[test]
fn loads_once_and_initializes_baseline_with_snapshot() {
    let loads = Arc::new(AtomicUsize::new(0));
    let counter = loads.clone();
    let date = make(
        Source::<String>::new(
            key("core/date"),
            move || {
                counter.fetch_add(1, Ordering::SeqCst);
                async { Ok(Observation::Available("2026-06-03".to_string())) }
            },
            |d| format!("Today's date is {d}."),
            |p, c| format!("The date changed from {p} to {c}."),
        )
        .with_removed(|_| "The date was removed.".into()),
    );
    let mut loc = spec("core/location", Some("/repo"));
    loc.baseline = Some(|v| format!("Directory: {v}"));
    let ctx = combine([date, string_context(loc)]).unwrap();
    let g = block_on(initialize(&ctx)).unwrap();
    assert_eq!(
        g.baseline,
        "Today's date is 2026-06-03.\n\nDirectory: /repo"
    );
    assert_eq!(
        serde_json::to_value(&g.snapshot).unwrap(),
        json!({
            "core/date": {"value": "2026-06-03", "removed": "The date was removed."},
            "core/location": {"value": "/repo"},
        })
    );
    assert_eq!(loads.load(Ordering::SeqCst), 1);
}

#[test]
fn renders_updates_only_after_value_changes() {
    let previous = snap(json!({
        "core/date": {"value": "2026-06-03", "removed": "The date was removed."},
        "core/location": {"value": "/repo", "removed": "Removed: /repo"},
    }));
    let mut d = spec("core/date", Some("2026-06-04"));
    d.update = Some(|b, c| format!("The date changed from {b} to {c}."));
    d.removed = Some(|_| "The date was removed.".into());
    let changed = combine([
        string_context(d),
        string_context(spec("core/location", Some("/repo"))),
    ])
    .unwrap();
    let ReconcileResult::Updated { text, snapshot } =
        block_on(reconcile(&changed, &previous)).unwrap()
    else {
        panic!("expected Updated")
    };
    assert_eq!(text, "The date changed from 2026-06-03 to 2026-06-04.");
    assert_eq!(
        serde_json::to_value(&snapshot).unwrap(),
        json!({
            "core/date": {"value": "2026-06-04", "removed": "The date was removed."},
            "core/location": {"value": "/repo", "removed": "Removed: /repo"},
        })
    );
    let mut same = spec("core/date", Some("2026-06-03"));
    same.removed = Some(|_| "The date was removed.".into());
    let unchanged = combine([
        string_context(same),
        string_context(spec("core/location", Some("/repo"))),
    ])
    .unwrap();
    assert_eq!(
        block_on(reconcile(&unchanged, &previous)).unwrap(),
        ReconcileResult::Unchanged
    );
}

#[test]
fn uses_baseline_for_new_source() {
    let mut s = spec("core/skills", Some("effect"));
    s.baseline = Some(|v| format!("Available skill: {v}"));
    let result = block_on(reconcile(&string_context(s), &Snapshot::new())).unwrap();
    assert_eq!(
        result,
        ReconcileResult::Updated {
            text: "Available skill: effect".into(),
            snapshot: snap(json!({"core/skills": {"value": "effect"}})),
        }
    );
}

#[test]
fn retains_snapshot_while_unavailable() {
    let previous =
        snap(json!({"core/remote": {"value": "instructions", "removed": "Instructions removed"}}));
    let ctx = string_context(spec("core/remote", None));
    assert_eq!(
        block_on(reconcile(&ctx, &previous)).unwrap(),
        ReconcileResult::Unchanged
    );
    assert_eq!(
        block_on(replace(&ctx, &previous)).unwrap(),
        ReplacementResult::ReplacementBlocked
    );
    assert!(matches!(
        block_on(replace(&ctx, &Snapshot::new())).unwrap(),
        ReplacementResult::ReplacementReady(_)
    ));
}

#[test]
fn blocks_initialization_while_unavailable() {
    let err = block_on(initialize(&string_context(spec("core/remote", None)))).unwrap_err();
    let ContextError::InitializationBlocked { keys } = &err else {
        panic!("{err}")
    };
    assert_eq!(keys, &vec![key("core/remote")]);
    assert_eq!(
        err.to_string(),
        "System context initialization blocked by unavailable sources: core/remote"
    );
}

#[test]
fn emits_stored_removal_message() {
    let previous = snap(
        json!({"core/instructions": {"value": "contents", "removed": "Instructions removed; stop applying them."}}),
    );
    assert_eq!(
        block_on(reconcile(&SystemContext::empty(), &previous)).unwrap(),
        ReconcileResult::Updated {
            text: "Instructions removed; stop applying them.".into(),
            snapshot: Snapshot::new()
        }
    );
}

#[test]
fn replacement_when_source_without_removal_disappears() {
    let previous = snap(json!({"core/date": {"value": "2026-06-04"}}));
    assert!(matches!(
        block_on(reconcile(&SystemContext::empty(), &previous)).unwrap(),
        ReconcileResult::ReplacementReady(_)
    ));
}

#[test]
fn renders_multiple_removals_in_key_order() {
    let previous = snap(json!({
        "core/z": {"value": "z", "removed": "Removed z"},
        "core/a": {"value": "a", "removed": "Removed a"},
    }));
    let ReconcileResult::Updated { text, .. } =
        block_on(reconcile(&SystemContext::empty(), &previous)).unwrap()
    else {
        panic!()
    };
    assert_eq!(text, "Removed a\n\nRemoved z");
}

#[test]
fn rejects_empty_rendering() {
    let mut s = spec("core/empty", Some("value"));
    s.baseline = Some(|_| String::new());
    let err = block_on(initialize(&string_context(s))).unwrap_err();
    assert!(
        err.to_string().contains("rendered an empty baseline"),
        "{err}"
    );
    let mut s = spec("core/empty", Some("value"));
    s.update = Some(|_, _| String::new());
    let err = block_on(reconcile(
        &string_context(s),
        &snap(json!({"core/empty": {"value": "old"}})),
    ))
    .unwrap_err();
    assert!(matches!(
        err,
        ContextError::EmptyRendering { kind: "update", .. }
    ));
    let mut s = spec("core/empty", Some("value"));
    s.removed = Some(|_| String::new());
    let err = block_on(initialize(&string_context(s))).unwrap_err();
    assert!(matches!(
        err,
        ContextError::EmptyRendering {
            kind: "removal",
            ..
        }
    ));
}

#[test]
fn replacement_when_stored_value_no_longer_decodes() {
    let previous = snap(json!({"core/date": {"value": 42, "removed": "Date removed"}}));
    let ctx = string_context(spec("core/date", Some("2026-06-04")));
    assert!(matches!(
        block_on(reconcile(&ctx, &previous)).unwrap(),
        ReconcileResult::ReplacementReady(_)
    ));
}

#[test]
fn replaces_from_one_coherent_observation() {
    let loads = Arc::new(AtomicUsize::new(0));
    let counter = loads.clone();
    let ctx = make(Source::<String>::new(
        key("core/date"),
        move || {
            counter.fetch_add(1, Ordering::SeqCst);
            async { Ok(Observation::Available("2026-06-04".to_string())) }
        },
        |v| v.clone(),
        |_, c| c.clone(),
    ));
    let ReconcileResult::ReplacementReady(g) =
        block_on(reconcile(&ctx, &snap(json!({"core/date": {"value": 42}})))).unwrap()
    else {
        panic!()
    };
    assert_eq!(g.baseline, "2026-06-04");
    assert_eq!(loads.load(Ordering::SeqCst), 1);
}

#[test]
fn does_not_render_discarded_updates_while_replacing() {
    static UPDATES: AtomicUsize = AtomicUsize::new(0);
    let mut d = spec("core/date", Some("2026-06-04"));
    d.update = Some(|_, _| {
        UPDATES.fetch_add(1, Ordering::SeqCst);
        "updated".into()
    });
    let ctx = combine([
        string_context(d),
        string_context(spec("core/location", Some("/repo"))),
    ])
    .unwrap();
    let previous =
        snap(json!({"core/date": {"value": "2026-06-03"}, "core/location": {"value": 42}}));
    assert!(matches!(
        block_on(reconcile(&ctx, &previous)).unwrap(),
        ReconcileResult::ReplacementReady(_)
    ));
    assert_eq!(UPDATES.load(Ordering::SeqCst), 0);
}

#[test]
fn blocks_incompatible_replacement_while_admitted_source_unavailable() {
    let previous = snap(json!({
        "core/date": {"value": 42, "removed": "Date removed"},
        "core/remote": {"value": "instructions", "removed": "Instructions removed"},
    }));
    let ctx = combine([
        string_context(spec("core/date", Some("2026-06-04"))),
        string_context(spec("core/remote", None)),
    ])
    .unwrap();
    assert_eq!(
        block_on(reconcile(&ctx, &previous)).unwrap(),
        ReconcileResult::ReplacementBlocked
    );
    assert_eq!(
        block_on(replace(&ctx, &previous)).unwrap(),
        ReplacementResult::ReplacementBlocked
    );
}

#[test]
fn rejects_duplicate_source_keys() {
    let err = combine([
        string_context(spec("core/date", Some("one"))),
        string_context(spec("core/date", Some("two"))),
    ])
    .unwrap_err();
    assert!(matches!(&err, ContextError::DuplicateKey { key: k } if k.as_str() == "core/date"));
    assert_eq!(err.to_string(), "Duplicate system context key: core/date");
}

#[test]
fn combines_in_order() {
    let ctx = combine([
        string_context(spec("core/date", Some("date"))),
        string_context(spec("core/location", Some("location"))),
    ])
    .unwrap();
    assert_eq!(
        block_on(initialize(&ctx)).unwrap().baseline,
        "date\n\nlocation"
    );
}

#[test]
fn combines_distinct_value_types_and_compares_structured_snapshots() {
    #[derive(serde::Serialize, serde::Deserialize, PartialEq)]
    struct Facts {
        count: u32,
        names: Vec<String>,
    }
    let structured = make(Source::new(
        key("core/facts"),
        || async {
            Ok(Observation::Available(Facts {
                count: 2,
                names: vec!["a".into()],
            }))
        },
        |facts: &Facts| format!("Count: {}", facts.count),
        |previous, current| format!("Count: {} -> {}", previous.count, current.count),
    ));
    let ctx = combine([structured, string_context(spec("core/date", Some("today")))]).unwrap();
    let generation = block_on(initialize(&ctx)).unwrap();
    assert_eq!(generation.baseline, "Count: 2\n\ntoday");
    assert_eq!(
        generation.snapshot[&key("core/facts")].value,
        json!({"count": 2, "names": ["a"]})
    );
    assert_eq!(
        block_on(reconcile(&ctx, &generation.snapshot)).unwrap(),
        ReconcileResult::Unchanged
    );
    let mut previous = generation.snapshot;
    previous[&key("core/facts")].value["count"] = json!(1);
    let ReconcileResult::Updated { text, .. } = block_on(reconcile(&ctx, &previous)).unwrap()
    else {
        panic!()
    };
    assert_eq!(text, "Count: 1 -> 2");
}

#[test]
fn requires_namespaced_keys() {
    assert_eq!(Key::parse("core/date").unwrap().as_str(), "core/date");
    assert!(Key::parse("date").is_err());
    assert!(Key::parse("Core/date").is_err());
    assert!(Key::parse("core/").is_err());
    assert!(Key::parse("a.b_c-d/e/f.g").is_ok());
}

#[test]
fn requires_namespaced_durable_snapshot_keys() {
    let ok: Snapshot = serde_json::from_value(json!({"core/date": {"value": "date"}})).unwrap();
    assert_eq!(
        ok.keys().map(Key::as_str).collect::<Vec<_>>(),
        vec!["core/date"]
    );
    assert!(serde_json::from_value::<Snapshot>(json!({"date": {"value": "date"}})).is_err());
    assert!(
        serde_json::from_value::<Snapshot>(json!({"core/date": {"value": "date", "removed": ""}}))
            .is_err()
    );
    assert!(
        serde_json::from_value::<Snapshot>(
            json!({"core/date": {"value": "date", "removed": null}})
        )
        .is_err()
    );
    let _ = SourceSnapshot {
        value: json!(1),
        removed: None,
    };
}

#[test]
fn source_failure_does_not_wait_for_unavailable_observation() {
    use futures::FutureExt;
    let failing = make(Source::<String>::new(
        key("core/failure"),
        || async { Err(ContextError::Source("source failed".into())) },
        Clone::clone,
        |_, value| value.clone(),
    ));
    let waiting = make(Source::<String>::new(
        key("core/waiting"),
        futures::future::pending::<Result<Observation<String>, ContextError>>,
        Clone::clone,
        |_, value| value.clone(),
    ));
    let ctx = combine([failing, waiting]).unwrap();
    let result = block_on(async { initialize(&ctx).now_or_never() });
    assert!(matches!(result, Some(Err(ContextError::Source(_)))));
}

#[test]
fn updates_preserve_unavailable_snapshot_and_ignore_unadmitted_unavailable_source() {
    let previous = snap(json!({"core/remote": {"value": "admitted", "removed": "Remote removed"}}));
    let ctx = combine([
        string_context(spec("core/remote", None)),
        string_context(spec("core/new-remote", None)),
        string_context(spec("core/date", Some("today"))),
    ])
    .unwrap();
    let ReconcileResult::Updated { text, snapshot } = block_on(reconcile(&ctx, &previous)).unwrap()
    else {
        panic!("expected update")
    };
    assert_eq!(text, "today");
    assert_eq!(snapshot[&key("core/remote")], previous[&key("core/remote")]);
    assert!(!snapshot.contains_key(&key("core/new-remote")));
}
