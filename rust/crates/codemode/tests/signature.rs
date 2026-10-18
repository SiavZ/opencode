//! Purpose: port of packages/codemode/test/signature.test.ts (TypeScript signature rendering).
//! Scope: pretty/compact rendering, JSDoc tags, quoting, unions, catalogs and search signatures.
//! Uses:
//! rust/crates/
//! |-- codemode/src/lib.rs
//! `-- codemode/tests/common/mod.rs
//! External:
//! `-- serde_json::json
//! Used by:
//! `-- none, entrypoint
//! Invariants: tests drive the real renderer and runtime through the public API.
//! Contract tests: self

mod common;

use common::{runtime_value, tool};
use opencode_codemode::{
    Definition, Options, Runtime, Schema, ToolTree, input_type_script, json_schema_to_type_script,
    output_type_script,
};
use serde_json::{Value as Json, json};

fn list_issues() -> Definition {
    tool(
        "List issues in a repository",
        json!({
            "type": "object",
            "properties": {
                "owner": { "type": "string", "description": "Repository owner" },
                "after": { "type": "string", "description": "Cursor from the previous response's pageInfo" },
                "perPage": { "type": "number", "description": "Results per page", "default": 30 },
                "labels": { "type": "array", "items": { "type": "string" }, "description": "Filter by labels", "minItems": 1, "maxItems": 10 },
                "state": { "type": "string", "enum": ["open", "closed"] },
            },
            "required": ["owner"],
        }),
        None,
        |_| Ok(json!("[]")),
    )
}

fn lookup_order() -> Definition {
    tool(
        "Look up an order",
        Schema::object([
            ("id", Schema::string().describe("Order identifier")),
            ("verbose", Schema::optional(Schema::boolean())),
        ]),
        Some(
            Schema::object([("status", Schema::string().describe("Current order status"))]).into(),
        ),
        |_| Ok(json!({ "status": "open" })),
    )
}

#[test]
fn described_fields_get_jsdoc_comments() {
    assert_eq!(
        input_type_script(&list_issues(), true),
        [
            "{",
            "  /** Repository owner */",
            "  owner: string,",
            "  /** Cursor from the previous response's pageInfo */",
            "  after?: string,",
            "  /**",
            "   * Results per page",
            "   * @default 30",
            "   */",
            "  perPage?: number,",
            "  /**",
            "   * Filter by labels",
            "   * @minItems 1",
            "   * @maxItems 10",
            "   */",
            "  labels?: Array<string>,",
            "  state?: \"open\" | \"closed\",",
            "}",
        ]
        .join("\n")
    );
}

#[test]
fn compact_mode_output_is_unchanged() {
    assert_eq!(
        input_type_script(&list_issues(), false),
        r#"{ owner: string; after?: string; perPage?: number; labels?: Array<string>; state?: "open" | "closed" }"#
    );
    assert_eq!(
        input_type_script(&lookup_order(), false),
        "{ id: string; verbose?: boolean }"
    );
    assert_eq!(
        output_type_script(&lookup_order(), false),
        "{ status: string }"
    );
}

#[test]
fn nested_objects_recurse_with_indent() {
    let pretty = json_schema_to_type_script(
        &json!({
            "type": "object",
            "properties": {
                "filter": {
                    "type": "object",
                    "description": "Search filter",
                    "properties": { "state": { "type": "string", "description": "Issue state" } },
                },
            },
        }),
        true,
    );
    assert_eq!(
        pretty,
        [
            "{",
            "  /** Search filter */",
            "  filter?: {",
            "    /** Issue state */",
            "    state?: string,",
            "  },",
            "}"
        ]
        .join("\n")
    );
}

#[test]
fn schema_annotations_become_jsdoc() {
    assert_eq!(
        input_type_script(&lookup_order(), true),
        [
            "{",
            "  /** Order identifier */",
            "  id: string,",
            "  verbose?: boolean,",
            "}"
        ]
        .join("\n")
    );
    assert_eq!(
        output_type_script(&lookup_order(), true),
        [
            "{",
            "  /** Current order status */",
            "  status: string,",
            "}"
        ]
        .join("\n")
    );
}

