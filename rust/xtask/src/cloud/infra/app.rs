//! Purpose: port of infra/app.ts (public API Worker with SyncServer DO, docs site, web app) and
//! infra/enterprise.ts (enterprise share storage and Teams SolidStart site).
//! Scope: pure plan construction.
//! Uses:
//! rust/xtask/
//! |-- src/cloud/infra/stage.rs
//! `-- src/cloud/model.rs
//! External:
//! |-- serde_json::Value
//! `-- serde_json::json
//! Used by:
//! rust/xtask/
//! `-- src/cloud/infra/mod.rs
//! Invariants: SYNC_SERVER binding and DO migrations are skipped for stages vimtor/adam,
//! migration tags are "" for production/thdxr and "v1" elsewhere (infra/app.ts:30-48).
//! Contract tests: xtask/tests/cloud_plan.rs

use serde_json::{Value, json};

use crate::cloud::infra::stage::secret;
use crate::cloud::model::{Mode, Plan, Resource, reference};

pub fn app(plan: &mut Plan) {
    let domain = plan.stage.domain();
    for (name, line) in [
        ("GITHUB_APP_ID", "infra/app.ts:3"),
        ("GITHUB_APP_PRIVATE_KEY", "infra/app.ts:4"),
        ("EMAILOCTOPUS_API_KEY", "infra/app.ts:5"),
        ("ADMIN_SECRET", "infra/app.ts:6"),
        ("DISCORD_SUPPORT_BOT_TOKEN", "infra/app.ts:7"),
        ("DISCORD_SUPPORT_CHANNEL_ID", "infra/app.ts:8"),
        ("FEISHU_APP_ID", "infra/app.ts:9"),
        ("FEISHU_APP_SECRET", "infra/app.ts:10"),
    ] {
        plan.add(secret(name, line, false));
    }
    plan.add(Resource::new(
        Mode::Managed,
        "sst.cloudflare.Bucket",
        "Bucket",
        "infra/app.ts:11",
    ));

    let stage = plan.stage.name.clone();
    let durable_object = !matches!(stage.as_str(), "vimtor" | "adam");
    let tag = if matches!(stage.as_str(), "production" | "thdxr") {
        ""
    } else {
        "v1"
    };
    let worker = if durable_object {
        json!({
            "logpush": true,
            "extra_bindings": [{ "name": "SYNC_SERVER", "type": "durable_object_namespace", "class_name": "SyncServer" }],
            "migrations": { "old_tag": tag, "new_tag": tag }
        })
    } else {
        json!({ "logpush": true })
    };
    plan.add(
        Resource::new(
            Mode::Managed,
            "sst.cloudflare.Worker",
            "Api",
            "infra/app.ts:13-50",
        )
        .props(json!({
            "domain": format!("api.{domain}"),
            "handler": "packages/function/src/api.ts",
            "environment": { "WEB_DOMAIN": domain },
            "url": true,
            "worker": worker
        }))
        .links([
            "Bucket",
            "GITHUB_APP_ID",
            "GITHUB_APP_PRIVATE_KEY",
            "ADMIN_SECRET",
            "DISCORD_SUPPORT_BOT_TOKEN",
            "DISCORD_SUPPORT_CHANNEL_ID",
            "FEISHU_APP_ID",
            "FEISHU_APP_SECRET",
        ]),
    );
    plan.add(
        Resource::new(
            Mode::Managed,
            "sst.cloudflare.x.Astro",
            "Web",
            "infra/app.ts:52-60",
        )
        .props(json!({
            "domain": format!("docs.{domain}"),
            "path": "packages/web",
            "environment": { "SST_STAGE": stage, "VITE_API_URL": reference("Api", "url") }
        })),
    );
    plan.add(
        Resource::new(
            Mode::Managed,
            "sst.cloudflare.StaticSite",
            "WebApp",
            "infra/app.ts:62-69",
        )
        .props(json!({
            "domain": format!("app.{domain}"),
            "path": "packages/app",
            "build": { "command": "bun turbo build", "output": "./dist" }
        })),
    );
}

pub fn enterprise(plan: &mut Plan) {
    plan.add(Resource::new(
        Mode::Managed,
        "sst.cloudflare.Bucket",
        "EnterpriseStorage",
        "infra/enterprise.ts:4",
    ));
    let env: Value = json!({
        "OPENCODE_STORAGE_ADAPTER": "r2",
        "OPENCODE_STORAGE_ACCOUNT_ID": "${provider:cloudflare.default_account_id}",
        "OPENCODE_STORAGE_ACCESS_KEY_ID": reference("R2AccessKey", "value"),
        "OPENCODE_STORAGE_SECRET_ACCESS_KEY": reference("R2SecretKey", "value"),
        "OPENCODE_STORAGE_BUCKET": reference("EnterpriseStorage", "name")
    });
    plan.add(
        Resource::new(
            Mode::Managed,
            "sst.cloudflare.x.SolidStart",
            "Teams",
            "infra/enterprise.ts:6-18",
        )
        .props(json!({
            "domain": plan.stage.short_domain(),
            "path": "packages/enterprise",
            "build_command": "bun run build:cloudflare",
            "environment": env
        }))
        .links(["SUPPORT_API_KEY"]),
    );
}
