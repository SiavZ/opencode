//! Purpose: port of packages/codemode/test/parity.test.ts (JS-parity behaviors).
//! Scope: one Rust test per TS `test(...)`, same programs and expected values.
//! Uses:
//! rust/crates/
//! `-- codemode/tests/common/mod.rs
//! External:
//! |-- serde_json::Value
//! |-- serde_json::json
//! `-- tokio::test
//! Used by:
//! `-- none, entrypoint
//! Invariants: tests drive the real interpreter through `opencode_codemode::execute`.
//! Contract tests: self

mod common;

use common::{error, value};
use serde_json::{Value as Json, json};

// H2: string property access reads as undefined (not a throw)
#[tokio::test]
async fn unknown_property_on_a_string_is_undefined() {
    assert_eq!(
        value(r#"const s = "hi"; return s.login === undefined"#).await,
        json!(true)
    );
    assert_eq!(value(r#"const s = "hi"; return s.login"#).await, Json::Null);
}

#[tokio::test]
async fn optional_chaining_and_fallback_on_a_string_does_not_throw() {
    assert_eq!(
        value(r#"const s = "hi"; return s?.login ?? "fallback""#).await,
        json!("fallback")
    );
}

#[tokio::test]
async fn the_real_mcp_pattern_falls_through() {
    assert_eq!(
        value(r#"const me = { result: '{"login":"x"}' }; return me.result?.login ?? me.result"#)
            .await,
        json!(r#"{"login":"x"}"#)
    );
}

#[tokio::test]
async fn unknown_property_on_a_number_is_undefined() {
    assert_eq!(value(r#"return (5).foo ?? "n""#).await, json!("n"));
}

#[tokio::test]
async fn supported_string_methods_still_work() {
    assert_eq!(value(r#"return "AB".toLowerCase()"#).await, json!("ab"));
    assert_eq!(value(r#"return "hello".length"#).await, json!(5));
}

// H3: array property access reads as undefined
#[tokio::test]
async fn unknown_property_on_an_array_is_undefined() {
    assert_eq!(value("return [1,2,3].foo === undefined").await, json!(true));
    assert_eq!(value("return [1,2,3].foo").await, Json::Null);
}

#[tokio::test]
async fn optional_chaining_on_an_array_does_not_throw() {
    assert_eq!(value(r#"return [1,2,3]?.foo ?? "fb""#).await, json!("fb"));
}

#[tokio::test]
async fn unimplemented_array_methods_read_undefined() {
    assert_eq!(
        value("return [1,2,3].toSpliced === undefined").await,
        json!(true)
    );
}

#[tokio::test]
async fn supported_array_methods_and_indexing_still_work() {
    assert_eq!(
        value("return [1,2,3].map(x => x + 1)").await,
        json!([2, 3, 4])
    );
    assert_eq!(value("return [1,2,3][9] === undefined").await, json!(true));
    assert_eq!(value("return [1,2,3][9]").await, Json::Null);
}

// H6: object spread of null/undefined is a no-op
#[tokio::test]
async fn spreading_null_is_a_no_op() {
    assert_eq!(
        value("const o = null; return { ...o, a: 1 }").await,
        json!({ "a": 1 })
    );
}

#[tokio::test]
async fn spreading_an_absent_argument_merges_cleanly() {
    assert_eq!(
        value("function f(opts){ return { ...opts, a: 1 } } return f(undefined)").await,
        json!({ "a": 1 })
    );
}

#[tokio::test]
async fn spreading_a_real_object_still_works() {
    assert_eq!(
        value("const o = { a: 1 }; return { ...o, b: 2 }").await,
        json!({ "a": 1, "b": 2 })
    );
}

#[tokio::test]
async fn spreading_an_array_into_an_object_still_errors() {
    assert_eq!(
        error("return { ...[1,2], a: 1 }").await.kind,
        opencode_codemode::DiagnosticKind::InvalidDataValue
    );
}

// H4: typeof on an undeclared identifier
#[tokio::test]
async fn feature_detection_guard_does_not_throw() {
    assert_eq!(
        value(r#"return typeof foo === "undefined" ? "safe" : "no""#).await,
        json!("safe")
    );
}

#[tokio::test]
async fn typeof_of_a_declared_binding_is_unaffected() {
    assert_eq!(value("const x = 5; return typeof x").await, json!("number"));
    assert_eq!(
        value(r#"const s = "a"; return typeof s"#).await,
        json!("string")
    );
}

#[tokio::test]
async fn referencing_an_undeclared_identifier_outside_typeof_still_throws() {
    assert!(error("return foo + 1").await.message.contains("foo"));
}

// H1: NaN/Infinity
#[tokio::test]
async fn guards_run_instead_of_crashing_on_transient_nan() {
    assert_eq!(value(r#"return parseInt("abc") || 0"#).await, json!(0));
    assert_eq!(
        value(r#"const x = Number("abc"); return Number.isNaN(x) ? 0 : x"#).await,
        json!(0)
    );
    assert_eq!(
        value("const o = {}; o.count = (o.count || 0) + 1; return o.count").await,
        json!(1)
    );
    assert_eq!(
        value("const a = []; return a.length ? a.reduce((s,x)=>s+x,0)/a.length : 0").await,
        json!(0)
    );
}

#[tokio::test]
async fn non_finite_values_become_null_at_the_boundary() {
    assert_eq!(value("return 5/0").await, Json::Null);
    assert_eq!(value("return 0/0").await, Json::Null);
    assert_eq!(value("return Math.max()").await, Json::Null);
    assert_eq!(
        value(r#"return { a: Number("x"), b: 2, c: [1/0] }"#).await,
        json!({ "a": null, "b": 2, "c": [null] })
    );
}

#[tokio::test]
async fn nan_and_infinity_are_usable_identifiers() {
    assert_eq!(value("return Number.isNaN(NaN)").await, json!(true));
    assert_eq!(value("return Infinity > 1e9").await, json!(true));
    assert_eq!(value("return Number.isFinite(1/0)").await, json!(false));
    assert_eq!(
        value("return [3,1,2].reduce((a,b)=>Math.max(a,b), -Infinity)").await,
        json!(3)
    );
    assert_eq!(
        value(r#"return JSON.stringify({ x: Number("z") })"#).await,
        json!(r#"{"x":null}"#)
    );
}

#[tokio::test]
async fn copy_out_normalizes_non_finite_numbers() {
    assert_eq!(opencode_codemode::copy_out_number(f64::NAN), Json::Null);
    assert_eq!(
        opencode_codemode::copy_out_number(f64::INFINITY),
        Json::Null
    );
    assert_eq!(
        opencode_codemode::copy_out_number(f64::NEG_INFINITY),
        Json::Null
    );
    assert_eq!(opencode_codemode::copy_out_number(42.0), json!(42));
    assert_eq!(
        value("return { a: NaN, b: [Infinity, 1] }").await,
        json!({ "a": null, "b": [null, 1] })
    );
}

// Error values and instanceof
#[tokio::test]
async fn new_error_carries_name_message_and_instanceof() {
    assert_eq!(
        value(r#"const e = new Error("boom"); return [e instanceof Error, e.name, e.message]"#)
            .await,
        json!([true, "Error", "boom"])
    );
}

#[tokio::test]
async fn error_without_new_behaves_like_new_error() {
    assert_eq!(
        value(r#"const e = Error("plain"); return [e instanceof Error, e.name, e.message]"#).await,
        json!([true, "Error", "plain"])
    );
    assert_eq!(
        value("const e = new Error(); return [e.name, e.message, e instanceof Error]").await,
        json!(["Error", "", true])
    );
}

#[tokio::test]
async fn specific_error_types_are_instanceof_themselves_and_error() {
    assert_eq!(
        value(r#"const e = new TypeError("t"); return [e instanceof TypeError, e instanceof Error, e instanceof RangeError]"#)
            .await,
        json!([true, true, false])
    );
    assert_eq!(
        value(r#"return new Error("e") instanceof TypeError"#).await,
        json!(false)
    );
}

#[tokio::test]
async fn thrown_errors_keep_instanceof_through_try_catch() {
    assert_eq!(
        value(
            r#"try { throw new Error("x") } catch (e) { return [e instanceof Error, e.message] }"#
        )
        .await,
        json!([true, "x"])
    );
}

#[tokio::test]
async fn interpreter_runtime_failures_are_caught_as_error_values() {
    assert_eq!(
        value(r#"try { JSON.parse("nope") } catch (e) { return e instanceof Error }"#).await,
        json!(true)
    );
    assert_eq!(
        value("try { undeclared() } catch (e) { return e instanceof Error }").await,
        json!(true)
    );
}

#[tokio::test]
async fn caught_failures_carry_the_real_js_constructor_name() {
    assert_eq!(
        value(
            r#"
      try { JSON.parse("{oops") } catch (e) {
        return [e.name, e instanceof SyntaxError, e instanceof Error, e instanceof TypeError, e.message.includes("JSON")]
      }
    "#
        )
        .await,
        json!(["SyntaxError", true, true, false, true])
    );
    assert_eq!(
        value("try { undeclared() } catch (e) { return [e.name, e instanceof ReferenceError] }")
            .await,
        json!(["ReferenceError", true])
    );
    assert_eq!(
        value("try { const c = 1; c = 2 } catch (e) { return [e.name, e instanceof TypeError] }")
            .await,
        json!(["TypeError", true])
    );
    assert_eq!(
        value(r#"try { "a".normalize("NOPE") } catch (e) { return [e.name, e instanceof RangeError] }"#).await,
        json!(["RangeError", true])
    );
    assert_eq!(
        value(r#"try { "a".match("(") } catch (e) { return [e.name, e instanceof SyntaxError] }"#)
            .await,
        json!(["SyntaxError", true])
    );
    assert_eq!(
        value(r#"try { new RegExp("(") } catch (e) { return [e.name, e instanceof SyntaxError] }"#)
            .await,
        json!(["SyntaxError", true])
    );
}

#[tokio::test]
async fn diagnostics_without_a_js_analogue_are_plain_error() {
    assert_eq!(
        value("try { JSON.parse(5) } catch (e) { return [e.name, e instanceof Error] }").await,
        json!(["Error", true])
    );
}

#[tokio::test]
async fn promise_all_settled_rejection_reasons_are_error_values() {
    assert_eq!(
        value(
            r#"
      const settled = await Promise.allSettled([Promise.reject(new Error("b"))])
      return [settled[0].reason instanceof Error, settled[0].reason.message]
    "#
        )
        .await,
        json!([true, "b"])
    );
}

#[tokio::test]
async fn non_error_thrown_values_are_not_instanceof_error() {
    assert_eq!(
        value(r#"try { throw "raw" } catch (e) { return e instanceof Error }"#).await,
        json!(false)
    );
    assert_eq!(
        value(r#"try { throw { message: "shaped" } } catch (e) { return e instanceof Error }"#)
            .await,
        json!(false)
    );
}

#[tokio::test]
async fn plain_data_is_never_instanceof_error() {
    assert_eq!(
        value(r#"return [({}) instanceof Error, "s" instanceof Error, null instanceof Error]"#)
            .await,
        json!([false, false, false])
    );
}

#[tokio::test]
async fn error_values_serialize_as_plain_name_message_data() {
    assert_eq!(
        value(r#"return new Error("m")"#).await,
        json!({ "name": "Error", "message": "m" })
    );
    assert_eq!(
        value(r#"return JSON.stringify(new Error("m"))"#).await,
        json!(r#"{"name":"Error","message":"m"}"#)
    );
    assert_eq!(
        value(r#"try { throw new Error("m") } catch (e) { return Object.keys(e) }"#).await,
        json!(["name", "message"])
    );
}

#[tokio::test]
async fn spreading_an_error_loses_the_brand() {
    assert_eq!(
        value(r#"const e = new Error("m"); return ({ ...e }) instanceof Error"#).await,
        json!(false)
    );
    assert_eq!(
        value(r#"const e = new Error("m"); return { ...e }"#).await,
        json!({ "name": "Error", "message": "m" })
    );
}

#[tokio::test]
async fn typeof_error_is_function_and_unknown_instanceof_rhs_is_catchable() {
    assert_eq!(value("return typeof Error").await, json!("function"));
    assert_eq!(
        value(r#"try { return 1 instanceof 5 } catch (e) { return "caught" }"#).await,
        json!("caught")
    );
    assert!(
        error("return 1 instanceof 5")
            .await
            .message
            .contains("right-hand side of 'instanceof'")
    );
}

// array methods: splice, fill, copyWithin, keys/values/entries
#[tokio::test]
async fn splice_removes_in_place_and_returns_removed() {
    assert_eq!(
        value("const a = [1,2,3,4]; const removed = a.splice(1, 2); return { removed, a }").await,
        json!({ "removed": [2, 3], "a": [1, 4] })
    );
}

#[tokio::test]
async fn splice_inserts_new_elements_at_the_cut() {
    assert_eq!(
        value(r#"const a = ["a","d"]; a.splice(1, 0, "b", "c"); return a"#).await,
        json!(["a", "b", "c", "d"])
    );
    assert_eq!(
        value(r#"const a = [1,2,3]; const removed = a.splice(1, 1, "x"); return { removed, a }"#)
            .await,
        json!({ "removed": [2], "a": [1, "x", 3] })
    );
}

#[tokio::test]
async fn splice_one_argument_and_negative_start() {
    assert_eq!(
        value("const a = [1,2,3]; const removed = a.splice(1); return { removed, a }").await,
        json!({ "removed": [2, 3], "a": [1] })
    );
    assert_eq!(
        value("const a = [1,2,3]; const removed = a.splice(-1); return { removed, a }").await,
        json!({ "removed": [3], "a": [1, 2] })
    );
}

#[tokio::test]
async fn splice_rejects_inserting_a_container_into_itself() {
    let err = error("const a = [1]; a.splice(0, 0, [a]); return a").await;
    assert_eq!(
        err.kind,
        opencode_codemode::DiagnosticKind::InvalidDataValue
    );
    assert!(err.message.contains("circular"));
}

#[tokio::test]
async fn fill_overwrites_a_range() {
    assert_eq!(
        value("const a = [1,2,3,4]; return a.fill(0, 1, 3)").await,
        json!([1, 0, 0, 4])
    );
    assert_eq!(
        value(r#"return [1,2,3].fill("z")"#).await,
        json!(["z", "z", "z"])
    );
}

#[tokio::test]
async fn copy_within_copies_a_range_in_place() {
    assert_eq!(
        value("return [1,2,3,4,5].copyWithin(0, 3)").await,
        json!([4, 5, 3, 4, 5])
    );
}

#[tokio::test]
async fn keys_values_entries_return_arrays() {
    assert_eq!(
        value(r#"return [...["x","y","z"].keys()]"#).await,
        json!([0, 1, 2])
    );
    assert_eq!(
        value(r#"return ["x","y"].values()"#).await,
        json!(["x", "y"])
    );
    assert_eq!(
        value(
            r#"
      const out = []
      for (const [index, item] of ["a","b"].entries()) out.push(index + ":" + item)
      return out
    "#
        )
        .await,
        json!(["0:a", "1:b"])
    );
    assert_eq!(value("return [...[7].entries()]").await, json!([[0, 7]]));
}

// string methods: localeCompare, normalize, trim aliases
#[tokio::test]
async fn locale_compare_orders_strings() {
    assert_eq!(
        value(r#"return ["b","a","c"].sort((x, y) => x.localeCompare(y))"#).await,
        json!(["a", "b", "c"])
    );
    assert_eq!(value(r#"return "a".localeCompare("a")"#).await, json!(0));
}

#[tokio::test]
async fn normalize_applies_unicode_forms() {
    assert_eq!(
        value(r#"return "\u0065\u0301".normalize("NFC").length"#).await,
        json!(1)
    );
    assert_eq!(
        value(r#"return "\u00e9".normalize("NFD").length"#).await,
        json!(2)
    );
    assert_eq!(
        value(r#"return "x".normalize() === "x""#).await,
        json!(true)
    );
}

#[tokio::test]
async fn an_invalid_normalize_form_is_catchable() {
    let message =
        value(r#"try { "x".normalize("nope"); return "no" } catch (e) { return e.message }"#).await;
    assert!(
        message
            .as_str()
            .is_some_and(|text| text.contains("\"NFC\""))
    );
}

#[tokio::test]
async fn trim_left_right_alias_start_end() {
    assert_eq!(value(r#"return "  x ".trimLeft()"#).await, json!("x "));
    assert_eq!(value(r#"return "  x ".trimRight()"#).await, json!("  x"));
}

// compound assignment matches its binary operator
async fn pair(compound: &str, expanded: &str) -> Json {
    let left = value(compound).await;
    assert_eq!(left, value(expanded).await);
    left
}

#[tokio::test]
async fn date_plus_equals_concatenates() {
    assert_eq!(
        pair(
            "let d = new Date(1000); d += 1; return d",
            "let d = new Date(1000); d = d + 1; return d"
        )
        .await,
        json!("1970-01-01T00:00:01.000Z1")
    );
}

#[tokio::test]
async fn date_numeric_compound_ops_use_time_value() {
    assert_eq!(
        pair(
            "let d = new Date(1000); d -= 400; return d",
            "let d = new Date(1000); d = d - 400; return d"
        )
        .await,
        json!(600)
    );
    assert_eq!(
        pair(
            "let d = new Date(1000); d /= 4; return d",
            "let d = new Date(1000); d = d / 4; return d"
        )
        .await,
        json!(250)
    );
}

#[tokio::test]
async fn string_plus_equals_object_or_array() {
    assert_eq!(
        pair(
            r#"let x = "a"; x += { b: 1 }; return x"#,
            r#"let x = "a"; x = x + { b: 1 }; return x"#
        )
        .await,
        json!("a[object Object]")
    );
    assert_eq!(
        pair(
            r#"let x = "a"; x += [1, 2]; return x"#,
            r#"let x = "a"; x = x + [1, 2]; return x"#
        )
        .await,
        json!("a1,2")
    );
}

#[tokio::test]
async fn compound_assignment_through_member_target() {
    assert_eq!(
        pair(
            r#"const o = { s: "t" }; o.s += new Date(0); return o.s"#,
            r#"const o = { s: "t" }; o.s = o.s + new Date(0); return o.s"#
        )
        .await,
        json!("t1970-01-01T00:00:00.000Z")
    );
}

#[tokio::test]
async fn numeric_and_string_compound_operators_sweep() {
    let cases: [(&str, &str, Json); 13] = [
        ("x += 3", "x = x + 3", json!(10)),
        ("x -= 3", "x = x - 3", json!(4)),
        ("x *= 3", "x = x * 3", json!(21)),
        ("x /= 2", "x = x / 2", json!(3.5)),
        ("x %= 3", "x = x % 3", json!(1)),
        ("x **= 2", "x = x ** 2", json!(49)),
        ("x &= 3", "x = x & 3", json!(3)),
        ("x |= 8", "x = x | 8", json!(15)),
        ("x ^= 2", "x = x ^ 2", json!(5)),
        ("x <<= 2", "x = x << 2", json!(28)),
        ("x >>= 1", "x = x >> 1", json!(-4)),
        ("x >>>= 1", "x = x >>> 1", json!(2_147_483_644u32)),
        ("x += \"b\"", "x = x + \"b\"", json!("ab")),
    ];
    for (compound, expanded, expected) in cases {
        let init = match compound {
            "x >>= 1" | "x >>>= 1" => "let x = -7;",
            "x += \"b\"" => "let x = \"a\";",
            _ => "let x = 7;",
        };
        assert_eq!(
            value(&format!("{init} {compound}; return x")).await,
            expected,
            "{compound}"
        );
        assert_eq!(
            value(&format!("{init} {expanded}; return x")).await,
            expected,
            "{expanded}"
        );
    }
}

// H5: builtin coercion functions as callbacks
#[tokio::test]
async fn filter_boolean_drops_falsy_values() {
    assert_eq!(
        value(r#"return [0, 1, "", 2, null, 3].filter(Boolean)"#).await,
        json!([1, 2, 3])
    );
}

#[tokio::test]
async fn map_string_coerces_each_element() {
    assert_eq!(
        value("return [1, 2, 3].map(String)").await,
        json!(["1", "2", "3"])
    );
}

#[tokio::test]
async fn arrow_callbacks_still_work() {
    assert_eq!(
        value("return [1, 2, 3, 4].filter(x => x % 2 === 0)").await,
        json!([2, 4])
    );
    assert_eq!(
        value("return [1, 2, 3].reduce((a, b) => a + b, 0)").await,
        json!(6)
    );
}

#[tokio::test]
async fn a_non_callable_callback_is_rejected() {
    assert!(
        error("return [1,2,3].map(42)")
            .await
            .message
            .contains("callback")
    );
}
