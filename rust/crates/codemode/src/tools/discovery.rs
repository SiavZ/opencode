//! Purpose: progressive tool discovery: catalog descriptions, budgeted agent instructions, and
//! the always-registered `$codemode.search` tool.
//! Scope: `tool-runtime.ts` catalog/prepare/search (lines 237-597).
//! Uses:
//! rust/crates/
//! |-- codemode/src/tools/mod.rs
//! |-- codemode/src/tools/render.rs
//! |-- codemode/src/tools/schema.rs
//! |-- codemode/src/value/number.rs
//! `-- codemode/src/value/text.rs
//! External:
//! |-- serde_json::Value
//! |-- serde_json::json
//! `-- str::to_string
//! Used by:
//! rust/crates/
//! |-- codemode/src/run.rs
//! |-- codemode/src/tools/mod.rs
//! `-- codemode/src/tools/runtime.rs
//! Invariants: namespace stub lines are never budgeted; a COMPLETE catalog never advertises search.
//! Contract tests: codemode/tests/codemode.rs, codemode/tests/signature.rs

use super::render::{input_properties, input_type_script, is_identifier, output_type_script};
use super::schema::Schema;
use super::{Definition, ToolDescription, ToolNode, ToolTree};
use crate::value::text::{len16, locale_compare};
use serde_json::{Value as Json, json};
use std::sync::Arc;

pub(crate) const RESERVED_NAMESPACE: &str = "$codemode";
pub(crate) const DEFAULT_CATALOG_BUDGET: usize = 2_000;
const DEFAULT_SEARCH_LIMIT: usize = 10;

#[derive(Clone, Debug)]
pub(crate) struct SearchEntry {
    pub description: ToolDescription,
    pub namespace: String,
    pub search_text: String,
}

pub(crate) fn tool_expression(path: &str) -> String {
    let mut out = String::from("tools");
    for segment in path.split('.') {
        if is_identifier(segment) {
            out.push('.');
            out.push_str(segment);
        } else {
            out.push('[');
            out.push_str(&crate::value::text::quote_json(segment));
            out.push(']');
        }
    }
    out
}

fn definitions(tree: &ToolTree, path: &[String], out: &mut Vec<(String, Definition)>) {
    for (name, node) in &tree.0 {
        let mut next = path.to_vec();
        next.push(name.clone());
        match node {
            ToolNode::Tool(definition) => out.push((next.join("."), definition.clone())),
            ToolNode::Namespace(child) => definitions(child, &next, out),
            ToolNode::Function(_) => {}
        }
    }
}

fn describe(path: &str, definition: &Definition) -> ToolDescription {
    ToolDescription {
        path: path.to_string(),
        description: definition.description.clone(),
        signature: format!(
            "{}(input: {}): Promise<{}>",
            tool_expression(path),
            input_type_script(definition, true),
            output_type_script(definition, true)
        ),
    }
}

fn visible(tree: &ToolTree) -> Vec<(String, Definition, ToolDescription)> {
    let mut found = Vec::new();
    definitions(tree, &[], &mut found);
    found
        .into_iter()
        .map(|(path, definition)| {
            let description = describe(&path, &definition);
            (path, definition, description)
        })
        .collect()
}

fn search_entry(path: &str, definition: &Definition, description: ToolDescription) -> SearchEntry {
    let mut parts = vec![path.to_string(), definition.description.clone()];
    for (name, property) in input_properties(definition) {
        parts.push(name);
        if let Some(property) = property {
            parts.push(property);
        }
    }
    SearchEntry {
        description,
        namespace: path.split('.').next().unwrap_or_default().to_string(),
        search_text: parts.join("\n").to_lowercase(),
    }
}

pub(crate) fn search_index(tree: &ToolTree) -> Vec<SearchEntry> {
    visible(tree)
        .into_iter()
        .map(|(path, definition, description)| search_entry(&path, &definition, description))
        .collect()
}

fn search_input() -> Schema {
    Schema::object([
        ("query", Schema::optional(Schema::string())),
        ("namespace", Schema::optional(Schema::string())),
        ("limit", Schema::optional(Schema::positive_int())),
        ("offset", Schema::optional(Schema::non_negative_int())),
    ])
}

