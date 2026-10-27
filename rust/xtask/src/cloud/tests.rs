//! Purpose: contract tests for `xtask cloud`: inventory coverage, determinism, goldens, secret
//! redaction, stage policy and the Honeycomb calculated-field hash.
//! Scope: unit tests (fixture: inventory and goldens are checked-in files, hash values were
//! captured by running the baseline TS hash function under bun 1.3).
//! Uses:
//! rust/xtask/
//! |-- src/cloud/infra/mod.rs
//! |-- src/cloud/infra/monitoring.rs
//! |-- src/cloud/inventory.rs
//! |-- src/cloud/mod.rs
//! |-- src/cloud/model.rs
//! `-- src/cloud/terraform.rs
//! External:
//! |-- inventory::diff
//! |-- serde_json::Value
//! |-- serde_json::to_string
//! `-- terraform::emit
//! Used by:
//! rust/xtask/
//! `-- src/cloud/mod.rs
//! Invariants: tests never touch the network or cloud credentials.
//! Contract tests: this file

use serde_json::Value;

use crate::cloud::infra::{build, monitoring::field_hash};
use crate::cloud::model::{Mode, Stage};
use crate::cloud::{
    DIFF_STAGES, GOLDEN_STAGES, inventory, load_inventory, render, rust_root, terraform,
};

#[test]
fn inventory_matches_every_checked_stage() {
    let inventory = load_inventory().expect("inventory parses");
    for name in DIFF_STAGES {
        for sst_dev in [false, true] {
            let plan = build(Stage {
                name: name.into(),
                sst_dev,
            });
            let diff = inventory::diff(&inventory, &plan);
            assert!(diff.is_clean(), "stage {name} sst_dev={sst_dev}: {diff:#?}");
        }
    }
}

#[test]
fn diff_detects_missing_and_extra() {
    let inventory = load_inventory().expect("inventory parses");
    let mut plan = build(Stage::new("dev"));
    let removed = plan.resources.remove(0);
    plan.resources.push(crate::cloud::model::Resource::new(
        Mode::Managed,
        "sst.cloudflare.Bucket",
        "Rogue",
        "test",
    ));
    let diff = inventory::diff(&inventory, &plan);
    assert_eq!(
        diff.extra,
        vec!["Rogue sst.cloudflare.Bucket managed".to_string()]
    );
    assert_eq!(diff.missing.len(), 1);
    assert!(diff.missing[0].starts_with(&removed.name));
}

#[test]
fn output_is_deterministic() {
    for name in GOLDEN_STAGES {
        assert_eq!(render(Stage::new(name)), render(Stage::new(name)));
    }
}

#[test]
fn goldens_are_current() {
    for name in GOLDEN_STAGES {
        let path = rust_root().join(format!("contracts/phase-13/cloud-golden/{name}.tf.json"));
        let golden = std::fs::read_to_string(&path).expect("golden exists");
        assert_eq!(
            golden,
            render(Stage::new(name)),
            "stale golden {name}, run `cargo xtask cloud golden --write`"
        );
    }
}

#[test]
fn secrets_are_variables_without_values() {
    let doc = terraform::emit(&build(Stage {
        name: "dev".into(),
        sst_dev: true,
    }));
    let variables = doc["variable"].as_object().expect("variables");
    for (name, var) in variables {
        if var["sensitive"] == Value::Bool(true) {
            assert!(
                var.get("default").is_none(),
                "secret {name} must not carry a default"
            );
        }
    }
    let text = serde_json::to_string(&doc).expect("json");
    for needle in ["sk_live", "sk_test", "whsec_", "BEGIN RSA", "BEGIN PRIVATE"] {
        assert!(
            !text.contains(needle),
            "secret-looking value {needle} in output"
        );
    }
    assert_eq!(
        doc["provider"]["stripe"]["api_key"],
        "${var.STRIPE_SECRET_KEY}"
    );
}

#[test]
fn stage_domains_and_policy() {
    let prod = Stage::new("production");
    let dev = Stage::new("dev");
    let pr = Stage::new("pr-123");
    assert_eq!(
        (prod.domain(), prod.short_domain()),
        ("opencode.ai".into(), "opncd.ai".into())
    );
    assert_eq!(
        (dev.domain(), dev.short_domain()),
        ("dev.opencode.ai".into(), "dev.opncd.ai".into())
    );
    assert_eq!(pr.domain(), "pr-123.dev.opencode.ai");
    assert_eq!(pr.short_domain(), "pr-123.dev.opncd.ai");
    assert_eq!((prod.removal(), prod.protect()), ("retain", true));
    assert_eq!((pr.removal(), pr.protect()), ("remove", false));
    assert!(prod.deploy_aws() && dev.deploy_aws() && !pr.deploy_aws());
    assert_eq!(pr.aws_stage(), "dev");
    assert!(prod.monitoring() && Stage::new("vimtor").monitoring() && !dev.monitoring());
    assert_eq!(dev.aws_profile(false), Some("opencode-dev"));
    assert_eq!(prod.aws_profile(true), None);
}

