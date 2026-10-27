//! Purpose: enforce native console topology, environment, secret, volume and routing contracts.
//! Scope: pure plan tests without credentials, builds or deployment side effects.
//! Uses:
//! rust/xtask/
//! |-- src/cloud/infra/mod.rs
//! `-- src/cloud/model.rs
//! External:
//! `-- serde_json::Value
//! Used by:
//! rust/xtask/
//! `-- src/cloud/infra/console/native.rs
//! Invariants: no Auth/Stat/Log TypeScript fallback, no Cloudflare loopback deployment fiction.
//! Contract tests: this file.

use crate::cloud::infra::build;
use crate::cloud::model::{Mode, Stage};

#[test]
fn native_console_services_have_truthful_non_deployable_topology() {
    for stage in ["production", "dev", "pr-123"] {
        let plan = build(Stage::new(stage));
        for name in ["AuthApi", "AuthStorage", "Console", "LogProcessor", "Stat"] {
            let resource = plan.get(name).unwrap();
            assert_eq!(resource.mode, Mode::Linkable);
            assert_eq!(resource.kind, "native.ConsoleService");
            assert_eq!(resource.props["deployable"], false);
            assert_eq!(resource.props["status"], "deployment_backend_required");
            let text = resource.props.to_string();
            assert!(!text.contains("packages/console/function/src/"));
            assert!(!text.contains("cloudflare-module"));
            assert!(!text.contains("tail_consumers"));
        }
        let issuer = &plan.get("AuthApi").unwrap().props["service"];
        assert_eq!(
            issuer["environment"]["OPENCODE_HOSTED_SERVICE"],
            "console-issuer"
        );
        assert_eq!(issuer["volumes"][0]["access"], "single-writer");
        assert_eq!(issuer["secret_files"].as_array().unwrap().len(), 2);
        assert_eq!(
            issuer["environment"]["OPENCODE_ISSUER_STORE_PATH"],
            "/var/lib/opencode/issuer/oauth.sqlite"
        );
        let console = &plan.get("Console").unwrap().props["service"];
        assert_eq!(console["processes"]["ssr"]["preset"], "node-server");
        assert_eq!(
            console["processes"]["ssr"]["shared_network_namespace"],
            console["processes"]["axum"]["shared_network_namespace"]
        );
        let env = &console["processes"]["axum"]["environment"];
        assert_eq!(
            env["OPENCODE_CONSOLE_AUTH_STATE_REDIS_URL"],
            "redis://127.0.0.1:6379/0"
        );
        assert_eq!(
            env["OPENCODE_CONSOLE_TAIL_URL"],
            "http://127.0.0.1:8790/tail"
        );
        assert_eq!(
            env["OPENCODE_ZEN_BLACK_PRICE_PLAN200"],
            "${ref:ZEN_BLACK_PRICE.plan200}"
        );
        for index in 1..=30 {
            assert!(env.get(format!("ZEN_MODELS{index}")).is_some());
        }
        assert_eq!(
            plan.get("Stat").unwrap().props["service"]["environment"]["OPENCODE_HOSTED_SERVICE"],
            "console-stat"
        );
        let log = &plan.get("LogProcessor").unwrap().props["service"];
        assert_eq!(log["ingress"]["path"], "/tail");
        assert_eq!(log["cloudflare_tail_consumer"], false);
        assert_eq!(log["volumes"][0]["durable"], true);
        assert_eq!(
            plan.get("NativeConsoleTopology").unwrap().props["blockers"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }
}

#[test]
fn native_console_rendered_plan_never_provisions_legacy_handlers() {
    let plan = build(Stage::new("production"));
    let rendered = crate::cloud::terraform::emit(&plan);
    for name in ["AuthApi", "AuthStorage", "Console", "LogProcessor", "Stat"] {
        assert!(rendered["module"].get(name).is_none());
        assert_eq!(rendered["locals"][name]["deployable"], false);
    }
    for secret in [
        "OPENCODE_ISSUER_SIGNING_KEY_PEM",
        "OPENCODE_ISSUER_RETIRED_JWKS",
        "OPENCODE_ISSUER_KEY_ID",
        "OPENCODE_CONSOLE_LOG_INGRESS_TOKEN",
    ] {
        assert_eq!(rendered["variable"][secret]["sensitive"], true);
        assert!(rendered["variable"][secret].get("default").is_none());
    }
    let env = &rendered["locals"]["Console"]["service"]["processes"]["axum"]["environment"];
    assert_eq!(
        env["OPENCODE_CONSOLE_LOG_INGRESS_TOKEN"],
        "${var.OPENCODE_CONSOLE_LOG_INGRESS_TOKEN}"
    );
    assert_eq!(
        env["OPENCODE_ZEN_BLACK_PRICE_PLAN200"],
        "${local.ZEN_BLACK_PRICE.plan200}"
    );
    assert!(!rendered.to_string().contains("unresolved."));
}