fn search_output() -> Schema {
    Schema::object([
        (
            "items",
            Schema::array(Schema::object([
                ("path", Schema::string()),
                ("description", Schema::string()),
                ("signature", Schema::string()),
            ])),
        ),
        ("remaining", Schema::non_negative_int()),
        (
            "next",
            Schema::null_or(Schema::object([("offset", Schema::non_negative_int())])),
        ),
    ])
}

/// Splits a query into lowercased terms (camelCase boundaries and non-alphanumerics separate).
fn tokenize(query: &str) -> Vec<String> {
    let mut spaced = String::new();
    let mut previous: Option<char> = None;
    for ch in query.chars() {
        if let Some(prev) = previous
            && (prev.is_ascii_lowercase() || prev.is_ascii_digit())
            && ch.is_ascii_uppercase()
        {
            spaced.push(' ');
        }
        spaced.push(ch);
        previous = Some(ch);
    }
    spaced
        .to_lowercase()
        .split(|ch: char| !(ch.is_ascii_lowercase() || ch.is_ascii_digit()))
        .filter(|term| !term.is_empty() && *term != "*")
        .map(str::to_string)
        .collect()
}

fn term_forms(term: String) -> Vec<String> {
    let mut forms = vec![term.clone()];
    if term.ends_with("es") && term.len() > 3 {
        forms.push(term[..term.len() - 2].to_string());
    }
    if term.ends_with('s') && term.len() > 2 {
        forms.push(term[..term.len() - 1].to_string());
    }
    forms
}

fn run_search(index: &[SearchEntry], input: &Json) -> Json {
    let query = input.get("query").and_then(Json::as_str).unwrap_or("");
    let offset = input.get("offset").and_then(Json::as_f64).unwrap_or(0.0) as usize;
    let limit = input
        .get("limit")
        .and_then(Json::as_f64)
        .map_or(DEFAULT_SEARCH_LIMIT, |value| value as usize);
    let namespace = input.get("namespace").and_then(Json::as_str);
    let scoped: Vec<&SearchEntry> = index
        .iter()
        .filter(|entry| namespace.is_none_or(|name| entry.namespace == name))
        .collect();
    let trimmed = query.trim();
    let path_query = trimmed.strip_prefix("tools.").unwrap_or(trimmed);
    let exact = (!path_query.is_empty())
        .then(|| {
            scoped.iter().find(|entry| {
                entry.description.path == path_query
                    || tool_expression(&entry.description.path) == trimmed
            })
        })
        .flatten();
    let terms: Vec<Vec<String>> = tokenize(query).into_iter().map(term_forms).collect();
    let ranked: Vec<&SearchEntry> = match exact {
        Some(entry) => vec![*entry],
        None => {
            let mut scored: Vec<(&SearchEntry, u32)> = scoped
                .iter()
                .map(|entry| {
                    let path = entry.description.path.to_lowercase();
                    let description = entry.description.description.to_lowercase();
                    let score = terms
                        .iter()
                        .map(|forms| {
                            let any = |test: &dyn Fn(&String) -> bool| forms.iter().any(test);
                            (if any(&|form| path == *form || path.ends_with(&format!(".{form}"))) {
                                20
                            } else {
                                0
                            }) + (if any(&|form| path.contains(form.as_str())) {
                                8
                            } else {
                                0
                            }) + (if any(&|form| description.contains(form.as_str())) {
                                4
                            } else {
                                0
                            }) + (if any(&|form| entry.search_text.contains(form.as_str())) {
                                2
                            } else {
                                0
                            })
                        })
                        .sum();
                    (*entry, score)
                })
                .filter(|(_, score)| terms.is_empty() || *score > 0)
                .collect();
            scored.sort_by(|left, right| {
                right.1.cmp(&left.1).then_with(|| {
                    locale_compare(&left.0.description.path, &right.0.description.path)
                })
            });
            scored.into_iter().map(|(entry, _)| entry).collect()
        }
    };
    let items: Vec<Json> = ranked
        .iter()
        .skip(offset)
        .take(limit)
        .map(|entry| {
            json!({
                "path": tool_expression(&entry.description.path),
                "description": entry.description.description,
                "signature": entry.description.signature,
            })
        })
        .collect();
    let remaining = ranked.len().saturating_sub(offset + items.len());
    json!({
        "items": items,
        "remaining": remaining,
        "next": if remaining > 0 { json!({ "offset": offset + items.len() }) } else { Json::Null },
    })
}