#[test]
fn production_resources_prevent_destroy() {
    let prod = terraform::emit(&build(Stage::new("production")));
    assert_eq!(
        prod["resource"]["stripe_product"]["ZenLite"]["lifecycle"]["prevent_destroy"],
        true
    );
    let dev = terraform::emit(&build(Stage::new("dev")));
    assert!(
        dev["resource"]["stripe_product"]["ZenLite"]
            .get("lifecycle")
            .is_none()
    );
}

#[test]
fn durable_object_binding_rules() {
    let worker = |stage: &str| {
        build(Stage::new(stage))
            .get("Api")
            .map(|r| r.props["worker"].clone())
            .expect("Api")
    };
    assert!(worker("vimtor").get("extra_bindings").is_none());
    assert!(worker("adam").get("migrations").is_none());
    assert_eq!(worker("production")["migrations"]["new_tag"], "");
    assert_eq!(worker("thdxr")["migrations"]["old_tag"], "");
    assert_eq!(worker("dev")["migrations"]["new_tag"], "v1");
    assert_eq!(
        worker("dev")["extra_bindings"][0]["class_name"],
        "SyncServer"
    );
}

#[test]
fn console_stage_rules() {
    let get = |stage: &str, name: &str| {
        build(Stage::new(stage))
            .get(name)
            .map(|r| r.props.clone())
            .expect(name)
    };
    assert_eq!(
        get("dev", "ZEN_LITE_PRICE")["product"],
        "prod_U1tUscpmwtV2bG"
    );
    assert_eq!(
        get("production", "ZEN_LITE_PRICE")["product"],
        "${ref:ZenLite.id}"
    );
    assert_eq!(get("pr-123", "ConsoleMigration")["consoleUrl"], "");
    assert_eq!(
        get("dev", "ConsoleMigration")["inferenceUrl"],
        "https://dev.opencode.ai/inference"
    );
    assert_eq!(
        get("production", "StripeWebhookEndpoint")["enabled_events"]
            .as_array()
            .map(Vec::len),
        Some(26)
    );
    assert_eq!(get("dev", "R2Sql")["bucket"], "platform-dev-lake");
    assert_eq!(get("dev", "StatsSyncService")["scaling"]["max"], 1);
}

#[test]
fn monitoring_disabled_outside_production() {
    let vimtor = build(Stage::new("vimtor"));
    let trigger = vimtor.get("LowModelTpsGo").expect("trigger");
    assert_eq!(trigger.props["disabled"], true);
    assert_eq!(
        vimtor.get("DiscordAlerts").expect("recipient").props["name"],
        "Discord Alerts (vimtor)"
    );
    let prod = build(Stage::new("production"));
    assert_eq!(
        prod.get("LowModelTpsGo").expect("trigger").props["disabled"],
        false
    );
}

#[test]
fn calculated_field_hash_matches_baseline() {
    // Captured from infra/monitoring.ts:34-39 under bun.
    assert_eq!(
        field_hash("IF(AND(GTE($status, \"200\"), LT($status, \"400\")), 1, 0)"),
        "j7un9p"
    );
    assert_eq!(
        field_hash("IF(GT($llm.error.code, \"400\"), 1, 0)"),
        "43kpv9"
    );
    let plan = build(Stage::new("production"));
    let trigger = plan.get("IncreasedModelHttpErrorsGo").expect("trigger");
    assert_eq!(
        trigger.props["query_json"]["calculated_fields"][0]["name"],
        "is_failed_http_status_q3j3ra"
    );
}

#[test]
fn outputs_follow_sst_config() {
    let dev = terraform::emit(&build(Stage::new("dev")));
    let keys: Vec<&String> = dev["output"].as_object().expect("outputs").keys().collect();
    assert_eq!(keys, ["StatWorkerUrl", "StatsUrl", "AwsStage"]);
    assert_eq!(
        dev["output"]["StatWorkerUrl"]["value"],
        "${local.Stat.url}"
    );
    let pr = terraform::emit(&build(Stage::new("pr-123")));
    assert!(pr["output"].get("StatsUrl").is_none());
    assert_eq!(pr["output"]["AwsStage"]["value"], "dev");
}

#[test]
fn no_unresolved_references() {
    for name in DIFF_STAGES {
        let text = render(Stage {
            name: name.into(),
            sst_dev: true,
        });
        assert!(
            !text.contains("unresolved."),
            "stage {name} has an unresolved reference"
        );
        assert!(
            !text.contains("${ref:"),
            "stage {name} leaked a symbolic reference"
        );
        assert!(
            !text.contains("unmapped_kind"),
            "stage {name} has an unmapped kind"
        );
    }
}
