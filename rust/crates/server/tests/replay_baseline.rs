//! Purpose: replay every TypeScript baseline HTTP fixture (tests/fixtures/baseline, captured by
//! rust/contracts/phase-06/capture/capture.py) against the Rust full-host router and hold the
//! result to an explicit expected-failure ledger. Fixture-based, in-process (tower oneshot).
//! Scope: integration test over the public router constructors.
//! Uses:
//! rust/crates/
//! |-- server/src/app.rs
//! |-- server/src/middleware/auth.rs
//! |-- server/src/middleware/cors.rs
//! |-- server/src/services/mod.rs
//! |-- server/tests/support/mod.rs
//! |-- server/tests/support/replay/matcher.rs
//! |-- server/tests/support/replay/mod.rs
//! `-- server/tests/support/replay/run.rs
//! External:
//! |-- app::router
//! |-- axum::Router
//! |-- fixtures::baseline_dir
//! |-- fixtures::ledger
//! |-- fixtures::ledger_path
//! |-- fixtures::load
//! |-- fixtures::openapi_operations
//! |-- fixtures::repo_root
//! |-- run::replay
//! |-- serde_json::to_string_pretty
//! |-- tempfile::TempDir
//! |-- tempfile::tempdir
//! `-- tokio::test
//! Used by:
//! `-- none, entrypoint
//! Invariants: no silent skips. Every case either passes or is listed in
//! tests/fixtures/replay-ledger.json with its mismatch category; a ledgered case that now passes
//! or fails with another category fails the test (stale ledger). `REPLAY_BLESS=1` rewrites the
//! ledger and rust/contracts/phase-06/fixtures/coverage.md; without it coverage.md must be current.
//! The router is composed with `Services::default()` (nothing injected), so the replay measures the
//! HTTP layer that exists today: unported operations answer the typed 503 and are ledgered as
//! `not-ported`.
//! Contract tests: this file.

mod support;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use opencode_server::app::{self, ServerConfig};
use opencode_server::middleware::auth::AuthConfig;
use opencode_server::middleware::cors::CorsConfig;
use opencode_server::services::Services;
use support::replay::matcher::Bindings;
use support::replay::run::{self, Outcome, PASSWORD};
use support::replay::{self as fixtures, Case, Expected};

fn router(server: &str) -> axum::Router {
    let auth = AuthConfig {
        username: "opencode".to_string(),
        password: (server == "pass").then(|| PASSWORD.to_string()),
    };
    app::router(
        Services::default(),
        ServerConfig::full_host(auth, CorsConfig::default()),
    )
}

/// Temp project laid out like the capture project, bound to `{PROJ}` / `{TMP}`.
fn project() -> (tempfile::TempDir, Vec<(String, String)>) {
    let root = tempfile::tempdir().unwrap();
    let real = std::fs::canonicalize(root.path()).unwrap();
    let proj = real.join("proj");
    std::fs::create_dir_all(proj.join("src")).unwrap();
    std::fs::write(
        proj.join("README.md"),
        "# fixture project\n\nhello fixture\nmodified\n",
    )
    .unwrap();
    std::fs::write(
        proj.join("src/main.ts"),
        "export function hello() {\n  return 'fixture'\n}\n",
    )
    .unwrap();
    let literals = vec![
        ("{PROJ}".to_string(), proj.display().to_string()),
        ("{TMP}".to_string(), real.display().to_string()),
    ];
    (root, literals)
}