pub(crate) fn search_tool(index: Arc<Vec<SearchEntry>>) -> Definition {
    Definition::new(
        "Search available Code Mode tools",
        search_input(),
        Some(search_output().into()),
        move |input: Json, _context: super::ToolCallContext| -> super::ToolFuture {
            let result = run_search(&index, &input);
            Box::pin(async move { Ok(result) })
        },
    )
}

fn estimate_tokens(text: &str) -> usize {
    crate::value::number::js_round(len16(text) as f64 / 4.0).max(0.0) as usize
}

fn catalog_line(tool: &ToolDescription) -> String {
    let line = tool.description.split('\n').next().unwrap_or("").trim();
    let description = if len16(line) > 120 {
        format!("{}...", crate::value::text::slice16(line, 0, 119))
    } else {
        line.to_string()
    };
    if description.is_empty() {
        format!("  - {}", tool.signature)
    } else {
        format!("  - {} // {description}", tool.signature)
    }
}

pub(crate) struct DiscoveryPlan {
    pub catalog: Vec<ToolDescription>,
    pub instructions: String,
    pub search_index: Vec<SearchEntry>,
}

/// Budgeted, round-robin catalog plus the agent-facing instructions (`prepare`).
pub(crate) fn prepare(tree: &ToolTree, catalog_budget: usize) -> DiscoveryPlan {
    let visible = visible(tree);
    let described: Vec<ToolDescription> = visible
        .iter()
        .map(|(_, _, description)| description.clone())
        .collect();
    let mut namespaces: Vec<(String, Vec<ToolDescription>)> = Vec::new();
    for tool in &described {
        let namespace = tool
            .path
            .split('.')
            .next()
            .unwrap_or(&tool.path)
            .to_string();
        match namespaces.iter_mut().find(|(name, _)| *name == namespace) {
            Some((_, group)) => group.push(tool.clone()),
            None => namespaces.push((namespace, vec![tool.clone()])),
        }
    }
    namespaces.sort_by(|left, right| locale_compare(&left.0, &right.0));
    let mut queues: Vec<std::collections::VecDeque<ToolDescription>> = namespaces
        .iter()
        .map(|(_, group)| {
            let mut queue = group.clone();
            queue.sort_by(|left, right| {
                estimate_tokens(&catalog_line(left))
                    .cmp(&estimate_tokens(&catalog_line(right)))
                    .then_with(|| locale_compare(&left.path, &right.path))
            });
            queue.into()
        })
        .collect();
    let mut picked: Vec<Vec<String>> = vec![Vec::new(); namespaces.len()];
    let mut used = 0;
    let mut active: Vec<usize> = (0..namespaces.len())
        .filter(|index| !queues[*index].is_empty())
        .collect();
    while !active.is_empty() {
        let mut still = Vec::new();
        for index in active {
            let Some(cost) = queues[index]
                .front()
                .map(|tool| estimate_tokens(&catalog_line(tool)))
            else {
                continue;
            };
            if used + cost > catalog_budget {
                continue;
            }
            let Some(tool) = queues[index].pop_front() else {
                continue;
            };
            picked[index].push(tool.path);
            used += cost;
            if !queues[index].is_empty() {
                still.push(index);
            }
        }
        active = still;
    }
    let total_shown: usize = picked.iter().map(Vec::len).sum();
    let complete = total_shown == described.len();
    let empty = described.is_empty();
    let lines = instruction_lines(
        empty,
        complete,
        total_shown,
        described.len(),
        &namespaces,
        &picked,
    );
    DiscoveryPlan {
        catalog: described,
        instructions: lines.join("\n"),
        search_index: visible
            .into_iter()
            .map(|(path, definition, description)| search_entry(&path, &definition, description))
            .collect(),
    }
}

