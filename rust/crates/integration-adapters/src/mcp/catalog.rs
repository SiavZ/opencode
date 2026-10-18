//! Purpose: MCP catalog operations, port of packages/opencode/src/mcp/catalog.ts: cursor
//! pagination with duplicate-cursor and 1000-page guards, tolerant tools/list (outputSchema
//! dropped when invalid), prompts/resources/templates listing, catalog keys, and tools/call
//! result translation (isError -> error text, structuredContent -> text).
//! Scope: pure functions over a connected `McpClient`.
//! Uses:
//! rust/crates/
//! |-- integration-adapters/src/mcp/client.rs
//! |-- integration-adapters/src/mcp/jsonrpc.rs
//! `-- integration-api/src/mcp.rs
//! External:
//! `-- serde_json::{Value, json}
//! Used by:
//! rust/crates/
//! |-- integration-adapters/src/mcp/mod.rs
//! `-- integration-adapters/src/mcp/service.rs
//! Invariants: `tool_name` = sanitize(client) + "_" + sanitize(tool); resource keys escape `%`
//! and `:` in the client name; tool output validators only apply to tools whose outputSchema
//! survived listing (SDK behavior).
//! Contract tests: integration-adapters/tests/mcp_stdio.rs

use std::collections::BTreeMap;

use opencode_integration_api::mcp::{MAX_LIST_PAGES, ToolDef, sanitize_name};
use serde_json::{Value, json};

use super::client::McpClient;
use super::jsonrpc::RpcError;

pub fn tool_name(client: &str, tool: &str) -> String {
    format!("{}_{}", sanitize_name(client), sanitize_name(tool))
}

/// Baseline `paginate`: follow `nextCursor` until absent, rejecting repeats and runaway lists.
pub async fn paginate<F, Fut>(mut list: F, field: &str) -> Result<Vec<Value>, RpcError>
where
    F: FnMut(Option<String>) -> Fut,
    Fut: std::future::Future<Output = Result<Value, RpcError>>,
{
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut cursor = None;
    for _ in 0..MAX_LIST_PAGES {
        let page = list(cursor.clone()).await?;
        if let Some(items) = page.get(field).and_then(Value::as_array) {
            out.extend(items.iter().cloned());
        }
        let Some(next) = page.get("nextCursor").and_then(Value::as_str) else {
            return Ok(out);
        };
        if !seen.insert(next.to_string()) {
            return Err(RpcError::Other(format!(
                "MCP list returned duplicate cursor: {next}"
            )));
        }
        cursor = Some(next.to_string());
    }
    Err(RpcError::Other(format!(
        "MCP list exceeded {MAX_LIST_PAGES} pages"
    )))
}

fn cursor_params(cursor: Option<String>) -> Option<Value> {
    cursor.map(|c| json!({"cursor": c}))
}

async fn list(
    client: &McpClient,
    method: &str,
    field: &str,
    timeout: Option<u64>,
) -> Result<Vec<Value>, RpcError> {
    paginate(|c| client.request(method, cursor_params(c), timeout), field).await
}

/// Baseline `defs`: list tools (default timeout 30s); any failure yields `None`.
pub async fn defs(client: &McpClient, timeout: Option<u64>) -> Option<Vec<ToolDef>> {
    let timeout = Some(timeout.unwrap_or(opencode_integration_api::mcp::DEFAULT_TIMEOUT_MS));
    let listed = paginate(
        |c| async move {
            let page = client
                .request("tools/list", cursor_params(c), timeout)
                .await?;
            Ok(tolerant_page(page))
        },
        "tools",
    )
    .await;
    match listed {
        Ok(tools) => Some(
            tools
                .into_iter()
                .filter_map(|t| serde_json::from_value(t).ok())
                .collect(),
        ),
        Err(e) => {
            tracing::warn!(error = %e, "failed to list MCP tools");
            None
        }
    }
}

/// Baseline fallback (catalog.ts:145-168): when strict parsing of a `tools/list` page fails on an
/// output schema, the page is re-read with `TolerantListToolsResultSchema`, which omits
/// `outputSchema` from every tool on that page. Tools whose required fields are invalid are
/// dropped (the SDK would reject the page; the remaining tools stay usable).
fn tolerant_page(mut page: Value) -> Value {
    let Some(tools) = page.get_mut("tools").and_then(Value::as_array_mut) else {
        return page;
    };
    let bad = tools
        .iter()
        .any(|t| t.get("outputSchema").is_some_and(|s| !is_valid_schema(s)));
    if bad {
        tools
            .iter_mut()
            .filter_map(Value::as_object_mut)
            .for_each(|t| {
                t.remove("outputSchema");
            });
    }
    tools.retain(|t| {
        t.get("name").is_some_and(Value::is_string)
            && t.get("inputSchema").is_some_and(Value::is_object)
    });
    page
}

/// SDK ToolSchema requires `outputSchema.type == "object"`, and the validator must resolve every
/// local `$ref` (`#/...`).
fn is_valid_schema(schema: &Value) -> bool {
    schema.get("type").and_then(Value::as_str) == Some("object") && refs_resolve(schema, schema)
}

fn refs_resolve(root: &Value, node: &Value) -> bool {
    match node {
        Value::Object(m) => m.iter().all(|(k, v)| match (k.as_str(), v) {
            ("$ref", Value::String(r)) => r
                .strip_prefix('#')
                .is_some_and(|p| root.pointer(p).is_some()),
            _ => refs_resolve(root, v),
        }),
        Value::Array(a) => a.iter().all(|v| refs_resolve(root, v)),
        _ => true,
    }
}

