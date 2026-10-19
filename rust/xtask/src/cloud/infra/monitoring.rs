//! Purpose: port of infra/monitoring.ts (Honeycomb webhook recipient, query specs and six triggers).
//! Scope: pure plan construction, only for production and vimtor (sst.config.ts:36-38).
//! Uses:
//! rust/xtask/
//! `-- src/cloud/model.rs
//! External:
//! |-- i32::from
//! |-- serde_json::Value
//! `-- serde_json::json
//! Used by:
//! rust/xtask/
//! |-- src/cloud/infra/mod.rs
//! `-- src/cloud/tests.rs
//! Invariants: calculated field names are `<name>_<base36(hash31(expr) >>> 0)>` exactly as
//! infra/monitoring.ts:34-39, triggers are disabled outside production (infra/monitoring.ts:5).
//! Contract tests: xtask/tests/cloud_plan.rs (hash fixtures captured with bun from the TS function)

use serde_json::{Value, json};

use crate::cloud::model::{Mode, Plan, Resource, reference};

const DESCRIPTION: &str = "Managed by SST (Don't edit in Honeycomb UI)";

const FAILED_HTTP: &str = "\nIF(\n  AND(\n    GTE($status, \"400\"),\n    NOT(EQUALS($status, \"401\")),\n    NOT(\n      AND(\n        EQUALS($status, \"429\"),\n        OR(\n          EQUALS($error.type, \"GoUsageLimitError\"),\n          EQUALS($error.type, \"FreeUsageLimitError\")\n        )\n      )\n    )\n  ),\n  1,\n  0\n)";
const SUCCESS_HTTP: &str = "IF(AND(GTE($status, \"200\"), LT($status, \"400\")), 1, 0)";
const FAILED_PROVIDER: &str = "IF(GT($llm.error.code, \"400\"), 1, 0)";

