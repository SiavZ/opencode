//! Purpose: port of infra/console.ts (PlanetScale branch/password, auth Worker + KV, Stripe webhook,
//! products, prices and coupons, console SolidStart site, log tail Worker, Stat Worker).
//! Scope: pure plan construction. Phase 12 owns the console runtime behavior behind these resources.
//! Uses:
//! rust/xtask/
//! |-- src/cloud/infra/stage.rs
//! `-- src/cloud/model.rs
//! External:
//! `-- serde_json::json
//! Used by:
//! rust/xtask/
//! `-- src/cloud/infra/mod.rs
//! Invariants: production reads the existing `production` branch, other stages create a branch
//! named after the stage with parent `production` (infra/console.ts:14-26). Dev uses fixed Go
//! product/price IDs (infra/console.ts:151-153). Preview stages have empty migration URLs
//! (infra/console.ts:223-231). CLOUDFLARE_* secrets only exist under `sst dev`.
//! Contract tests: xtask/tests/cloud_plan.rs

use serde_json::json;

use crate::cloud::infra::stage::secret;
use crate::cloud::model::{Mode, Plan, Resource, reference};

pub const STRIPE_EVENTS: [&str; 26] = [
    "checkout.session.async_payment_failed",
    "checkout.session.async_payment_succeeded",
    "checkout.session.completed",
    "checkout.session.expired",
    "charge.refunded",
    "invoice.payment_succeeded",
    "invoice.payment_failed",
    "invoice.payment_action_required",
    "customer.created",
    "customer.deleted",
    "customer.updated",
    "customer.discount.created",
    "customer.discount.deleted",
    "customer.discount.updated",
    "customer.source.created",
    "customer.source.deleted",
    "customer.source.expiring",
    "customer.source.updated",
    "customer.subscription.created",
    "customer.subscription.deleted",
    "customer.subscription.paused",
    "customer.subscription.pending_update_applied",
    "customer.subscription.pending_update_expired",
    "customer.subscription.resumed",
    "customer.subscription.trial_will_end",
    "customer.subscription.updated",
];