async fn replay_all() -> Vec<(Case, Outcome)> {
    let cases = fixtures::load();
    assert!(!cases.is_empty(), "no baseline fixtures found");
    let (_root, literals) = project();
    let routers: BTreeMap<&str, axum::Router> =
        [("nopass", router("nopass")), ("pass", router("pass"))]
            .into_iter()
            .collect();
    let mut bindings = Bindings {
        literals,
        ..Bindings::default()
    };
    let mut out = Vec::new();
    for case in cases {
        let router = routers
            .get(case.server.as_str())
            .unwrap_or_else(|| panic!("{}: unknown server variant {}", case.id, case.server));
        let outcome = run::replay(router, &case, &mut bindings).await;
        out.push((case, outcome));
    }
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn baseline_replay_matches_ledger() {
    let results = replay_all().await;
    let ledger = fixtures::ledger();
    let bless = std::env::var_os("REPLAY_BLESS").is_some();

    let mut unexpected = Vec::new();
    let mut stale = Vec::new();
    let mut by_category: BTreeMap<String, usize> = BTreeMap::new();
    let mut passed = 0;
    for (case, outcome) in &results {
        match (&outcome.category, ledger.get(&case.id)) {
            (None, None) => passed += 1,
            (None, Some(expected)) => {
                passed += 1;
                stale.push(format!(
                    "{} now passes (ledger: {})",
                    case.id, expected.category
                ));
            }
            (Some(category), Some(expected)) if *category == expected.category => {
                *by_category.entry(category.clone()).or_default() += 1;
            }
            (Some(category), expected) => {
                *by_category.entry(category.clone()).or_default() += 1;
                unexpected.push(format!(
                    "{} [{}] (ledger: {}): {}",
                    case.id,
                    category,
                    expected.map_or("none", |e| e.category.as_str()),
                    outcome.detail
                ));
            }
        }
    }
    let known: BTreeSet<&str> = results.iter().map(|(c, _)| c.id.as_str()).collect();
    for id in ledger.keys() {
        if !known.contains(id.as_str()) {
            stale.push(format!("{id} is ledgered but no fixture case has that id"));
        }
    }
    let failed: usize = by_category.values().sum();
    println!(
        "baseline replay: {} cases, {passed} passed, {failed} expected-failures {by_category:?}",
        results.len()
    );

    let coverage = coverage_markdown(&results);
    let coverage_path = fixtures::repo_root().join("rust/contracts/phase-06/fixtures/coverage.md");
    if bless {
        let new_ledger: BTreeMap<String, Expected> = results
            .iter()
            .filter_map(|(case, outcome)| {
                outcome.category.as_ref().map(|category| {
                    (
                        case.id.clone(),
                        Expected {
                            category: category.clone(),
                            detail: outcome.detail.chars().take(300).collect(),
                        },
                    )
                })
            })
            .collect();
        std::fs::write(
            fixtures::ledger_path(),
            serde_json::to_string_pretty(&new_ledger).unwrap() + "\n",
        )
        .unwrap();
        std::fs::write(&coverage_path, &coverage).unwrap();
        println!(
            "blessed {} ledger entries and coverage.md",
            new_ledger.len()
        );
        return;
    }
    let on_disk = std::fs::read_to_string(&coverage_path).unwrap_or_default();
    assert!(
        unexpected.is_empty() && stale.is_empty(),
        "baseline replay drifted from the ledger (rerun with REPLAY_BLESS=1 after review)\n\
         unexpected:\n  {}\nstale:\n  {}",
        unexpected.join("\n  "),
        stale.join("\n  ")
    );
    assert!(
        on_disk == coverage,
        "rust/contracts/phase-06/fixtures/coverage.md is stale; rerun with REPLAY_BLESS=1"
    );
}

/// Every operation in packages/sdk/openapi.json has at least one recorded case, and every case
/// names a known operation (or is a route-level negative case with `operationId: null`).
#[test]
fn fixtures_cover_every_openapi_operation() {
    let operations = fixtures::openapi_operations();
    let cases = fixtures::load();
    let recorded: BTreeSet<&str> = cases
        .iter()
        .filter_map(|c| c.operation_id.as_deref())
        .collect();
    let missing: Vec<&str> = operations
        .iter()
        .map(|(_, _, id)| id.as_str())
        .filter(|id| !recorded.contains(id))
        .collect();
    assert!(
        missing.is_empty(),
        "operations without a fixture: {missing:?}"
    );
    let known: BTreeSet<&str> = operations.iter().map(|(_, _, id)| id.as_str()).collect();
    let unknown: Vec<&str> = recorded
        .iter()
        .copied()
        .filter(|id| !known.contains(id))
        .collect();
    assert!(
        unknown.is_empty(),
        "fixtures name unknown operations: {unknown:?}"
    );
    let ids: BTreeSet<&str> = cases.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids.len(), cases.len(), "duplicate case ids");
}

