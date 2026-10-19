//! Purpose: port of infra/stage.ts and infra/secret.ts resources (DNS, regional hostname, shared secrets).
//! Scope: pure plan construction.
//! Uses:
//! rust/xtask/
//! `-- src/cloud/model.rs
//! External:
//! `-- serde_json::json
//! Used by:
//! rust/xtask/
//! |-- src/cloud/infra/app.rs
//! |-- src/cloud/infra/console.rs
//! |-- src/cloud/infra/mod.rs
//! `-- src/cloud/infra/stats.rs
//! Invariants: TrustCenter records exist only in production (infra/stage.ts:11-28).
//! `sst.Secret` defaults ("unknown", "") are recorded as `has_default`, never as values.
//! Contract tests: xtask/tests/cloud_plan.rs

use serde_json::json;

use crate::cloud::model::{Mode, Plan, Resource, reference};

pub const ZONE_ID: &str = "430ba34c138cfb5360826c4909f99be8";

pub fn stage(plan: &mut Plan) {
    if plan.stage.is_production() {
        plan.add(
            Resource::new(
                Mode::Managed,
                "cloudflare.DnsRecord",
                "TrustCenter",
                "infra/stage.ts:12-19",
            )
            .props(json!({
                "zone_id": ZONE_ID, "name": "trust.opencode.ai", "type": "CNAME",
                "content": "3a69a5bb27875189.vercel-dns-016.com", "proxied": false, "ttl": 60
            })),
        );
        plan.add(
            Resource::new(
                Mode::Managed,
                "cloudflare.DnsRecord",
                "TrustCenterVerification",
                "infra/stage.ts:21-27",
            )
            .props(json!({
                "zone_id": ZONE_ID, "name": "opencode.ai", "type": "TXT",
                "content": "compai-domain-verification=org_6993a99c6200a2d642bb115d", "ttl": 60
            })),
        );
    }
    let domain = plan.stage.domain();
    plan.add(
        Resource::new(
            Mode::Managed,
            "cloudflare.RegionalHostname",
            "RegionalHostname",
            "infra/stage.ts:30-34",
        )
        .props(json!({ "hostname": domain, "region_key": "us", "zone_id": ZONE_ID })),
    );
}

pub fn secret(name: &str, source: &'static str, has_default: bool) -> Resource {
    Resource::new(Mode::Secret, "sst.Secret", name, source)
        .props(json!({ "has_default": has_default }))
}

pub fn secrets(plan: &mut Plan) {
    plan.add(secret("R2AccessKey", "infra/secret.ts:8", true));
    plan.add(secret("R2SecretKey", "infra/secret.ts:9", true));
    plan.add(secret("HONEYCOMB_API_KEY", "infra/secret.ts:10", false));
    plan.add(
        Resource::new(Mode::Managed, "random.RandomPassword", "HoneycombWebhookSecret", "infra/secret.ts:1-5,11")
            .props(json!({ "length": 24, "linkable": { "value": reference("HoneycombWebhookSecret", "result") } })),
    );
    plan.add(secret("SUPPORT_API_KEY", "infra/secret.ts:12", false));
    plan.add(secret("UpstashRedisRestUrl", "infra/secret.ts:13", false));
    plan.add(secret("UpstashRedisRestToken", "infra/secret.ts:14", false));
}