fn instruction_lines(
    empty: bool,
    complete: bool,
    total_shown: usize,
    total: usize,
    namespaces: &[(String, Vec<ToolDescription>)],
    picked: &[Vec<String>],
) -> Vec<String> {
    let base = "This is a restricted JavaScript language for calling tools, not a general-purpose runtime.";
    let mut lines: Vec<String> = Vec::new();
    lines.push(if empty {
        base.to_string()
    } else if complete {
        format!("{base} Inside the confined interpreter, `tools` contains the Code Mode tools listed below and internal runtime tools; surrounding agent tools are not available.")
    } else {
        format!("{base} Inside the confined interpreter, `tools` contains the Code Mode tools listed or searchable below and internal runtime tools; surrounding agent tools are not available.")
    });
    if !empty {
        lines.push("Do not infer or normalize tool names; use only exact signatures shown below or returned by search.".into());
        lines.extend(["", "## Workflow", ""].map(String::from));
        if complete {
            lines.push("1. Pick a tool from the list under `## Available tools` - each line is the exact call signature; use it as-is rather than guessing segments.".into());
            lines.push("2. Call it using the exact signature shown: `const result = await tools.<namespace>.<tool>(input)`; bracket notation and quotes are part of the path.".into());
            lines.push("3. Return only the fields you need from structured results; narrow unknown results before reading fields, and avoid returning large raw payloads.".into());
        } else {
            lines.push("1. If needed, discover tools: `return await tools.$codemode.search({ query: \"<intent + key nouns>\" })`.".into());
            lines.push("2. In the next execution, copy a returned path exactly, call it, and return only the needed fields.".into());
        }
        lines.extend(["", "## Rules", ""].map(String::from));
        lines.push(if complete {
            "- Only Code Mode tools listed here and internal runtime tools are available; surrounding agent tools are not implicitly exposed.".into()
        } else {
            "- Only Code Mode tools listed here or returned by `tools.$codemode.search` and internal runtime tools are available; surrounding agent tools are not implicitly exposed.".into()
        });
        lines.push("- Filter, aggregate, and transform collections in code - never return them raw or call a tool per item across messages.".into());
        lines.push("- A result typed `Promise<unknown>` may be structured data or text. Before reading fields, check that it is a non-null object and not an array; otherwise handle the returned text or primitive directly.".into());
        lines.push("- Run independent calls in parallel: `await Promise.all(items.map((item) => tools.<namespace>.<tool>(item)))`, or use `tools.<namespace>[\"tool-name\"](item)` when the listed signature uses bracket notation.".into());
        lines.push("- `Object.keys(tools)` lists namespaces; `Object.keys(tools.<namespace>)` lists its tools; `for...in` works on both.".into());
        if !complete {
            lines.push("- Browse one namespace: `await tools.$codemode.search({ query: \"\", namespace: \"<name>\" })`.".into());
            lines.push(
                "- If search returns `next`, repeat the same search with `offset: next.offset`."
                    .into(),
            );
        }
    }
    lines.extend(["", "## Language", ""].map(String::from));
    lines.push("Use common JavaScript data operations, functions, control flow, selected standard-library methods, and awaited tool calls. Built-ins include Date, RegExp, Map, Set, URL, URLSearchParams, and URI encoding helpers.".into());
    lines.push("Modules/imports, classes, generators, timers, fetch, eval, prototype access, unlisted methods, and promise chaining are unavailable. Use Code Mode tools for external operations. Use await with try/catch.".into());
    lines.push("Dates and URLs serialize to strings at data boundaries; Map/Set/RegExp/URLSearchParams serialize to `{}`.".into());
    lines.push(String::new());
    if empty {
        lines.extend(
            [
                "## Available tools",
                "",
                "No tools are currently available.",
            ]
            .map(String::from),
        );
        return lines;
    }
    lines.push(if complete {
        "## Available tools (COMPLETE list - every tool is shown below with its full call signature)".into()
    } else {
        format!("## Available tools (PARTIAL - {total_shown} of {total} shown; find the rest with tools.$codemode.search)")
    });
    lines.push(String::new());
    for ((namespace, group), shown) in namespaces.iter().zip(picked) {
        let count = format!(
            "{} tool{}",
            group.len(),
            if group.len() == 1 { "" } else { "s" }
        );
        let label = if shown.len() == group.len() {
            count
        } else if shown.is_empty() {
            format!("{count}, none shown")
        } else {
            format!("{count}, {} shown", shown.len())
        };
        lines.push(format!("- {namespace} ({label})"));
        for tool in group.iter().filter(|tool| shown.contains(&tool.path)) {
            lines.push(catalog_line(tool));
        }
    }
    if !complete {
        let signature = describe(
            &format!("{RESERVED_NAMESPACE}.search"),
            &search_tool(Arc::new(Vec::new())),
        )
        .signature;
        lines.extend([
            "".to_string(),
            "Search returns complete callable signatures:".to_string(),
            format!("- {signature}"),
        ]);
    }
    lines
}
