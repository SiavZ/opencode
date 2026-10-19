//! Purpose: compare real Rust variant generation with executed pure TypeScript observations.
//! Scope: deterministic fixture-based differential across SDKs, model families and release dates.
//! Uses:
//! rust/crates/
//! `-- provider-adapters/src/providers/variants.rs
//! External:
//! `-- serde_json::Value
//! Used by:
//! `-- none, test entrypoint
//! Invariants: fixture values come from unchanged TS functions via capture-provider-variants.ts.
//! Contract tests: this file.

use opencode_provider_adapters::providers::variants::generated;
use serde_json::Value;

#[test]
fn matches_actual_typescript_variant_observations() {
    let cases: Vec<Value> = serde_json::from_str(include_str!(
        "../../../contracts/phase-03/provider-variants.json"
    ))
    .unwrap();
    let failures: Vec<_> = cases
        .iter()
        .filter_map(|case| {
            let actual = Value::Object(generated(&case["model"], case.get("inventory")));
            (actual != case["expected"]).then(|| {
                format!(
                    "{} {} {} inventory={}\nactual={}\nexpected={}",
                    case["model"]["api"]["npm"],
                    case["model"]["id"],
                    case["model"]["release_date"],
                    case.get("inventory").unwrap_or(&Value::Null),
                    actual,
                    case["expected"]
                )
            })
        })
        .collect();
    assert!(
        failures.is_empty(),
        "{} / {} mismatches:\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n")
    );
}