#[test]
fn constraints_surface_as_jsdoc_tags() {
    let pretty = json_schema_to_type_script(
        &json!({
            "type": "object",
            "properties": {
                "legacy": { "type": "string", "deprecated": true },
                "homepage": { "type": "string", "format": "uri" },
                "tags": { "type": "array", "items": { "type": "string" }, "minItems": 2, "maxItems": 5, "default": ["a", "b"] },
            },
        }),
        true,
    );
    assert!(pretty.contains("  /** @deprecated */\n  legacy?: string"));
    assert!(pretty.contains("  /** @format uri */\n  homepage?: string"));
    assert!(
        pretty.contains(
            &[
                "  /**",
                "   * @default [\"a\",\"b\"]",
                "   * @minItems 2",
                "   * @maxItems 5",
                "   */",
                "  tags?: Array<string>"
            ]
            .join("\n")
        )
    );
}

/// The TS case passes a BigInt default, which JSON cannot hold; in Rust schemas are JSON values,
/// so an unserializable default is unrepresentable. The equivalent observable behavior (no broken
/// tag, plain field) is pinned for a schema without a usable default.
#[test]
fn skips_an_unserializable_default() {
    let pretty = json_schema_to_type_script(
        &json!({ "type": "object", "properties": { "size": { "type": "number" } } }),
        true,
    );
    assert_eq!(pretty, ["{", "  size?: number,", "}"].join("\n"));
}

#[test]
fn neutralizes_comment_terminators() {
    let pretty = json_schema_to_type_script(
        &json!({ "type": "object", "properties": { "note": { "type": "string", "description": "Ends */ early" } } }),
        true,
    );
    assert!(pretty.contains("  /** Ends * / early */"));
    assert!(!pretty.contains("Ends */"));
}

#[test]
fn multiline_descriptions_become_blocks() {
    let pretty = json_schema_to_type_script(
        &json!({ "type": "object", "properties": { "query": { "type": "string", "description": "\nFirst line\n\nSecond line\n" } } }),
        true,
    );
    assert_eq!(
        pretty,
        [
            "{",
            "  /**",
            "   * First line",
            "   *",
            "   * Second line",
            "   */",
            "  query?: string,",
            "}"
        ]
        .join("\n")
    );
}

#[test]
fn stays_total_on_cyclic_refs_and_deep_nesting() {
    let cyclic = json!({
        "$ref": "#/$defs/Node",
        "$defs": { "Node": { "type": "object", "properties": { "child": { "$ref": "#/$defs/Node" }, "name": { "type": "string" } } } },
    });
    assert_eq!(
        json_schema_to_type_script(&cyclic, false),
        "{ child?: unknown; name?: string }"
    );
    assert!(json_schema_to_type_script(&cyclic, true).contains("child?: unknown"));
    let mut deep = json!({ "type": "string" });
    for _ in 0..12 {
        deep = json!({ "type": "object", "properties": { "next": deep } });
    }
    for pretty in [false, true] {
        let rendered = json_schema_to_type_script(&deep, pretty);
        assert!(rendered.contains("unknown"));
        assert!(rendered.contains("next?:"));
    }
}

#[test]
fn intersects_ref_and_union_siblings() {
    assert_eq!(
        json_schema_to_type_script(
            &json!({
                "$ref": "#/$defs/User",
                "properties": { "active": { "type": "boolean" } },
                "required": ["active"],
                "$defs": { "User": { "type": "object", "properties": { "id": { "type": "string" } }, "required": ["id"] } },
            }),
            false
        ),
        "{ id: string } & { active: boolean }"
    );
    assert_eq!(
        json_schema_to_type_script(
            &json!({
                "type": "object",
                "properties": { "common": { "type": "boolean" } },
                "required": ["common"],
                "anyOf": [
                    { "type": "object", "properties": { "name": { "type": "string" } }, "required": ["name"] },
                    { "type": "object", "properties": { "count": { "type": "number" } }, "required": ["count"] },
                ],
            }),
            false
        ),
        "({ name: string } | { count: number }) & { common: boolean }"
    );
    assert_eq!(
        json_schema_to_type_script(&json!({ "$ref": "https://example.com/schema.json" }), false),
        "unknown"
    );
    assert_eq!(
        json_schema_to_type_script(
            &json!({ "$ref": "#/$defs/User/properties/id", "$defs": { "User": { "type": "object" }, "id": { "type": "string" } } }),
            false
        ),
        "unknown"
    );
    assert_eq!(
        json_schema_to_type_script(
            &json!({ "type": ["object", "null"], "properties": { "name": { "type": "string" } } }),
            false
        ),
        "{ name?: string } | null"
    );
}

fn raw_schema() -> Json {
    json!({
        "type": "object",
        "properties": {
            "foo-bar": { "type": "string" },
            "@type": { "type": "string" },
            "x.y": { "type": "number", "description": "Dotted name" },
            "123": { "type": "number" },
            "plain": { "type": "boolean" },
        },
        "required": ["@type"],
    })
}

