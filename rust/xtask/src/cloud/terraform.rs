//! Purpose: emit a deterministic Terraform JSON document (`main.tf.json`) for one stage plan.
//! Scope: pure transformation from `Plan` to `serde_json::Value`, no I/O.
//! Uses:
//! rust/xtask/
//! `-- src/cloud/model.rs
//! External:
//! |-- serde_json::Map
//! |-- serde_json::Value
//! |-- serde_json::json
//! `-- serde_json::to_string
//! Used by:
//! rust/xtask/
//! |-- src/cloud/mod.rs
//! `-- src/cloud/tests.rs
//! Invariants: provider primitives map to provider resources, SST composite components map to
//! `module` blocks with source `./modules/<component>` (module bodies are a recorded blocker),
//! `sst.Secret` maps to sensitive variables without defaults, linkables map to locals, dev
//! commands are excluded from Terraform and listed under the `//` metadata key. Production
//! resources get `lifecycle.prevent_destroy` (protect, sst.config.ts:8).
//! Contract tests: xtask/src/cloud/tests.rs

use serde_json::{Map, Value, json};

use crate::cloud::model::{Mode, Plan, Resource};

/// Terraform address target for a resource kind.
enum Target {
    Resource(&'static str),
    Data(&'static str),
    Module(&'static str),
}

fn target(kind: &str) -> Option<Target> {
    Some(match kind {
        "cloudflare.DnsRecord" => Target::Resource("cloudflare_dns_record"),
        "cloudflare.RegionalHostname" => Target::Resource("cloudflare_regional_hostname"),
        "sst.cloudflare.Bucket" => Target::Resource("cloudflare_r2_bucket"),
        "sst.cloudflare.Kv" => Target::Resource("cloudflare_workers_kv_namespace"),
        "random.RandomPassword" => Target::Resource("random_password"),
        "stripe.WebhookEndpoint" => Target::Resource("stripe_webhook_endpoint"),
        "stripe.Product" => Target::Resource("stripe_product"),
        "stripe.Price" => Target::Resource("stripe_price"),
        "stripe.Coupon" => Target::Resource("stripe_coupon"),
        "planetscale.Branch" => Target::Resource("planetscale_branch"),
        "planetscale.Password" => Target::Resource("planetscale_password"),
        "honeycombio.WebhookRecipient" => Target::Resource("honeycombio_webhook_recipient"),
        "honeycombio.Trigger" => Target::Resource("honeycombio_trigger"),
        "planetscale.getDatabase" => Target::Data("planetscale_database"),
        "planetscale.getBranch" => Target::Data("planetscale_branch"),
        "sst.cloudflare.Worker" => Target::Module("cloudflare-worker"),
        "sst.cloudflare.x.SolidStart" => Target::Module("cloudflare-solidstart"),
        "sst.cloudflare.x.Astro" => Target::Module("cloudflare-astro"),
        "sst.cloudflare.StaticSite" => Target::Module("cloudflare-static-site"),
        "sst.aws.Vpc" => Target::Module("aws-vpc"),
        "sst.aws.Cluster" => Target::Module("aws-ecs-cluster"),
        "sst.aws.Service" => Target::Module("aws-ecs-service"),
        _ => return None,
    })
}

/// Terraform expression prefix for a named node in this plan.
fn address(plan: &Plan, name: &str) -> String {
    let Some(resource) = plan.get(name) else {
        return format!("unresolved.{name}");
    };
    match resource.mode {
        Mode::Secret => format!("var.{name}"),
        Mode::Linkable => format!("local.{name}"),
        Mode::DevCommand => format!("dev_command.{name}"),
        Mode::Managed | Mode::Data => match target(resource.kind) {
            Some(Target::Resource(t)) => format!("{t}.{name}"),
            Some(Target::Data(t)) => format!("data.{t}.{name}"),
            Some(Target::Module(_)) => format!("module.{name}"),
            None => format!("unresolved.{name}"),
        },
    }
}

/// Rewrite `${ref:Name.attr}`, `${urlencode:Name.attr}` and `${provider:...}` placeholders.
fn resolve(plan: &Plan, value: &Value) -> Value {
    match value {
        Value::String(s) => Value::String(resolve_str(plan, s)),
        Value::Array(items) => Value::Array(items.iter().map(|v| resolve(plan, v)).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), resolve(plan, v)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn resolve_str(plan: &Plan, input: &str) -> String {
    let mut out = String::new();
    let mut rest = input;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let token = &after[..end];
        let rendered = match token.split_once(':') {
            Some(("ref", path)) => {
                let (name, attr) = path.split_once('.').unwrap_or((path, ""));
                let base = address(plan, name);
                if plan.get(name).is_some_and(|r| r.mode == Mode::Secret) {
                    format!("${{{base}}}")
                } else {
                    format!("${{{base}.{attr}}}")
                }
            }
            Some(("urlencode", path)) => {
                let (name, attr) = path.split_once('.').unwrap_or((path, ""));
                format!("${{urlencode({}.{attr})}}", address(plan, name))
            }
            Some(("provider", "cloudflare.default_account_id")) => {
                "${var.cloudflare_account_id}".into()
            }
            _ => format!("${{{token}}}"),
        };
        out.push_str(&rendered);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

fn body(plan: &Plan, resource: &Resource) -> Map<String, Value> {
    let mut map = match resolve(plan, &resource.props) {
        Value::Object(map) => map,
        _ => Map::new(),
    };
    if !resource.links.is_empty() {
        let links: Map<String, Value> = resource
            .links
            .iter()
            .map(|l| {
                (
                    l.clone(),
                    Value::String(format!("${{{}}}", address(plan, l))),
                )
            })
            .collect();
        map.insert("links".into(), Value::Object(links));
    }
    if resource.kind == "honeycombio.Trigger"
        && let Some(query) = map.remove("query_json")
    {
        map.insert(
            "query_json".into(),
            Value::String(serde_json::to_string(&query).unwrap_or_default()),
        );
    }
    if matches!(resource.kind, "sst.cloudflare.Bucket" | "sst.cloudflare.Kv") {
        map.insert("account_id".into(), json!("${var.cloudflare_account_id}"));
        map.insert(
            "name".into(),
            json!(format!(
                "opencode-{}-{}",
                plan.stage.name,
                resource.name.to_lowercase()
            )),
        );
    }
    map.insert("//".into(), json!(format!("source: {}", resource.source)));
    map
}

pub fn emit(plan: &Plan) -> Value {
    let mut variables = Map::new();
    variables.insert("cloudflare_account_id".into(), json!({ "type": "string" }));
    let mut locals = Map::new();
    let mut resources: Map<String, Value> = Map::new();
    let mut data: Map<String, Value> = Map::new();
    let mut modules = Map::new();
    let mut dev_commands = Vec::new();
    for resource in &plan.resources {
        match resource.mode {
            Mode::Secret => {
                variables.insert(
                    resource.name.clone(),
                    json!({ "type": "string", "sensitive": true, "description": format!("sst.Secret {}", resource.source) }),
                );
            }
            Mode::Linkable => {
                locals.insert(resource.name.clone(), resolve(plan, &resource.props));
            }
            Mode::DevCommand => dev_commands.push(json!(resource.name)),
            Mode::Managed | Mode::Data => {
                let mut map = body(plan, resource);
                match target(resource.kind) {
                    Some(Target::Resource(t)) => {
                        if plan.stage.protect() {
                            map.insert("lifecycle".into(), json!({ "prevent_destroy": true }));
                        }
                        insert_typed(&mut resources, t, &resource.name, map);
                    }
                    Some(Target::Data(t)) => insert_typed(&mut data, t, &resource.name, map),
                    Some(Target::Module(m)) => {
                        map.insert("source".into(), json!(format!("./modules/{m}")));
                        map.insert("protect".into(), json!(plan.stage.protect()));
                        modules.insert(resource.name.clone(), Value::Object(map));
                    }
                    None => {
                        map.insert("unmapped_kind".into(), json!(resource.kind));
                        modules.insert(resource.name.clone(), Value::Object(map));
                    }
                }
            }
        }
    }
    let outputs: Map<String, Value> = plan
        .outputs
        .iter()
        .map(|(k, v)| (k.clone(), json!({ "value": resolve(plan, v) })))
        .collect();
    json!({
        "//": {
            "generator": "cargo xtask cloud plan",
            "stage": plan.stage.name,
            "removal": plan.stage.removal(),
            "protect": plan.stage.protect(),
            "home": "cloudflare",
            "dev_commands": dev_commands
        },
        "terraform": {
            "required_providers": {
                "aws": { "source": "hashicorp/aws", "version": "7.30.0" },
                "cloudflare": { "source": "cloudflare/cloudflare" },
                "stripe": { "source": "lukasaron/stripe", "version": "0.0.28" },
                "random": { "source": "hashicorp/random", "version": "4.19.2" },
                "planetscale": { "source": "planetscale/planetscale", "version": "0.4.1" },
                "honeycombio": { "source": "honeycombio/honeycombio", "version": "0.49.0" }
            }
        },
        "provider": {
            "aws": { "region": "us-east-1", "profile": "${var.aws_profile}" },
            "stripe": { "api_key": "${var.STRIPE_SECRET_KEY}" }
        },
        "variable": with_profile(variables, plan),
        "locals": locals,
        "data": data,
        "resource": resources,
        "module": modules,
        "output": outputs
    })
}

fn with_profile(mut variables: Map<String, Value>, plan: &Plan) -> Map<String, Value> {
    // sst.config.ts:14-18, null under GitHub Actions.
    variables.insert(
        "aws_profile".into(),
        json!({ "type": "string", "nullable": true, "default": plan.stage.aws_profile(false) }),
    );
    variables
}

fn insert_typed(into: &mut Map<String, Value>, kind: &str, name: &str, body: Map<String, Value>) {
    let entry = into
        .entry(kind.to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if let Value::Object(map) = entry {
        map.insert(name.to_string(), Value::Object(body));
    }
}