pub async fn prompts(client: &McpClient, timeout: Option<u64>) -> Result<Vec<Value>, RpcError> {
    if !client.has_capability("prompts") {
        return Ok(Vec::new());
    }
    list(client, "prompts/list", "prompts", timeout).await
}

pub async fn resources(client: &McpClient, timeout: Option<u64>) -> Result<Vec<Value>, RpcError> {
    if !client.has_capability("resources") {
        return Ok(Vec::new());
    }
    list(client, "resources/list", "resources", timeout).await
}

pub async fn resource_templates(
    client: &McpClient,
    timeout: Option<u64>,
) -> Result<Vec<Value>, RpcError> {
    if !client.has_capability("resources") {
        return Ok(Vec::new());
    }
    list(
        client,
        "resources/templates/list",
        "resourceTemplates",
        timeout,
    )
    .await
}

/// Baseline `fetch` keying: `client:key` with escaped client for keyed lists, otherwise
/// `sanitize(client):sanitize(name)`. Each item gains `client`.
pub fn keyed(client: &str, items: Vec<Value>, key: Option<&str>) -> BTreeMap<String, Value> {
    let sanitized = sanitize_name(client);
    let escaped = client.replace('%', "%25").replace(':', "%3A");
    items
        .into_iter()
        .filter_map(|mut item| {
            let k = match key {
                Some(field) => format!("{escaped}:{}", item.get(field)?.as_str()?),
                None => format!("{sanitized}:{}", sanitize_name(item.get("name")?.as_str()?)),
            };
            item.as_object_mut()?.insert("client".into(), json!(client));
            Some((k, item))
        })
        .collect()
}

/// Translate a `tools/call` result like the baseline `convertTool.execute`: error results become
/// errors carrying their joined text, structured-only results gain a JSON text block.
pub fn translate_call_result(def: Option<&ToolDef>, result: Value) -> Result<Value, String> {
    let is_error = result
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let content = result
        .get("content")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let structured = result
        .get("structuredContent")
        .filter(|v| !v.is_null())
        .cloned();
    if let Some(def) = def
        && def.output_schema.is_some()
        && structured.is_none()
        && !is_error
    {
        return Err(format!(
            "MCP error -32600: Tool {} has an output schema but did not return structured content",
            def.name
        ));
    }
    if is_error {
        let text = content
            .iter()
            .filter(|c| c.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|c| c.get("text").and_then(Value::as_str))
            .filter(|t| !t.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");
        return Err(if text.is_empty() {
            "MCP tool returned an error".into()
        } else {
            text
        });
    }
    if !content.is_empty() {
        return Ok(result);
    }
    let Some(structured) = structured else {
        return Ok(result);
    };
    let mut out = result;
    out["content"] = json!([{"type": "text", "text": structured.to_string()}]);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_keys() {
        assert_eq!(tool_name("my server", "do.it"), "my_server_do_it");
        let k = keyed(
            "a:b%",
            vec![json!({"name": "r", "uri": "file:///x"})],
            Some("uri"),
        );
        assert!(k.contains_key("a%3Ab%25:file:///x"));
        assert_eq!(k["a%3Ab%25:file:///x"]["client"], "a:b%");
        let p = keyed("s.v", vec![json!({"name": "p q"})], None);
        assert!(p.contains_key("s_v:p_q"));
    }

    #[test]
    fn call_result_translation() {
        let err = translate_call_result(
            None,
            json!({"isError": true, "content": [
            {"type": "text", "text": "a"}, {"type": "text", "text": "  "}, {"type": "image"}, {"type": "text", "text": "b"}]}),
        );
        assert_eq!(err.unwrap_err(), "a\n\nb");
        let empty = translate_call_result(None, json!({"isError": true, "content": []}));
        assert_eq!(empty.unwrap_err(), "MCP tool returned an error");
        let s = translate_call_result(None, json!({"content": [], "structuredContent": {"x": 1}}))
            .unwrap();
        assert_eq!(s["content"][0]["text"], "{\"x\":1}");
        assert_eq!(s["structuredContent"]["x"], 1);
    }

    #[test]
    fn output_schema_requires_structured_content() {
        let def: ToolDef = serde_json::from_value(json!({"name": "t", "inputSchema": {"type": "object"}, "outputSchema": {"type": "object"}})).unwrap();
        let e = translate_call_result(
            Some(&def),
            json!({"content": [{"type": "text", "text": "x"}]}),
        );
        assert_eq!(
            e.unwrap_err(),
            "MCP error -32600: Tool t has an output schema but did not return structured content"
        );
        assert!(
            translate_call_result(Some(&def), json!({"isError": true, "content": []})).is_err()
        );
    }

    #[test]
    fn tolerant_output_schema() {
        let page = tolerant_page(json!({"tools": [
            {"name": "a", "inputSchema": {"type": "object"}, "outputSchema": {"type": "object"}},
            {"name": "b", "inputSchema": {"type": "object"}, "outputSchema": {"type": "object", "properties": {"x": {"$ref": "#/$defs/nope"}}}},
            {"inputSchema": {}}
        ]}));
        assert_eq!(page["tools"].as_array().unwrap().len(), 2);
        assert!(
            page["tools"][0].get("outputSchema").is_none(),
            "whole page loses outputSchema"
        );
        let ok = tolerant_page(
            json!({"tools": [{"name": "a", "inputSchema": {"type": "object"},
            "outputSchema": {"type": "object", "$defs": {"d": {}}, "properties": {"x": {"$ref": "#/$defs/d"}}}}]}),
        );
        assert!(ok["tools"][0].get("outputSchema").is_some());
    }
}