#[test]
fn compact_rendering_quotes_non_identifier_keys() {
    assert_eq!(
        json_schema_to_type_script(&raw_schema(), false),
        r#"{ "123"?: number; "foo-bar"?: string; "@type": string; "x.y"?: number; plain?: boolean }"#
    );
}

#[test]
fn pretty_rendering_quotes_non_identifier_keys() {
    assert_eq!(
        json_schema_to_type_script(&raw_schema(), true),
        [
            "{",
            "  \"123\"?: number,",
            "  \"foo-bar\"?: string,",
            "  \"@type\": string,",
            "  /** Dotted name */",
            "  \"x.y\"?: number,",
            "  plain?: boolean,",
            "}",
        ]
        .join("\n")
    );
}

#[test]
fn json_schema_input_and_output_signatures_quote() {
    let definition = tool(
        "Adapter tool with awkward field names",
        raw_schema(),
        Some(json!({ "type": "object", "properties": { "content-type": { "type": "string" } }, "required": ["content-type"] }).into()),
        |_| Ok(json!({ "content-type": "text/plain" })),
    );
    assert!(input_type_script(&definition, false).contains(r#""foo-bar"?: string"#));
    assert_eq!(
        output_type_script(&definition, false),
        r#"{ "content-type": string }"#
    );
    assert_eq!(
        output_type_script(&definition, true),
        ["{", "  \"content-type\": string,", "}"].join("\n")
    );
}

#[test]
fn validating_schemas_with_non_identifier_fields_quote() {
    let definition = tool(
        "Schema tool with awkward field names",
        Schema::object([
            ("foo-bar", Schema::string()),
            ("plain", Schema::optional(Schema::number())),
        ]),
        None,
        |_| Ok(Json::Null),
    );
    assert_eq!(
        input_type_script(&definition, false),
        r#"{ "foo-bar": string; plain?: number }"#
    );
    assert_eq!(
        input_type_script(&definition, true),
        ["{", "  \"foo-bar\": string,", "  plain?: number,", "}"].join("\n")
    );
}

#[test]
fn any_of_with_number_branch_keeps_siblings() {
    let schema = json!({ "anyOf": [{ "type": "string" }, { "type": "number" }] });
    assert_eq!(
        json_schema_to_type_script(&schema, false),
        "string | number"
    );
    assert_eq!(json_schema_to_type_script(&schema, true), "string | number");
}

#[test]
fn nullable_numeric_unions_keep_null() {
    let schema = json!({ "oneOf": [{ "type": "number" }, { "type": "null" }] });
    assert_eq!(json_schema_to_type_script(&schema, false), "number | null");
    assert_eq!(json_schema_to_type_script(&schema, true), "number | null");
}

#[test]
fn tool_signatures_preserve_numeric_unions() {
    let definition = tool(
        "Tool with numeric unions",
        json!({ "type": "object", "properties": { "value": { "anyOf": [{ "type": "string" }, { "type": "number" }] } } }),
        Some(json!({ "anyOf": [{ "type": "number" }, { "type": "boolean" }] }).into()),
        |_| Ok(json!(1)),
    );
    assert_eq!(
        input_type_script(&definition, false),
        "{ value?: string | number }"
    );
    assert_eq!(output_type_script(&definition, false), "number | boolean");
}

#[test]
fn all_of_renders_intersections() {
    assert_eq!(
        json_schema_to_type_script(
            &json!({ "allOf": [{ "type": "object", "properties": { "id": { "type": "string" } } }, { "type": ["string", "null"] }] }),
            false
        ),
        "{ id?: string } & (string | null)"
    );
}

#[test]
fn all_of_does_not_discard_unresolved_constraints() {
    assert_eq!(
        json_schema_to_type_script(
            &json!({ "allOf": [{ "type": "string" }, { "$ref": "https://example.com/external.json" }] }),
            false
        ),
        "unknown"
    );
    assert_eq!(
        json_schema_to_type_script(
            &json!({ "allOf": [{ "type": "string" }, { "allOf": [{ "$ref": "https://example.com/external.json" }] }] }),
            false
        ),
        "unknown"
    );
    assert_eq!(
        json_schema_to_type_script(
            &json!({
                "type": "string",
                "allOf": [{ "$ref": "#/$defs/Constraint" }],
                "$defs": { "Constraint": { "description": "TypeScript-neutral constraint" } },
            }),
            false
        ),
        "string"
    );
}

fn catalog_runtime() -> Runtime {
    Runtime::new(Options {
        tools: ToolTree::new()
            .namespace("github", ToolTree::new().tool("list_issues", list_issues()))
            .namespace("orders", ToolTree::new().tool("lookup", lookup_order())),
        ..Options::default()
    })
    .expect("runtime")
}

async fn search(runtime: &Runtime, query: &str) -> Vec<Json> {
    let result = runtime_value(
        runtime,
        &format!(
            "return await tools.$codemode.search({{ query: {} }})",
            json!(query)
        ),
    )
    .await;
    assert!(result.ok, "{:?}", result.error);
    result.value.unwrap_or_default()["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

fn signature_at(items: &[Json], path: &str) -> String {
    items
        .iter()
        .find(|item| item["path"] == path)
        .and_then(|item| item["signature"].as_str())
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn json_schema_tool_search_signature_carries_jsdoc() {
    let runtime = catalog_runtime();
    let items = search(&runtime, "list issues repository").await;
    assert_eq!(
        signature_at(&items, "tools.github.list_issues"),
        [
            "tools.github.list_issues(input: {",
            "  /** Repository owner */",
            "  owner: string,",
            "  /** Cursor from the previous response's pageInfo */",
            "  after?: string,",
            "  /**",
            "   * Results per page",
            "   * @default 30",
            "   */",
            "  perPage?: number,",
            "  /**",
            "   * Filter by labels",
            "   * @minItems 1",
            "   * @maxItems 10",
            "   */",
            "  labels?: Array<string>,",
            "  state?: \"open\" | \"closed\",",
            "}): Promise<unknown>",
        ]
        .join("\n")
    );
}

#[tokio::test]
async fn annotated_schema_tool_search_signature_carries_jsdoc() {
    let runtime = catalog_runtime();
    for query in ["look up order", "tools.orders.lookup"] {
        let items = search(&runtime, query).await;
        assert_eq!(
            signature_at(&items, "tools.orders.lookup"),
            [
                "tools.orders.lookup(input: {",
                "  /** Order identifier */",
                "  id: string,",
                "  verbose?: boolean,",
                "}): Promise<{",
                "  /** Current order status */",
                "  status: string,",
                "}>",
            ]
            .join("\n")
        );
    }
}

#[tokio::test]
async fn inline_catalog_uses_the_same_signatures() {
    let runtime = catalog_runtime();
    let github = signature_at(
        &search(&runtime, "list issues repository").await,
        "tools.github.list_issues",
    );
    let orders = signature_at(
        &search(&runtime, "look up order").await,
        "tools.orders.lookup",
    );
    let instructions = runtime.instructions();
    assert!(instructions.contains(&format!("  - {github} // List issues in a repository")));
    assert!(instructions.contains(&format!("  - {orders} // Look up an order")));
    assert!(instructions.contains("/** Repository owner */"));
}

fn resolve_runtime() -> Runtime {
    let resolve = tool(
        "Resolve a Context7 library ID",
        json!({
            "type": "object",
            "properties": { "query": { "type": "string" }, "libraryName": { "type": "string" } },
            "required": ["query", "libraryName"],
        }),
        None,
        |_| Ok(json!("/reactjs/react.dev")),
    );
    Runtime::new(Options {
        tools: ToolTree::new().namespace(
            "context7",
            ToolTree::new().tool("resolve-library-id", resolve),
        ),
        ..Options::default()
    })
    .expect("runtime")
}

#[test]
fn inline_catalog_uses_bracket_notation() {
    let runtime = resolve_runtime();
    let instructions = runtime.instructions();
    assert!(instructions.contains(
        "tools.context7[\"resolve-library-id\"](input: {\n  query: string,\n  libraryName: string,\n}): Promise<unknown>"
    ));
    assert!(instructions.contains("Do not infer or normalize tool names"));
    assert!(instructions.contains("bracket notation and quotes are part of the path"));
    assert!(!instructions.contains("tools.context7.resolve-library-id"));
    assert!(!instructions.contains("tools.context7.resolve_library_id"));
}

#[tokio::test]
async fn search_returns_bracket_notation_paths() {
    let items = search(&resolve_runtime(), "resolve library").await;
    assert_eq!(
        items[0]["path"],
        json!("tools.context7[\"resolve-library-id\"]")
    );
    assert!(
        items[0]["signature"]
            .as_str()
            .unwrap_or_default()
            .contains("tools.context7[\"resolve-library-id\"](input: {")
    );
}