/// `Math.imul(31, r) + charCode` over UTF-16 code units, then `>>> 0` and base 36.
pub fn field_hash(expression: &str) -> String {
    let mut acc: i32 = 0;
    for unit in expression.encode_utf16() {
        acc = acc.wrapping_mul(31).wrapping_add(i32::from(unit));
    }
    let mut n = acc as u32;
    if n == 0 {
        return "0".into();
    }
    let digits = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut out = Vec::new();
    while n > 0 {
        out.push(digits[(n % 36) as usize]);
        n /= 36;
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

fn field(name: &str, expression: &str) -> Value {
    json!({ "name": format!("{name}_{}", field_hash(expression)), "expression": expression })
}

fn model_http_errors(product: &str) -> Value {
    let filters = json!([
        { "column": "model", "op": "exists" },
        { "column": "event_type", "op": "=", "value": "completions" },
        { "column": "user_agent", "op": "contains", "value": "opencode" },
        { "column": "isGoTier", "op": "=", "value": if product == "go" { "true" } else { "false" } }
    ]);
    let failed = field("is_failed_http_status", FAILED_HTTP);
    json!({
        "breakdowns": ["model"],
        "calculated_fields": [failed],
        "calculations": [
            { "op": "COUNT", "name": "TOTAL", "filter_combination": "AND", "filters": filters },
            { "op": "SUM", "name": "FAILED", "column": failed["name"], "filter_combination": "AND", "filters": filters }
        ],
        "formulas": [{ "name": "ERROR", "expression": "IF(GTE($TOTAL, 150), DIV($FAILED, $TOTAL), 0)" }],
        "time_range": 900
    })
}

fn provider_http_errors() -> Value {
    let base = [
        json!({ "column": "provider", "op": "exists" }),
        json!({ "column": "user_agent", "op": "contains", "value": "opencode" }),
    ];
    let success = field("is_success_http_status", SUCCESS_HTTP);
    let failed = field("is_failed_provider_http_status", FAILED_PROVIDER);
    let mut ok = base.to_vec();
    ok.push(json!({ "column": "event_type", "op": "=", "value": "completions" }));
    let mut bad = base.to_vec();
    bad.push(json!({ "column": "event_type", "op": "=", "value": "llm.error" }));
    bad.push(json!({ "column": "llm.error.code", "op": "!=", "value": "404" }));
    json!({
        "breakdowns": ["provider"],
        "calculated_fields": [success, failed],
        "calculations": [
            { "op": "SUM", "name": "SUCCESS", "column": success["name"], "filter_combination": "AND", "filters": ok },
            { "op": "SUM", "name": "FAILED", "column": failed["name"], "filter_combination": "AND", "filters": bad }
        ],
        "formulas": [{ "name": "ERROR", "expression": "IF(GTE(SUM($SUCCESS, $FAILED), 150), DIV($FAILED, SUM($SUCCESS, $FAILED)), 0)" }],
        "time_range": 900
    })
}

fn model_low_tps(product: &str) -> Value {
    let filters = json!([
        { "column": "model", "op": "exists" },
        { "column": "event_type", "op": "=", "value": "completions" },
        { "column": "user_agent", "op": "contains", "value": "opencode" },
        { "column": "isGoTier", "op": "=", "value": if product == "go" { "true" } else { "false" } },
        { "column": "status", "op": ">=", "value": "200" },
        { "column": "status", "op": "<", "value": "400" },
        { "column": "tps.output", "op": "exists" }
    ]);
    json!({
        "breakdowns": ["model"],
        "calculations": [
            { "op": "COUNT", "name": "TOTAL", "filter_combination": "AND", "filters": filters },
            { "op": "P50", "name": "TPS", "column": "tps.output", "filter_combination": "AND", "filters": filters }
        ],
        "formulas": [{ "name": "LOW_TPS", "expression": "IF(GTE($TOTAL, 100), $TPS, 999)" }],
        "time_range": 1800
    })
}

fn free_tier() -> Value {
    json!({
        "calculations": [{ "op": "COUNT" }],
        "filters": [
            { "column": "event_type", "op": "=", "value": "completions" },
            { "column": "user_agent", "op": "contains", "value": "opencode" },
            { "column": "isFreeTier", "op": "=", "value": "true" }
        ],
        "time_range": 3600
    })
}

pub fn monitoring(plan: &mut Plan) {
    let stage = plan.stage.name.clone();
    let disabled = !plan.stage.is_production();
    let recipient_name = if plan.stage.is_production() {
        "Discord Alerts".to_string()
    } else {
        format!("Discord Alerts ({stage})")
    };
    plan.add(
        Resource::new(Mode::Managed, "honeycombio.WebhookRecipient", "DiscordAlerts", "infra/monitoring.ts:7-29")
            .props(json!({
                "name": recipient_name,
                "url": format!("https://{}/honeycomb/webhook", plan.stage.domain()),
                "secret": reference("HoneycombWebhookSecret", "result"),
                "templates": [{ "type": "trigger", "body": "{\n        \"url\": {{ .Result.URL | quote }},\n        \"type\": {{ .Vars.type | quote }},\n        \"name\": {{ .Name | quote }},\n        \"status\": {{ .Alert.Status | quote }},\n        \"isTest\": {{ .Alert.IsTest }},\n        \"groups\": {{ .Result.GroupsTriggered | toJson }}\n      }" }],
                "variables": [{ "name": "type" }]
            })),
    );
    // (id, name, query, frequency, threshold op, threshold value, alert type, source)
    #[allow(clippy::type_complexity)]
    let triggers: [(&str, &str, Value, u32, &str, f64, &str, &str); 6] = [
        (
            "IncreasedModelHttpErrorsGo",
            "Increased Model HTTP Errors [Go]",
            model_http_errors("go"),
            300,
            ">=",
            0.7,
            "model_http_errors",
            "infra/monitoring.ts:160-178",
        ),
        (
            "IncreasedModelHttpErrorsZen",
            "Increased Model HTTP Errors [Zen]",
            model_http_errors("zen"),
            300,
            ">=",
            0.7,
            "model_http_errors",
            "infra/monitoring.ts:180-198",
        ),
        (
            "LowModelTpsGo",
            "Low Model TPS [Go]",
            model_low_tps("go"),
            600,
            "<=",
            10.0,
            "model_low_tps",
            "infra/monitoring.ts:200-218",
        ),
        (
            "LowModelTpsZen",
            "Low Model TPS [Zen]",
            model_low_tps("zen"),
            600,
            "<=",
            10.0,
            "model_low_tps",
            "infra/monitoring.ts:220-238",
        ),
        (
            "IncreasedProviderHttpErrors",
            "Increased Provider HTTP Errors",
            provider_http_errors(),
            300,
            ">=",
            0.7,
            "provider_http_errors",
            "infra/monitoring.ts:240-258",
        ),
        (
            "IncreasedFreeTierRequests",
            "Increased Free Tier Requests",
            free_tier(),
            900,
            ">=",
            50.0,
            "custom",
            "infra/monitoring.ts:260-287",
        ),
    ];
    for (id, name, query, frequency, op, value, kind, source) in triggers {
        let mut props = json!({
            "name": name,
            "description": DESCRIPTION,
            "disabled": disabled,
            "query_json": query,
            "alert_type": "on_change",
            "frequency": frequency,
            "thresholds": [{ "op": op, "value": value, "exceeded_limit": 1 }],
            "recipients": [{
                "id": reference("DiscordAlerts", "id"),
                "notification_details": [{ "variables": [{ "name": "type", "value": kind }] }]
            }]
        });
        if id == "IncreasedFreeTierRequests" {
            props["baseline_details"] = json!([{ "type": "percentage", "offset_minutes": 1440 }]);
        }
        plan.add(Resource::new(Mode::Managed, "honeycombio.Trigger", id, source).props(props));
    }
}
