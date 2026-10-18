//! Purpose: the registry emits `booting location services` with directory and workspaceID
//! (packages/core/src/location-services.ts:97-101). Separate binary so no other test can race the
//! tracing callsite interest cache.
//! Scope: live test of the real registry with a capturing subscriber.
//! Uses:
//! rust/crates/
//! `-- locations/src/lib.rs
//! External:
//! |-- tokio
//! `-- tracing
//! Used by:
//! `-- none, entrypoint
//! Invariants: exactly one boot line per successful build.
//! Contract tests: locations/tests/registry_log.rs

use std::sync::{Arc, Mutex};

use opencode_locations::ports::location_ref;
use opencode_locations::{
    BuildContext, LocationError, LocationRegistry, LocationServices, RegistryOptions,
};
use opencode_schema::ids::WorkspaceID;

struct Bundle;
impl LocationServices for Bundle {}

#[tokio::test(start_paused = true)]
async fn boot_log_names_directory_and_workspace() {
    let lines = Arc::new(Mutex::new(Vec::<String>::new()));
    let _guard = tracing::subscriber::set_default(Capture(lines.clone()));
    let registry: LocationRegistry<Bundle, ()> = LocationRegistry::new(
        Arc::new(()),
        |_: BuildContext<()>| async { Ok::<_, LocationError>(Bundle) },
        RegistryOptions::default(),
    );
    let workspace = WorkspaceID::new("wrk_9").unwrap();
    let _lease = registry
        .get(&location_ref("/repo", Some(workspace)))
        .await
        .unwrap();
    let _again = registry
        .get(&location_ref(
            "/repo",
            Some(WorkspaceID::new("wrk_9").unwrap()),
        ))
        .await
        .unwrap();
    let _local = registry.get(&location_ref("/other", None)).await.unwrap();
    let lines = lines.lock().unwrap();
    let boots: Vec<&String> = lines
        .iter()
        .filter(|l| l.contains("booting location services"))
        .collect();
    assert_eq!(boots.len(), 2, "{lines:?}");
    assert!(
        boots[0].contains("directory=\"/repo\"") && boots[0].contains("workspaceID=\"wrk_9\""),
        "{boots:?}"
    );
    assert!(
        boots[1].contains("directory=\"/other\"") && !boots[1].contains("workspaceID"),
        "{boots:?}"
    );
}

/// Minimal subscriber recording event fields.
struct Capture(Arc<Mutex<Vec<String>>>);

impl tracing::Subscriber for Capture {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        struct Visitor(String);
        impl tracing::field::Visit for Visitor {
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                self.0.push_str(&format!(" {}={:?}", field.name(), value));
            }
        }
        let mut visitor = Visitor(String::new());
        event.record(&mut visitor);
        self.0.lock().unwrap().push(visitor.0);
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}
