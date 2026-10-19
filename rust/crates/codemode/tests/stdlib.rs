//! Purpose: port of packages/codemode/test/stdlib.test.ts (Date, RegExp, URL, Map, Set, checkpoints).
//! Scope: one Rust test per TS `test(...)`, same programs and expected values.
//! Uses:
//! rust/crates/
//! |-- codemode/src/lib.rs
//! `-- codemode/tests/common/mod.rs
//! External:
//! |-- serde_json::Value
//! |-- serde_json::json
//! `-- tokio::test
//! Used by:
//! `-- none, entrypoint
//! Invariants: tests drive the real interpreter through the public API.
//! Contract tests: self

mod common;

use common::{error, run, run_tools, tool, value};
use opencode_codemode::{DiagnosticKind, Schema, ToolTree};
use serde_json::{Value as Json, json};
use std::sync::{Arc, Mutex};

// Date
#[tokio::test]
async fn date_now_returns_a_number() {
    assert_eq!(value("return typeof Date.now()").await, json!("number"));
}

#[tokio::test]
async fn epoch_construction_and_iso_rendering() {
    assert_eq!(
        value("return new Date(0).toISOString()").await,
        json!("1970-01-01T00:00:00.000Z")
    );
}

#[tokio::test]
async fn string_parsing_round_trips() {
    assert_eq!(
        value(r#"return new Date("2024-01-02T03:04:05.000Z").getTime()"#).await,
        json!(1_704_164_645_000u64)
    );
    assert_eq!(
        value(r#"return Date.parse("2024-01-02T03:04:05.000Z")"#).await,
        json!(1_704_164_645_000u64)
    );
}

#[tokio::test]
async fn date_arithmetic_and_comparison_use_the_time_value() {
    assert_eq!(
        value("const a = new Date(1000); const b = new Date(3000); return b - a").await,
        json!(2000)
    );
    assert_eq!(
        value("const a = new Date(1000); const b = new Date(3000); return a < b").await,
        json!(true)
    );
    assert_eq!(value("return +new Date(42)").await, json!(42));
}

#[tokio::test]
async fn utc_getters_read_calendar_components() {
    assert_eq!(
        value(r#"const d = new Date("2024-03-05T06:07:08.009Z"); return [d.getUTCFullYear(), d.getUTCMonth(), d.getUTCDate(), d.getUTCHours(), d.getUTCMinutes(), d.getUTCSeconds(), d.getUTCMilliseconds()]"#).await,
        json!([2024, 2, 5, 6, 7, 8, 9])
    );
}

#[tokio::test]
async fn invalid_dates_yield_nan_times() {
    assert_eq!(
        value(r#"return Number.isNaN(new Date("garbage").getTime())"#).await,
        json!(true)
    );
    assert_eq!(
        value(r#"return new Date("garbage").toJSON()"#).await,
        Json::Null
    );
}

#[tokio::test]
async fn to_iso_string_on_an_invalid_date_is_catchable() {
    assert_eq!(
        value(
            r#"try { new Date("garbage").toISOString(); return "no" } catch { return "caught" }"#
        )
        .await,
        json!("caught")
    );
}

#[tokio::test]
async fn template_interpolation_renders_the_iso_form() {
    assert_eq!(
        value("return `at ${new Date(0)}`").await,
        json!("at 1970-01-01T00:00:00.000Z")
    );
}

#[tokio::test]
async fn dates_serialize_to_iso_strings_at_the_boundary() {
    assert_eq!(
        value("return new Date(0)").await,
        json!("1970-01-01T00:00:00.000Z")
    );
    assert_eq!(
        value("return { when: new Date(0), tags: [new Date(1000)] }").await,
        json!({ "when": "1970-01-01T00:00:00.000Z", "tags": ["1970-01-01T00:00:01.000Z"] })
    );
    assert_eq!(
        value("return JSON.stringify({ d: new Date(0) })").await,
        json!(r#"{"d":"1970-01-01T00:00:00.000Z"}"#)
    );
}

#[tokio::test]
async fn date_coercions() {
    assert_eq!(value("return Number(new Date(5))").await, json!(5));
    assert_eq!(
        value("return String(new Date(0))").await,
        json!("1970-01-01T00:00:00.000Z")
    );
    assert_eq!(value("return Boolean(new Date(0))").await, json!(true));
}

#[tokio::test]
async fn sorting_dates_with_a_numeric_comparator() {
    assert_eq!(
        value(
            r#"
      const dates = [new Date(3000), new Date(1000), new Date(2000)]
      return dates.sort((a, b) => a - b).map((d) => d.getTime())
    "#
        )
        .await,
        json!([1000, 2000, 3000])
    );
}

#[tokio::test]
async fn new_date_component_form() {
    assert_eq!(
        value(
            "const d = new Date(2024, 0, 2); return [d.getFullYear(), d.getMonth(), d.getDate()]"
        )
        .await,
        json!([2024, 0, 2])
    );
}

#[tokio::test]
async fn date_typeof_and_unknown_properties_are_forgiving() {
    assert_eq!(value("return typeof new Date(0)").await, json!("object"));
    assert_eq!(
        value("return new Date(0).nope === undefined").await,
        json!(true)
    );
}

// RegExp
#[tokio::test]
async fn regex_literal_test() {
    assert_eq!(value(r#"return /ab+c/.test("xabbbc")"#).await, json!(true));
    assert_eq!(value(r#"return /ab+c/.test("nope")"#).await, json!(false));
}

#[tokio::test]
async fn exec_exposes_captures_and_index() {
    assert_eq!(
        value(r#"const m = /a(b+)/.exec("xxabbc"); return { full: m[0], group: m[1], index: m.index }"#).await,
        json!({ "full": "abb", "group": "bb", "index": 2 })
    );
    assert_eq!(value(r#"return /a/.exec("zzz")"#).await, Json::Null);
}

#[tokio::test]
async fn named_groups_read_through() {
    assert_eq!(
        value(r#"const m = /(?<word>[a-z]+)-(?<num>\d+)/.exec("id ab-42"); return m.groups.word + m.groups.num"#).await,
        json!("ab42")
    );
}

#[tokio::test]
async fn global_exec_advances_last_index() {
    assert_eq!(
        value(
            r#"
      const r = /\d+/g
      const first = r.exec("a1b22c")
      const second = r.exec("a1b22c")
      return [first[0], second[0]]
    "#
        )
        .await,
        json!(["1", "22"])
    );
}

#[tokio::test]
async fn string_match_non_global_and_global() {
    assert_eq!(
        value(r#"const m = "a1b22".match(/\d+/); return [m[0], m.index]"#).await,
        json!(["1", 1])
    );
    assert_eq!(
        value(r#"return "a1b22".match(/\d+/g)"#).await,
        json!(["1", "22"])
    );
    assert_eq!(value(r#"return "abc".match(/\d/)"#).await, Json::Null);
}

#[tokio::test]
async fn match_all_materializes_match_arrays() {
    assert_eq!(
        value(r#"return "a1b22".matchAll(/(\d+)/g).map((m) => m[1])"#).await,
        json!(["1", "22"])
    );
}

#[tokio::test]
async fn replace_and_replace_all_with_patterns() {
    assert_eq!(
        value(r##"return "a1b2".replace(/\d/, "#")"##).await,
        json!("a#b2")
    );
    assert_eq!(
        value(r##"return "a1b2".replace(/\d/g, "#")"##).await,
        json!("a#b#")
    );
    assert_eq!(
        value(r##"return "a1b2".replaceAll(/\d/g, "#")"##).await,
        json!("a#b#")
    );
    assert_eq!(
        value(r#"return "hi bob".replace(/b(o)b/, "[$1]")"#).await,
        json!("hi [o]")
    );
}

#[tokio::test]
async fn function_replacers_receive_captures_offsets_input_and_groups() {
    assert_eq!(
        value(
            r#"
        const seen = []
        const output = "a1b22".replace(/(\d)(\d)?/g, (match, first, second, offset, input) => {
          seen.push([match, first, second === undefined, offset, input])
          return Number(match) * 2
        })
        return { output, seen }
      "#
        )
        .await,
        json!({ "output": "a2b44", "seen": [["1", "1", true, 1, "a1b22"], ["22", "2", false, 3, "a1b22"]] })
    );
    assert_eq!(
        value(
            r#"
        return "red-blue".replace(
          /(?<left>[a-z]+)-(?<right>[a-z]+)/,
          (match, left, right, offset, input, groups) => groups.right + ":" + groups.left,
        )
      "#
        )
        .await,
        json!("blue:red")
    );
}

#[tokio::test]
async fn function_replacers_with_strings_zero_length_and_coercion() {
    assert_eq!(
        value(r#"return "banana".replace("na", (match, offset, input) => "[" + offset + "]")"#)
            .await,
        json!("ba[2]na")
    );
    assert_eq!(
        value(r#"return "ab".replaceAll("", (match, offset) => offset)"#).await,
        json!("0a1b2")
    );
    assert_eq!(
        value(r#"return "😀".replaceAll(/(?:)/gu, (match, offset) => "[" + offset + "]")"#).await,
        json!("[0]😀[2]")
    );
    assert_eq!(
        value(r#"return "123".replace(/\d/g, (match) => match === "1" ? 7 : match === "2" ? null : { n: 3 })"#).await,
        json!("7null[object Object]")
    );
}

#[tokio::test]
async fn function_replacers_can_await_tool_calls() {
    let decorate = || {
        ToolTree::new().namespace(
            "host",
            ToolTree::new().tool(
                "decorate",
                tool(
                    "Decorate a string",
                    Schema::string(),
                    Some(Schema::string().into()),
                    |input| Ok(json!(format!("[{}]", input.as_str().unwrap_or_default()))),
                ),
            ),
        )
    };
    let result = run_tools(
        r#"return "a1b22".replace(/\d+/g, async (match) => await tools.host.decorate(match))"#,
        decorate(),
    )
    .await;
    assert_eq!(result.value, Some(json!("a[1]b[22]")));
    let missing = run_tools(
        r#"return "a1".replace(/\d/, (match) => tools.host.decorate(match))"#,
        decorate(),
    )
    .await;
    let error = missing.error.expect("diagnostic");
    assert_eq!(error.kind, DiagnosticKind::InvalidDataValue);
    assert!(error.message.contains("un-awaited Promise"));
}

#[tokio::test]
async fn replace_all_without_g_is_catchable() {
    assert_eq!(
        value(r#"try { "a".replaceAll(/a/, "b"); return "no" } catch { return "caught" }"#).await,
        json!("caught")
    );
}

#[tokio::test]
async fn split_and_search_accept_patterns() {
    assert_eq!(
        value(r#"return "a1b22c".split(/\d+/)"#).await,
        json!(["a", "b", "c"])
    );
    assert_eq!(value(r#"return "ab42".search(/\d/)"#).await, json!(2));
    assert_eq!(value(r#"return "ab".search(/\d/)"#).await, json!(-1));
}

#[tokio::test]
async fn new_regexp_from_strings_and_invalid_patterns_are_catchable() {
    assert_eq!(
        value(r#"return new RegExp("a+", "i").test("AAA")"#).await,
        json!(true)
    );
    assert_eq!(
        value(r#"try { new RegExp("("); return "no" } catch { return "caught" }"#).await,
        json!("caught")
    );
    assert_eq!(
        value("return [/a/ instanceof RegExp, /a/.source]").await,
        json!([true, "a"])
    );
}

#[tokio::test]
async fn invalid_patterns_fail_with_actionable_messages() {
    let from_string = error(r#"return "abc".match("(")"#).await;
    assert!(
        from_string
            .message
            .contains(r#"String.match received the string "(""#)
    );
    assert!(from_string.message.contains("escape them with a backslash"));
    let from_constructor = error(r#"return new RegExp("(")"#).await;
    assert!(
        from_constructor
            .message
            .contains(r#"new RegExp(...) received "(""#)
    );
    assert!(
        from_constructor
            .message
            .contains("escape them with a backslash")
    );
    let from_flags = error(r#"return new RegExp("a", "xz")"#).await;
    assert!(
        from_flags.message.contains(r#"invalid flags "xz""#),
        "{}",
        from_flags.message
    );
    assert!(from_flags.message.contains("Valid flags are"));
}

#[tokio::test]
async fn missing_g_flag_errors_say_how_to_fix() {
    assert!(
        error(r#"return "aa".replaceAll(/a/, "b")"#)
            .await
            .message
            .contains("write /a/g, or use String.replace")
    );
    assert!(
        error(r#"return "aa".matchAll(/a/)"#)
            .await
            .message
            .contains("write /a/g, or use String.match")
    );
}

#[tokio::test]
async fn a_non_pattern_argument_names_the_expected_shapes() {
    let err = error(r#"return "abc".match(42)"#).await;
    assert!(err.message.contains("expects a regular expression"));
    assert!(err.message.contains("not number"));
}

#[tokio::test]
async fn source_and_flags_read_through() {
    assert_eq!(
        value("const r = /ab/gi; return { source: r.source, flags: r.flags, global: r.global }")
            .await,
        json!({ "source": "ab", "flags": "gi", "global": true })
    );
}

#[tokio::test]
async fn regexes_serialize_to_empty_objects() {
    assert_eq!(value("return /a/").await, json!({}));
    assert_eq!(
        value("return JSON.stringify({ r: /a/g })").await,
        json!(r#"{"r":{}}"#)
    );
}

#[tokio::test]
async fn regex_template_interpolation_renders_literal_form() {
    assert_eq!(value("return `${/ab/g}`").await, json!("/ab/g"));
}

// URL and URI helpers
#[tokio::test]
async fn encodes_and_decodes_uris() {
    assert_eq!(
        value(
            r#"
        return [
          encodeURI("https://example.test/a b?q=a/b"),
          encodeURIComponent("a b/c?"),
          decodeURI("https://example.test/a%20b?q=a/b"),
          decodeURIComponent("a%20b%2Fc%3F"),
          ["a b", "c/d"].map(encodeURIComponent),
        ]
      "#
        )
        .await,
        json!([
            "https://example.test/a%20b?q=a/b",
            "a%20b%2Fc%3F",
            "https://example.test/a b?q=a/b",
            "a b/c?",
            ["a%20b", "c%2Fd"]
        ])
    );
    assert_eq!(
        value(r#"try { decodeURIComponent("%zz"); return false } catch (error) { return error instanceof URIError }"#).await,
        json!(true)
    );
}

#[tokio::test]
async fn resolves_and_mutates_urls_with_linked_search_params() {
    let href = "https://user:pass@example.com:8443/items/a%20b?id=a+b&tag=x%2Fy#part%201";
    assert_eq!(
        value(
            r#"
        const url = new URL("../users?id=old#top", "https://user:pass@example.com:8443/api/v1/")
        url.pathname = "/items/a b"
        url.searchParams.set("id", "a b")
        url.searchParams.append("tag", "x/y")
        url.hash = "part 1"
        return {
          href: url.href,
          origin: url.origin,
          host: url.host,
          pathname: url.pathname,
          search: url.search,
          id: url.searchParams.get("id"),
          string: String(url),
          json: url.toJSON(),
          instances: [
            url instanceof URL,
            url.searchParams instanceof URLSearchParams,
            url.searchParams === url.searchParams,
          ],
        }
      "#
        )
        .await,
        json!({
            "href": href,
            "origin": "https://example.com:8443",
            "host": "example.com:8443",
            "pathname": "/items/a%20b",
            "search": "?id=a+b&tag=x%2Fy",
            "id": "a b",
            "string": href,
            "json": href,
            "instances": [true, true, true],
        })
    );
}

#[tokio::test]
async fn url_search_params_full_surface() {
    assert_eq!(
        value(
            r#"
        const params = new URLSearchParams([["tag", "b"], ["tag", "a"], ["q", "a b"]])
        const seen = []
        params.forEach((value, key) => seen.push(key + "=" + value))
        params.delete("tag", "b")
        params.append("tag", "c")
        params.sort()
        return {
          text: params.toString(),
          size: params.size,
          tags: params.getAll("tag"),
          has: params.has("tag", "c"),
          entries: Array.from(params),
          object: Object.fromEntries(params),
          record: new URLSearchParams({ page: 2, filter: "open" }).toString(),
          seen,
        }
      "#
        )
        .await,
        json!({
            "text": "q=a+b&tag=a&tag=c",
            "size": 3,
            "tags": ["a", "c"],
            "has": true,
            "entries": [["q", "a b"], ["tag", "a"], ["tag", "c"]],
            "object": { "q": "a b", "tag": "c" },
            "record": "page=2&filter=open",
            "seen": ["tag=b", "tag=a", "q=a b"],
        })
    );
}

#[tokio::test]
async fn url_parsing_failures_are_catchable_and_json_forms() {
    assert_eq!(
        value(
            r#"
        const parsed = URL.parse("/users", "https://example.test/api/")
        let invalidIsTypeError = false
        try { new URL("not relative without a base") } catch (error) { invalidIsTypeError = error instanceof TypeError }
        return {
          canParse: URL.canParse("/users", "https://example.test/api/"),
          cannotParse: URL.canParse("not relative without a base"),
          parsed: parsed.href,
          invalidIsTypeError,
          boundary: [new URL("https://example.test/a"), new URLSearchParams("q=one")],
          json: JSON.stringify({ url: new URL("https://example.test/a"), params: new URLSearchParams("q=one") }),
        }
      "#
        )
        .await,
        json!({
            "canParse": true,
            "cannotParse": false,
            "parsed": "https://example.test/users",
            "invalidIsTypeError": true,
            "boundary": ["https://example.test/a", {}],
            "json": r#"{"url":"https://example.test/a","params":{}}"#,
        })
    );
}

#[tokio::test]
async fn distinguishes_omitted_url_arguments_from_explicit_undefined() {
    assert_eq!(
        value(
            r#"
        function throwsTypeError(run) {
          try { run(); return false } catch (error) { return error instanceof TypeError }
        }
        const params = new URLSearchParams()
        const required = [
          () => params.append(),
          () => params.delete(),
          () => params.get(),
          () => params.getAll(),
          () => params.has(),
          () => params.set(),
          () => params.forEach(),
        ].map(throwsTypeError)
        params.append(undefined, undefined)
        return {
          construct: throwsTypeError(() => new URL()),
          canParse: throwsTypeError(() => URL.canParse()),
          parse: throwsTypeError(() => URL.parse()),
          explicitUndefined: new URL(undefined, "https://example.test/base/").href,
          params: params.toString(),
          required,
        }
      "#
        )
        .await,
        json!({
            "construct": true,
            "canParse": true,
            "parse": true,
            "explicitUndefined": "https://example.test/base/undefined",
            "params": "undefined=undefined",
            "required": [true, true, true, true, true, true, true],
        })
    );
}

// Map
#[tokio::test]
async fn map_get_set_has_size_with_chaining() {
    assert_eq!(
        value(
            r#"
      const m = new Map()
      m.set("a", 1).set("b", 2)
      return { a: m.get("a"), b: m.get("b"), has: m.has("a"), miss: m.get("zz") === undefined, size: m.size }
    "#
        )
        .await,
        json!({ "a": 1, "b": 2, "has": true, "miss": true, "size": 2 })
    );
}

#[tokio::test]
async fn map_object_keys_use_identity() {
    assert_eq!(
        value(
            r#"
      const key = { id: 1 }
      const m = new Map()
      m.set(key, "hit")
      return [m.get(key), m.get({ id: 1 }) === undefined]
    "#
        )
        .await,
        json!(["hit", true])
    );
}

#[tokio::test]
async fn map_construction_from_pairs_and_maps() {
    assert_eq!(
        value(r#"const m = new Map([["a", 1], ["b", 2]]); return m.get("b")"#).await,
        json!(2)
    );
    assert_eq!(
        value(r#"const m = new Map([["a", 1]]); const n = new Map(m); n.set("b", 2); return [n.get("a"), n.get("b"), m.has("b")]"#).await,
        json!([1, 2, false])
    );
    assert!(
        error(r#"return new Map("nope")"#)
            .await
            .message
            .contains("[key, value] pairs")
    );
    assert!(
        error(r#"return new Map(["flat"])"#)
            .await
            .message
            .contains("[key, value] pairs")
    );
}

#[tokio::test]
async fn map_keys_values_entries_return_arrays() {
    assert_eq!(
        value(
            r#"
      const m = new Map([["a", 1], ["b", 2]])
      return { keys: m.keys(), values: m.values(), entries: m.entries() }
    "#
        )
        .await,
        json!({ "keys": ["a", "b"], "values": [1, 2], "entries": [["a", 1], ["b", 2]] })
    );
}

#[tokio::test]
async fn object_from_entries_and_array_from_map() {
    assert_eq!(
        value(r#"return Object.fromEntries(new Map([["a", 1], ["b", 2]]))"#).await,
        json!({ "a": 1, "b": 2 })
    );
    assert_eq!(
        value(r#"return Array.from(new Map([["a", 1]]))"#).await,
        json!([["a", 1]])
    );
}

#[tokio::test]
async fn map_for_of_destructures_pairs() {
    assert_eq!(
        value(
            r#"
      const m = new Map([["a", 1], ["b", 2]])
      let total = 0
      let names = ""
      for (const [key, count] of m) { names += key; total += count }
      return names + total
    "#
        )
        .await,
        json!("ab3")
    );
}

#[tokio::test]
async fn map_spread_produces_entry_pairs() {
    assert_eq!(
        value(r#"return [...new Map([["a", 1]])]"#).await,
        json!([["a", 1]])
    );
}

#[tokio::test]
async fn map_for_each_passes_value_key() {
    assert_eq!(
        value(
            r#"
      const m = new Map([["a", 1], ["b", 2]])
      const seen = []
      m.forEach((count, key) => seen.push(key + count))
      return seen
    "#
        )
        .await,
        json!(["a1", "b2"])
    );
}

#[tokio::test]
async fn map_delete_and_clear() {
    assert_eq!(
        value(
            r#"
      const m = new Map([["a", 1], ["b", 2]])
      const removed = m.delete("a")
      const missed = m.delete("zz")
      const sizeAfterDelete = m.size
      m.clear()
      return [removed, missed, sizeAfterDelete, m.size]
    "#
        )
        .await,
        json!([true, false, 1, 0])
    );
}

#[tokio::test]
async fn map_counting_idiom() {
    assert_eq!(
        value(
            r#"
      const words = ["a", "b", "a", "c", "a"]
      const counts = new Map()
      for (const word of words) counts.set(word, (counts.get(word) ?? 0) + 1)
      return Object.fromEntries(counts)
    "#
        )
        .await,
        json!({ "a": 3, "b": 1, "c": 1 })
    );
}

#[tokio::test]
async fn maps_serialize_to_empty_objects() {
    assert_eq!(value(r#"return new Map([["a", 1]])"#).await, json!({}));
    assert_eq!(
        value(r#"return JSON.stringify(new Map([["a", 1]]))"#).await,
        json!("{}")
    );
}

#[tokio::test]
async fn console_log_renders_map_contents() {
    let result = run(r#"console.log(new Map([["a", 1]])); return null"#).await;
    assert!(result.ok);
    assert_eq!(result.logs.unwrap_or_default()[0], r#"Map(1) [["a",1]]"#);
}

// Set
#[tokio::test]
async fn set_add_has_delete_size() {
    assert_eq!(
        value(
            r#"
      const s = new Set()
      s.add(1).add(2).add(1)
      const removed = s.delete(2)
      return [s.size, s.has(1), s.has(2), removed]
    "#
        )
        .await,
        json!([1, true, false, true])
    );
}

#[tokio::test]
async fn set_dedupe_idiom() {
    assert_eq!(
        value("return [...new Set([1, 2, 2, 3, 1])]").await,
        json!([1, 2, 3])
    );
}

#[tokio::test]
async fn set_construction_from_strings_and_sets() {
    assert_eq!(
        value(r#"return [...new Set("aba")]"#).await,
        json!(["a", "b"])
    );
    assert_eq!(
        value("return Array.from(new Set(new Set([1, 2])))").await,
        json!([1, 2])
    );
}

#[tokio::test]
async fn set_same_value_zero_finds_nan() {
    assert_eq!(
        value("const s = new Set([NaN]); return s.has(NaN)").await,
        json!(true)
    );
}

#[tokio::test]
async fn set_for_of_iterates_values() {
    assert_eq!(
        value("let total = 0\nfor (const n of new Set([1, 2, 3])) total += n\nreturn total").await,
        json!(6)
    );
}

#[tokio::test]
async fn sets_serialize_to_empty_objects() {
    assert_eq!(
        value("return { s: new Set([1]) }").await,
        json!({ "s": {} })
    );
}

// stdlib integration
#[tokio::test]
async fn typeof_reports_constructors_as_functions() {
    assert_eq!(value("return typeof Map").await, json!("function"));
    assert_eq!(value("return typeof ((x) => x)").await, json!("function"));
    assert_eq!(value("return typeof Math").await, json!("object"));
    assert_eq!(value("return typeof tools").await, json!("object"));
}

#[tokio::test]
async fn negation_works_on_any_value() {
    assert_eq!(value("return !new Map()").await, json!(false));
    assert_eq!(value("const fn = () => 1; return !fn").await, json!(false));
}

#[tokio::test]
async fn object_spread_of_sandbox_values_is_a_no_op() {
    assert_eq!(
        value(r#"return { ...new Map([["a", 1]]), kept: true }"#).await,
        json!({ "kept": true })
    );
}

#[tokio::test]
async fn dates_inside_map_values_survive() {
    assert_eq!(
        value(r#"const m = new Map([["start", new Date(1000)]]); return m.get("start").getTime()"#)
            .await,
        json!(1000)
    );
}

#[tokio::test]
async fn instanceof_recognizes_stdlib_types() {
    assert_eq!(
        value("return [new Date(0) instanceof Date, /a/ instanceof RegExp, new Map() instanceof Map, new Set() instanceof Set]").await,
        json!([true, true, true, true])
    );
    assert_eq!(
        value("return [[1] instanceof Array, [1] instanceof Object, ({}) instanceof Object, 5 instanceof Object]").await,
        json!([true, true, true, false])
    );
    assert_eq!(
        value(r#"return [new Map() instanceof Set, "s" instanceof Date]"#).await,
        json!([false, false])
    );
    assert_eq!(
        value("const p = Promise.resolve(1); const isPromise = p instanceof Promise; await p; return isPromise").await,
        json!(true)
    );
}

#[tokio::test]
async fn realistic_pipeline() {
    assert_eq!(
        value(
            r#"
      const raw = '[{"at":"2024-01-01T05:00:00Z","tag":"a b"},{"at":"2024-01-01T09:00:00Z","tag":"b c"},{"at":"2024-01-02T01:00:00Z","tag":"a"}]'
      const rows = JSON.parse(raw)
      const tags = new Set()
      const byDay = new Map()
      for (const row of rows) {
        for (const m of row.tag.matchAll(/[a-z]+/g)) tags.add(m[0])
        const day = new Date(row.at).toISOString().slice(0, 10)
        byDay.set(day, (byDay.get(day) ?? 0) + 1)
      }
      return { tags: [...tags].sort((a, b) => (a < b ? -1 : 1)), byDay: Object.fromEntries(byDay) }
    "#
        )
        .await,
        json!({ "tags": ["a", "b", "c"], "byDay": { "2024-01-01": 2, "2024-01-02": 1 } })
    );
}

// sandbox values at intra-sandbox checkpoints
#[tokio::test]
async fn object_values_entries_keep_dates_usable() {
    assert_eq!(
        value("return Object.values({ d: new Date(0) })[0].getTime()").await,
        json!(0)
    );
    assert_eq!(
        value(r#"const [key, d] = Object.entries({ d: new Date(0) })[0]; return key + ":" + d.getTime()"#).await,
        json!("d:0")
    );
}

#[tokio::test]
async fn object_assign_keeps_maps_usable() {
    assert_eq!(value(r#"const merged = Object.assign({}, { m: new Map([["a", 1]]) }); return merged.m.get("a")"#).await, json!(1));
}

#[tokio::test]
async fn object_and_array_spread_keep_sandbox_values_usable() {
    assert_eq!(
        value(
            r#"
      const src = { m: new Map([["a", 1]]) }
      const copy = { ...src }
      copy.m.set("b", 2)
      return [copy.m.get("a"), src.m.get("b")]
    "#
        )
        .await,
        json!([1, 2])
    );
    assert_eq!(
        value("const list = [new Date(1000)]; const copy = [...list]; return copy[0].getTime()")
            .await,
        json!(1000)
    );
}

#[tokio::test]
async fn array_from_keeps_nested_sandbox_values() {
    assert_eq!(
        value("return Array.from([new Date(5)])[0].getTime()").await,
        json!(5)
    );
}

#[tokio::test]
async fn regexes_stay_callable_through_object_values() {
    assert_eq!(
        value(r#"return Object.values({ r: /ab+/ })[0].test("abb")"#).await,
        json!(true)
    );
}

#[tokio::test]
async fn object_helpers_see_sandbox_values_as_empty_objects() {
    assert_eq!(
        value(r#"return Object.keys(new Map([["a", 1]]))"#).await,
        json!([])
    );
    assert_eq!(value("return Object.values(new Date(0))").await, json!([]));
    assert_eq!(
        value("return Object.entries(new Set([1]))").await,
        json!([])
    );
    assert_eq!(
        value(r#"return Object.assign({}, new Map([["a", 1]]))"#).await,
        json!({})
    );
    assert_eq!(
        value(r#"return Object.hasOwn(new Date(0), "time")"#).await,
        json!(false)
    );
}

#[tokio::test]
async fn host_boundary_serializes_json_forms() {
    assert_eq!(
        value(r#"return { d: new Date(0), m: new Map([["a", 1]]) }"#).await,
        json!({ "d": "1970-01-01T00:00:00.000Z", "m": {} })
    );
    assert_eq!(
        value("return JSON.stringify({ d: new Date(0) })").await,
        json!(r#"{"d":"1970-01-01T00:00:00.000Z"}"#)
    );
    let observed = Arc::new(Mutex::new(Vec::new()));
    let sink = observed.clone();
    let capture = tool(
        "Capture the exact input the host receives",
        json!({ "type": "object" }),
        None,
        move |input| {
            sink.lock()
                .map_err(|_| opencode_codemode::tool_error("poisoned"))?
                .push(input);
            Ok(json!("ok"))
        },
    );
    let result = run_tools(
        r#"return await tools.host.capture({ when: new Date(0), tags: new Map([["a", 1]]) })"#,
        ToolTree::new().namespace("host", ToolTree::new().tool("capture", capture)),
    )
    .await;
    assert!(result.ok);
    assert_eq!(
        *observed.lock().expect("lock"),
        vec![json!({ "when": "1970-01-01T00:00:00.000Z", "tags": {} })]
    );
}
