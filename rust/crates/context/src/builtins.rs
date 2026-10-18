//! Purpose: built-in system context sources (`packages/core/src/system-context/builtins.ts`):
//! `core/environment` (working directory, workspace root, git flag, platform) and `core/date`
//! (host-local calendar date), registered under the `core/builtins` entry.
//! Scope: Location-scoped producer. Instruction, skill and reference sources live with their domains.
//! Uses:
//! rust/crates/
//! |-- context/src/registry.rs
//! `-- context/src/source/mod.rs
//! External:
//! `-- jiff::Zoned
//! Used by:
//! rust/crates/
//! |-- context/src/lib.rs
//! `-- context/tests/registry.rs
//! Invariants: environment text is computed once per registration (baseline computes it once per
//! Location layer), the date is re-read on every load so a new local day produces one update.
//! Contract tests: context/tests/registry.rs

use std::sync::Arc;

use crate::registry::{Entry, Registration, Registry};
use crate::source::{ContextError, Key, Observation, Source, SystemContext, combine, make};

/// Location facts the environment source renders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocationFacts {
    pub directory: String,
    pub project_directory: String,
    pub git: bool,
}

/// Host-local clock rendering JavaScript `Date.prototype.toDateString` (`Wed Jun 03 2026`).
pub type Today = Arc<dyn Fn() -> String + Send + Sync>;

/// `process.platform` spelling of the current OS.
pub fn platform() -> &'static str {
    match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    }
}

/// System clock in the host time zone.
pub fn system_today() -> Today {
    Arc::new(|| jiff::Zoned::now().strftime("%a %b %d %Y").to_string())
}

/// The `<env>` block, byte-identical to the baseline.
pub fn environment(location: &LocationFacts) -> String {
    [
        "<env>".to_string(),
        format!("  Working directory: {}", location.directory),
        format!("  Workspace root folder: {}", location.project_directory),
        format!(
            "  Is directory a git repo: {}",
            if location.git { "yes" } else { "no" }
        ),
        format!("  Platform: {}", platform()),
        "</env>".to_string(),
    ]
    .join("\n")
}

/// The combined built-in context for one Location.
pub fn context(location: &LocationFacts, today: Today) -> Result<SystemContext, ContextError> {
    let env = environment(location);
    combine([
        make(Source::<String>::new(
            key("core/environment"),
            move || {
                let env = env.clone();
                async move { Ok(Observation::Available(env)) }
            },
            |env| {
                [
                    "Here is some useful information about the environment you are running in:",
                    env,
                ]
                .join("\n")
            },
            |_, env| ["The environment you are running in is now:", env].join("\n"),
        )),
        make(Source::<String>::new(
            key("core/date"),
            move || {
                let date = today();
                async move { Ok(Observation::Available(date)) }
            },
            |date| format!("Today's date: {date}"),
            |_, date| format!("Today's date is now: {date}"),
        )),
    ])
}

/// Register the built-ins under `core/builtins`. Keep the guard alive for the Location lifetime.
pub fn register(
    registry: &Registry,
    location: &LocationFacts,
    today: Today,
) -> Result<Registration, ContextError> {
    registry.register(Entry::constant(
        key("core/builtins"),
        context(location, today)?,
    ))
}

fn key(value: &str) -> Key {
    Key::parse(value).expect("builtin keys are valid")
}