/// Fixtures never contain machine-local paths or the capture password. `/Users/name` appears in
/// the baseline bash tool description, so the check targets this machine's real roots instead.
#[test]
fn fixtures_are_sanitized() {
    let repo = std::fs::canonicalize(fixtures::repo_root()).unwrap();
    let mut needles = vec![
        repo.display().to_string(),
        "/private/var/".to_string(),
        "/var/folders/".to_string(),
        PASSWORD.to_string(),
        "p06fx-".to_string(),
    ];
    if let Some(home) = std::env::var_os("HOME") {
        needles.push(std::path::PathBuf::from(home).display().to_string() + "/");
    }
    for entry in std::fs::read_dir(fixtures::baseline_dir()).unwrap() {
        let path = entry.unwrap().path();
        let text = std::fs::read_to_string(&path).unwrap();
        for needle in &needles {
            assert!(
                !text.contains(needle.as_str()),
                "{} contains {needle:?}",
                path.display()
            );
        }
    }
}

fn coverage_markdown(results: &[(Case, Outcome)]) -> String {
    let operations = fixtures::openapi_operations();
    let mut md = String::new();
    let total = results.len();
    let passed = results.iter().filter(|(_, o)| o.category.is_none()).count();
    let mut categories: BTreeMap<&str, usize> = BTreeMap::new();
    for (_, o) in results {
        if let Some(c) = &o.category {
            *categories.entry(c.as_str()).or_default() += 1;
        }
    }
    md.push_str("# Phase 06 baseline replay coverage\n\n");
    md.push_str(
        "Generated by `REPLAY_BLESS=1 cargo test -p opencode-server --test replay_baseline`. Do not edit.\n\
         Fixtures: `rust/crates/server/tests/fixtures/baseline/` (one file per route family). Ledger:\n\
         `rust/crates/server/tests/fixtures/replay-ledger.json`. Router: `app::router(Services::default(),\n\
         ServerConfig::full_host(..))`, so every service-backed operation is measured against an empty host.\n\n",
    );
    let _ = writeln!(
        md,
        "Cases: {total}. Passed: {passed}. Expected failures: {}.\n",
        total - passed
    );
    md.push_str("| category | cases |\n| --- | --- |\n");
    for (c, n) in &categories {
        let _ = writeln!(md, "| {c} | {n} |");
    }
    md.push_str("\n## Operations\n\n| method | route | operationId | fixture | cases | passed | expected failures |\n| --- | --- | --- | --- | --- | --- | --- |\n");
    for (method, path, op) in &operations {
        let mine: Vec<&(Case, Outcome)> = results
            .iter()
            .filter(|(c, _)| c.operation_id.as_deref() == Some(op.as_str()))
            .collect();
        let families: BTreeSet<&str> = mine.iter().map(|(c, _)| c.family.as_str()).collect();
        let ok = mine.iter().filter(|(_, o)| o.category.is_none()).count();
        let mut fails: BTreeMap<&str, usize> = BTreeMap::new();
        for (_, o) in &mine {
            if let Some(c) = &o.category {
                *fails.entry(c.as_str()).or_default() += 1;
            }
        }
        let fails: Vec<String> = fails.iter().map(|(c, n)| format!("{c} x{n}")).collect();
        let _ = writeln!(
            md,
            "| {method} | `{path}` | {op} | {} | {} | {ok} | {} |",
            families.into_iter().collect::<Vec<_>>().join(", "),
            mine.len(),
            if fails.is_empty() {
                "-".to_string()
            } else {
                fails.join(", ")
            }
        );
    }
    md.push_str("\n## Route-level and negative cases\n\n| case | family | status | result |\n| --- | --- | --- | --- |\n");
    for (case, outcome) in results.iter().filter(|(c, _)| {
        c.operation_id.is_none()
            || c.family.starts_with("negative")
            || c.family == "sse"
            || c.family == "docs"
    }) {
        let _ = writeln!(
            md,
            "| `{}` | {} | {} | {} |",
            case.id.replace('|', "\\|"),
            case.family,
            case.response
                .status
                .map_or("timeout".to_string(), |s| s.to_string()),
            outcome.category.as_deref().unwrap_or("pass")
        );
    }
    md
}
