//! Purpose: describe truthful native console service topology without inventing provisioned cloud resources.
//! Scope: pure deployment artifact construction over the baseline infrastructure resource graph.
//! Uses:
//! rust/xtask/
//! |-- src/cloud/infra/stage.rs
//! `-- src/cloud/model.rs
//! External:
//! `-- serde_json::{Value, json}
//! Used by:
//! rust/xtask/
//! `-- src/cloud/infra/console.rs
//! Invariants: native descriptors are linkable artifacts, not provisioned resources; no legacy TS fallback.
//! Contract tests: native/tests.rs

use serde_json::{Value, json};

use crate::cloud::infra::stage::secret;
use crate::cloud::model::{Mode, Plan, Resource, reference};

#[cfg(test)]
mod tests;

pub fn convert(plan: &mut Plan) {
    let domain = plan.stage.domain();
    let database = "mysql://${urlencode:DatabasePassword.username}:${urlencode:DatabasePassword.plaintext}@${ref:DatabasePassword.access_host_url}/opencode";
    for name in [
        "OPENCODE_ISSUER_SIGNING_KEY_PEM",
        "OPENCODE_ISSUER_RETIRED_JWKS",
        "OPENCODE_ISSUER_KEY_ID",
        "OPENCODE_CONSOLE_LOG_INGRESS_TOKEN",
    ] {
        plan.add(secret(name, "rust/native-console-topology", false));
    }
    let mut console_env = json!({
        "OPENCODE_HOSTED_SERVICE":"console",
        "OPENCODE_HOSTED_ADDR":"127.0.0.1:8787",
        "OPENCODE_STAGE":plan.stage.name,
        "OPENCODE_CONSOLE_DATABASE_URL":database,
        "OPENCODE_CONSOLE_AUTH_URL":format!("https://auth.{domain}"),
        "OPENCODE_CONSOLE_PUBLIC_ORIGIN":format!("https://{domain}"),
        "OPENCODE_CONSOLE_AUTH_STATE_REDIS_URL":"redis://127.0.0.1:6379/0",
        "OPENCODE_CONSOLE_SESSION_SECRET":reference("ZEN_SESSION_SECRET","value"),
        "OPENCODE_CONSOLE_MIGRATION_URL":reference("ConsoleMigration","consoleUrl"),
        "OPENCODE_CONSOLE_INFERENCE_URL":reference("ConsoleMigration","inferenceUrl"),
        "UPSTASH_REDIS_REST_URL":reference("UpstashRedisRestUrl","value"),
        "UPSTASH_REDIS_REST_TOKEN":reference("UpstashRedisRestToken","value"),
        "STRIPE_SECRET_KEY":reference("STRIPE_SECRET_KEY","value"),
        "STRIPE_WEBHOOK_SECRET":reference("STRIPE_WEBHOOK_SECRET","value"),
        "ZEN_LIMITS":reference("ZEN_LIMITS","value"),
        "OPENCODE_CONSOLE_TAIL_URL":"http://127.0.0.1:8790/tail",
        "OPENCODE_CONSOLE_LOG_INGRESS_TOKEN":reference("OPENCODE_CONSOLE_LOG_INGRESS_TOKEN","value"),
        "SUPPORT_API_KEY":reference("SUPPORT_API_KEY","value"),
        "HoneycombWebhookSecret":reference("HoneycombWebhookSecret","result"),
        "DISCORD_INCIDENT_WEBHOOK_URL":reference("DISCORD_INCIDENT_WEBHOOK_URL","value"),
        "EMAILOCTOPUS_API_KEY":reference("EMAILOCTOPUS_API_KEY","value"),
        "AWS_SES_ACCESS_KEY_ID":reference("AWS_SES_ACCESS_KEY_ID","value"),
        "AWS_SES_SECRET_ACCESS_KEY":reference("AWS_SES_SECRET_ACCESS_KEY","value"),
        "ENTERPRISE_SALES_INBOX_EMAIL":reference("ENTERPRISE_SALES_INBOX_EMAIL","value"),
        "SALESFORCE_CLIENT_ID":reference("SALESFORCE_CLIENT_ID","value"),
        "SALESFORCE_CLIENT_SECRET":reference("SALESFORCE_CLIENT_SECRET","value"),
        "SALESFORCE_INSTANCE_URL":reference("SALESFORCE_INSTANCE_URL","value"),
        "CLOUDFLARE_ACCESS_CLIENT_ID":reference("CLOUDFLARE_ACCESS_CLIENT_ID","value")
    });
    for (resource, keys) in [
        (
            "ZEN_LITE_PRICE",
            vec![
                "product",
                "price",
                "firstMonth50Coupon",
                "firstMonth100Coupon",
                "threeMonths100Coupon",
                "sixMonths100Coupon",
                "twelveMonths100Coupon",
            ],
        ),
        (
            "ZEN_BLACK_PRICE",
            vec!["product", "plan20", "plan100", "plan200"],
        ),
    ] {
        for key in keys {
            console_env[format!("OPENCODE_{resource}_{}", key.to_ascii_uppercase())] =
                reference(resource, key);
        }
    }
    for index in 1..=30 {
        console_env[format!("ZEN_MODELS{index}")] =
            reference(&format!("ZEN_MODELS{index}"), "value");
    }
    let issuer = json!({
        "type":"native-process","executable":"opencode-hosted","replicas":1,
        "ingress":{"host":format!("auth.{domain}"),"upstream":"http://127.0.0.1:8788","tls_termination":"required trusted ingress","public":true},
        "environment":{
            "OPENCODE_HOSTED_SERVICE":"console-issuer","OPENCODE_HOSTED_ADDR":"127.0.0.1:8788",
            "OPENCODE_CONSOLE_DATABASE_URL":database,"OPENCODE_STAGE":plan.stage.name,
            "OPENCODE_ISSUER_URL":format!("https://auth.{domain}"),
            "OPENCODE_ISSUER_STORE_PATH":"/var/lib/opencode/issuer/oauth.sqlite",
            "OPENCODE_ISSUER_SIGNING_KEY_FILE":"/run/secrets/issuer-signing.pem",
            "OPENCODE_ISSUER_RETIRED_JWKS_FILE":"/run/secrets/issuer-retired-jwks.json",
            "OPENCODE_ISSUER_KEY_ID":reference("OPENCODE_ISSUER_KEY_ID","value"),
            "OPENCODE_ISSUER_GITHUB_CLIENT_ID":reference("GITHUB_CLIENT_ID_CONSOLE","value"),
            "OPENCODE_ISSUER_GITHUB_CLIENT_SECRET":reference("GITHUB_CLIENT_SECRET_CONSOLE","value"),
            "OPENCODE_ISSUER_GOOGLE_CLIENT_ID":reference("GOOGLE_CLIENT_ID","value")
        },
        "volumes":[{"name":"issuer-state","mount":"/var/lib/opencode/issuer","durable":true,"access":"single-writer","backup_required":true}],
        "secret_files":[{"source":"OPENCODE_ISSUER_SIGNING_KEY_PEM","path":"/run/secrets/issuer-signing.pem","read_only":true},{"source":"OPENCODE_ISSUER_RETIRED_JWKS","path":"/run/secrets/issuer-retired-jwks.json","read_only":true}]
    });
    let ssr = json!({
        "type":"node-ssr","runtime":"node>=22.12","build_command":"bun rust/scripts/console-browser/build.ts",
        "entry":"rust/target/console-browser-production/output/server/index.mjs",
        "start_command":"node rust/scripts/console-browser/production-start.mjs",
        "preset":"node-server","vite_plugin":"rust-console-server-actions","baseline_writes":false,
        "build_environment":{"VITE_AUTH_URL":format!("https://auth.{domain}"),"VITE_STRIPE_PUBLISHABLE_KEY":reference("STRIPE_PUBLISHABLE_KEY","value")},
        "environment":{"OPENCODE_BROWSER_PORT":"3001","OPENCODE_SSR_PORT":"3002","OPENCODE_BROWSER_RUST_URL":"http://127.0.0.1:8787","OPENCODE_CONSOLE_PUBLIC_ORIGIN":format!("https://{domain}"),"VITE_AUTH_URL":format!("https://auth.{domain}"),"VITE_STRIPE_PUBLISHABLE_KEY":reference("STRIPE_PUBLISHABLE_KEY","value")},
        "shared_network_namespace":"console-pod"
    });
    let log = json!({
        "type":"native-process","executable":"opencode-hosted",
        "environment":{"OPENCODE_HOSTED_SERVICE":"console-log-processor","OPENCODE_HOSTED_ADDR":"127.0.0.1:8790","OPENCODE_CONSOLE_LOG_INGRESS_TOKEN":reference("OPENCODE_CONSOLE_LOG_INGRESS_TOKEN","value"),"OPENCODE_CONSOLE_WORKER_CAPTURE_FILE":"/var/lib/opencode/logs/enriched.jsonl"},
        "shared_network_namespace":"console-pod",
        "ingress":{"method":"POST","path":"/tail","authentication":"Bearer OPENCODE_CONSOLE_LOG_INGRESS_TOKEN","body":"Cloudflare TraceItem[] compatible JSON","max_body_bytes":8388608,"public":false},
        "volumes":[{"name":"log-outbox","mount":"/var/lib/opencode/logs","durable":true}],
        "delivery":"durable-jsonl-outbox","cloudflare_tail_consumer":false
    });
    let stat = json!({"type":"native-process","executable":"opencode-hosted","environment":{"OPENCODE_HOSTED_SERVICE":"console-stat","OPENCODE_HOSTED_ADDR":"127.0.0.1:8789","OPENCODE_CONSOLE_DATABASE_URL":database},"ingress":{"host":format!("stat.{domain}"),"method":"POST","port":8789}});
    for resource in &mut plan.resources {
        match resource.name.as_str() {
            "AuthApi" => descriptor(
                resource,
                issuer.clone(),
                json!({"url":format!("https://auth.{domain}")}),
            ),
            "AuthStorage" => descriptor(
                resource,
                json!({"type":"durable-volume","path":"/var/lib/opencode/issuer/oauth.sqlite","engine":"sqlite","access":"single-writer"}),
                json!({}),
            ),
            "Console" => descriptor(
                resource,
                json!({"type":"colocated-process-group","network":"shared-loopback","processes":{"axum":{"executable":"opencode-hosted","environment":console_env,"shared_network_namespace":"console-pod"},"ssr":ssr,"auth_state_redis":{"image":"redis:7","shared_network_namespace":"console-pod","command":["redis-server","--bind","127.0.0.1","--appendonly","yes","--dir","/data"],"volume":{"name":"auth-state","mount":"/data","durable":true},"public":false}},"routing":{"public_origin":format!("https://{domain}"),"public_entry":"production-start.mjs","ssr_port":3001,"axum_port":8787,"tls_termination":"required trusted ingress forwarding exact public origin"}}),
                json!({"url":format!("https://{domain}")}),
            ),
            "LogProcessor" => descriptor(resource, log.clone(), json!({})),
            "Stat" => descriptor(
                resource,
                stat.clone(),
                json!({"url":format!("https://stat.{domain}")}),
            ),
            _ => {}
        }
    }
    plan.add(Resource::new(Mode::Linkable,"native.ConsoleTopology","NativeConsoleTopology","rust/native-console-topology").props(json!({
        "version":1,"status":"deployment_backend_required","deployable":false,
        "services":["Console","AuthApi","Stat","LogProcessor"],
        "artifacts":{"hosted":"cargo build --release -p opencode-hosted","browser":"bun rust/scripts/console-browser/build.ts"},
        "blockers":[
            {"id":"native-host-backend","detail":"No provider/container scheduler, durable-volume provisioning, TLS ingress or secret-file mount renderer exists in this Terraform planner. Linkable descriptors are not deployed infrastructure."},
            {"id":"cloudflare-export-binding","detail":"Native gateway TraceItem producer targets authenticated loopback tail ingress and durable local capture. Existing Cloudflare workers need a separate authenticated export attachment; no native binary is usable as a Cloudflare tail_consumer. Honeycomb uploads remain disabled as in baseline."}
        ]
    })));
}

fn descriptor(resource: &mut Resource, service: Value, outputs: Value) {
    resource.mode = Mode::Linkable;
    resource.kind = "native.ConsoleService";
    resource.props =
        json!({"status":"deployment_backend_required","deployable":false,"service":service});
    for (key, value) in outputs.as_object().into_iter().flatten() {
        resource.props[key] = value.clone();
    }
    resource.links.clear();
}