/// PlanetScale database lookup, stage branch and password, plus the `Database`-style linkable.
/// Shared shape between infra/console.ts:9-42 and infra/stats.ts:13-52.
pub fn planetscale(
    plan: &mut Plan,
    prefix: &str,
    database: &str,
    sources: [&'static str; 4],
    url: bool,
) {
    let cluster = format!("{prefix}Cluster");
    plan.add(
        Resource::new(Mode::Data, "planetscale.getDatabase", &cluster, sources[0])
            .props(json!({ "name": database, "organization": "anomalyco" })),
    );
    let branch = format!("{prefix}DatabaseBranch");
    let branch_resource = if plan.stage.is_production() {
        Resource::new(Mode::Data, "planetscale.getBranch", &branch, sources[1]).props(
            json!({ "name": "production", "organization": "anomalyco", "database": database }),
        )
    } else {
        Resource::new(Mode::Managed, "planetscale.Branch", &branch, sources[1]).props(json!({
            "database": database, "organization": "anomalyco", "name": plan.stage.name, "parent_branch": "production"
        }))
    };
    plan.add(branch_resource);
    let password = format!("{prefix}DatabasePassword");
    plan.add(
        Resource::new(Mode::Managed, "planetscale.Password", &password, sources[2]).props(json!({
            "name": plan.stage.name, "database": database, "organization": "anomalyco",
            "branch": reference(&branch, "name")
        })),
    );
    let mut props = json!({
        "host": reference(&password, "access_host_url"),
        "database": database,
        "username": reference(&password, "username"),
        "password": reference(&password, "plaintext"),
        "port": 3306
    });
    if url {
        props["url"] = json!(format!(
            "mysql://${{urlencode:{password}.username}}:${{urlencode:{password}.plaintext}}@${{ref:{password}.access_host_url}}/{database}"
        ));
    }
    let linkable = if prefix.is_empty() {
        "Database".to_string()
    } else {
        format!("{prefix}Database")
    };
    plan.add(Resource::new(Mode::Linkable, "sst.Linkable", &linkable, sources[3]).props(props));
}

pub fn console(plan: &mut Plan) {
    let domain = plan.stage.domain();
    let stage = plan.stage.name.clone();
    planetscale(
        plan,
        "",
        "opencode",
        [
            "infra/console.ts:9-12",
            "infra/console.ts:14-26",
            "infra/console.ts:27-32",
            "infra/console.ts:34-42",
        ],
        false,
    );
    plan.add(
        Resource::new(Mode::DevCommand, "sst.x.DevCommand", "Studio", "infra/console.ts:44-51")
            .props(json!({ "command": "bun db studio", "directory": "packages/console/core", "autostart": true }))
            .links(["Database"]),
    );
    plan.add(secret(
        "GITHUB_CLIENT_ID_CONSOLE",
        "infra/console.ts:57",
        false,
    ));
    plan.add(secret(
        "GITHUB_CLIENT_SECRET_CONSOLE",
        "infra/console.ts:58",
        false,
    ));
    plan.add(secret("GOOGLE_CLIENT_ID", "infra/console.ts:59", false));
    plan.add(Resource::new(
        Mode::Managed,
        "sst.cloudflare.Kv",
        "AuthStorage",
        "infra/console.ts:60",
    ));
    plan.add(
        Resource::new(Mode::Managed, "sst.cloudflare.Worker", "AuthApi", "infra/console.ts:61-66")
            .props(json!({ "domain": format!("auth.{domain}"), "handler": "packages/console/function/src/auth.ts", "url": true }))
            .links(["Database", "AuthStorage", "GITHUB_CLIENT_ID_CONSOLE", "GITHUB_CLIENT_SECRET_CONSOLE", "GOOGLE_CLIENT_ID"]),
    );
    plan.add(
        Resource::new(Mode::Managed, "stripe.WebhookEndpoint", "StripeWebhookEndpoint", "infra/console.ts:72-102")
            .props(json!({ "url": format!("https://{domain}/stripe/webhook"), "enabled_events": STRIPE_EVENTS })),
    );
    stripe_go(plan, &stage);
    stripe_black(plan);
    for i in 1..=30 {
        plan.add(secret(
            &format!("ZEN_MODELS{i}"),
            "infra/console.ts:186-217",
            false,
        ));
    }
    plan.add(secret("STRIPE_SECRET_KEY", "infra/console.ts:218", false));
    plan.add(secret(
        "STRIPE_PUBLISHABLE_KEY",
        "infra/console.ts:219",
        false,
    ));
    plan.add(
        Resource::new(
            Mode::Linkable,
            "sst.Linkable",
            "AUTH_API_URL",
            "infra/console.ts:220-222",
        )
        .props(json!({ "value": reference("AuthApi", "url") })),
    );
    let migration = match stage.as_str() {
        "production" => Some("opencode.ai"),
        "dev" => Some("dev.opencode.ai"),
        _ => None,
    };
    plan.add(
        Resource::new(
            Mode::Linkable,
            "sst.Linkable",
            "ConsoleMigration",
            "infra/console.ts:223-231",
        )
        .props(json!({
            "consoleUrl": migration.map(|d| format!("https://{d}/console")).unwrap_or_default(),
            "inferenceUrl": migration.map(|d| format!("https://{d}/inference")).unwrap_or_default()
        })),
    );
    plan.add(
        Resource::new(
            Mode::Linkable,
            "sst.Linkable",
            "STRIPE_WEBHOOK_SECRET",
            "infra/console.ts:232-234",
        )
        .props(json!({ "value": reference("StripeWebhookEndpoint", "secret") })),
    );
    plan.add(Resource::new(
        Mode::Managed,
        "sst.cloudflare.Bucket",
        "ZenData",
        "infra/console.ts:240",
    ));
    plan.add(Resource::new(
        Mode::Managed,
        "sst.cloudflare.Bucket",
        "ZenDataNew",
        "infra/console.ts:241",
    ));
    for (name, line) in [
        ("DISCORD_INCIDENT_WEBHOOK_URL", "infra/console.ts:243"),
        ("AWS_SES_ACCESS_KEY_ID", "infra/console.ts:244"),
        ("AWS_SES_SECRET_ACCESS_KEY", "infra/console.ts:245"),
        ("ENTERPRISE_SALES_INBOX_EMAIL", "infra/console.ts:246"),
        ("SALESFORCE_CLIENT_ID", "infra/console.ts:248"),
        ("SALESFORCE_CLIENT_SECRET", "infra/console.ts:249"),
        ("SALESFORCE_INSTANCE_URL", "infra/console.ts:250"),
    ] {
        plan.add(secret(name, line, false));
    }
    plan.add(
        Resource::new(
            Mode::Managed,
            "sst.cloudflare.Worker",
            "LogProcessor",
            "infra/console.ts:252-255",
        )
        .props(json!({ "handler": "packages/console/function/src/log-processor.ts" }))
        .links(["HONEYCOMB_API_KEY"]),
    );
    console_site(plan, &domain);
    plan.add(
        Resource::new(
            Mode::Managed,
            "sst.cloudflare.Worker",
            "Stat",
            "infra/console.ts:319-323",
        )
        .props(json!({ "handler": "packages/console/function/src/stat.ts", "url": true }))
        .links(["Database"]),
    );
}

fn stripe_go(plan: &mut Plan, stage: &str) {
    plan.add(
        Resource::new(
            Mode::Managed,
            "stripe.Product",
            "ZenLite",
            "infra/console.ts:104-106",
        )
        .props(json!({ "name": "OpenCode Go" })),
    );
    let product = reference("ZenLite", "id");
    for (name, label, percent, duration, months, line) in [
        (
            "ZenLiteCouponFirstMonth50",
            "First month 50% off",
            50,
            "once",
            None,
            "infra/console.ts:107-112",
        ),
        (
            "ZenLiteCouponFirstMonth100",
            "First month 100% off",
            100,
            "once",
            None,
            "infra/console.ts:113-118",
        ),
        (
            "ZenLiteCoupon3Months100",
            "3 months 100% off",
            100,
            "repeating",
            Some(3),
            "infra/console.ts:119-125",
        ),
        (
            "ZenLiteCoupon6Months100",
            "6 months 100% off",
            100,
            "repeating",
            Some(6),
            "infra/console.ts:126-132",
        ),
        (
            "ZenLiteCoupon12Months100",
            "12 months 100% off",
            100,
            "repeating",
            Some(12),
            "infra/console.ts:133-139",
        ),
    ] {
        let mut props = json!({ "name": label, "percent_off": percent, "applies_to_products": [product], "duration": duration });
        if let Some(months) = months {
            props["duration_in_months"] = json!(months);
        }
        plan.add(Resource::new(Mode::Managed, "stripe.Coupon", name, line).props(props));
    }
    plan.add(Resource::new(Mode::Managed, "stripe.Price", "ZenLitePrice", "infra/console.ts:140-148").props(json!({
        "product": product, "currency": "usd", "recurring": { "interval": "month", "interval_count": 1 }, "unit_amount": 1000
    })));
    let dev = stage == "dev";
    plan.add(Resource::new(Mode::Linkable, "sst.Linkable", "ZEN_LITE_PRICE", "infra/console.ts:149-161").props(json!({
        "product": if dev { json!("prod_U1tUscpmwtV2bG") } else { reference("ZenLite", "id") },
        "price": if dev { json!("price_1T3phhE7fOCwHSD4zS6w2NPy") } else { reference("ZenLitePrice", "id") },
        "priceInr": 92900,
        "firstMonth50Coupon": reference("ZenLiteCouponFirstMonth50", "id"),
        "firstMonth100Coupon": reference("ZenLiteCouponFirstMonth100", "id"),
        "threeMonths100Coupon": reference("ZenLiteCoupon3Months100", "id"),
        "sixMonths100Coupon": reference("ZenLiteCoupon6Months100", "id"),
        "twelveMonths100Coupon": reference("ZenLiteCoupon12Months100", "id")
    })));
}

fn stripe_black(plan: &mut Plan) {
    plan.add(
        Resource::new(
            Mode::Managed,
            "stripe.Product",
            "ZenBlack",
            "infra/console.ts:163-165",
        )
        .props(json!({ "name": "OpenCode Black" })),
    );
    for (name, amount, line) in [
        ("ZenBlackPrice", 20000, "infra/console.ts:166-174"),
        ("ZenBlack100Price", 10000, "infra/console.ts:166-175"),
        ("ZenBlack20Price", 2000, "infra/console.ts:166-176"),
    ] {
        plan.add(
            Resource::new(Mode::Managed, "stripe.Price", name, line).props(json!({
                "product": reference("ZenBlack", "id"), "currency": "usd",
                "recurring": { "interval": "month", "interval_count": 1 }, "unit_amount": amount
            })),
        );
    }
    plan.add(
        Resource::new(
            Mode::Linkable,
            "sst.Linkable",
            "ZEN_BLACK_PRICE",
            "infra/console.ts:177-184",
        )
        .props(json!({
            "product": reference("ZenBlack", "id"),
            "plan200": reference("ZenBlackPrice", "id"),
            "plan100": reference("ZenBlack100Price", "id"),
            "plan20": reference("ZenBlack20Price", "id")
        })),
    );
}

fn console_site(plan: &mut Plan, domain: &str) {
    plan.add(secret("ZEN_LIMITS", "infra/console.ts:282", false));
    plan.add(secret("ZEN_SESSION_SECRET", "infra/console.ts:283", false));
    plan.add(secret(
        "CLOUDFLARE_ACCESS_CLIENT_ID",
        "infra/console.ts:284",
        false,
    ));
    let mut links: Vec<String> = [
        "ZenData",
        "ZenDataNew",
        "Database",
        "UpstashRedisRestUrl",
        "UpstashRedisRestToken",
        "AUTH_API_URL",
        "ConsoleMigration",
        "STRIPE_WEBHOOK_SECRET",
        "SUPPORT_API_KEY",
        "DISCORD_INCIDENT_WEBHOOK_URL",
        "HoneycombWebhookSecret",
        "STRIPE_SECRET_KEY",
        "EMAILOCTOPUS_API_KEY",
        "AWS_SES_ACCESS_KEY_ID",
        "AWS_SES_SECRET_ACCESS_KEY",
        "ENTERPRISE_SALES_INBOX_EMAIL",
        "SALESFORCE_CLIENT_ID",
        "SALESFORCE_CLIENT_SECRET",
        "SALESFORCE_INSTANCE_URL",
        "ZEN_BLACK_PRICE",
        "ZEN_LITE_PRICE",
        "ZEN_LIMITS",
        "ZEN_SESSION_SECRET",
        "CLOUDFLARE_ACCESS_CLIENT_ID",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    links.extend((1..=30).map(|i| format!("ZEN_MODELS{i}")));
    if plan.stage.sst_dev {
        // infra/console.ts:286-291, values come from the developer's environment.
        plan.add(secret(
            "CLOUDFLARE_DEFAULT_ACCOUNT_ID",
            "infra/console.ts:288",
            true,
        ));
        plan.add(secret("CLOUDFLARE_API_TOKEN", "infra/console.ts:289", true));
        links.push("CLOUDFLARE_DEFAULT_ACCOUNT_ID".into());
        links.push("CLOUDFLARE_API_TOKEN".into());
    }
    plan.add(
        Resource::new(
            Mode::Managed,
            "sst.cloudflare.x.SolidStart",
            "Console",
            "infra/console.ts:257-313",
        )
        .props(json!({
            "domain": domain,
            "path": "packages/console/app",
            "environment": {
                "VITE_AUTH_URL": reference("AuthApi", "url"),
                "VITE_STRIPE_PUBLISHABLE_KEY": reference("STRIPE_PUBLISHABLE_KEY", "value")
            },
            "server": {
                "placement": { "region": "aws:us-east-2" },
                "extra_compatibility_flags": ["global_fetch_strictly_public"],
                "tail_consumers": [{ "service": reference("LogProcessor", "script_name") }]
            }
        }))
        .links(links),
    );
}
