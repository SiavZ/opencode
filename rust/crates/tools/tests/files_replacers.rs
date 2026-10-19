//! Purpose: conformance tests for all nine legacy fuzzy edit strategies and exact errors.
//! Scope: pure public-boundary tests plus an opt-in live TypeScript differential oracle.
//! Uses:
//! rust/crates/
//! `-- tools/src/files/replacers.rs
//! External:
//! |-- serde_json::Value
//! |-- serde_json::from_slice
//! |-- serde_json::json
//! |-- serde_json::to_string
//! `-- tempfile::tempdir
//! Used by:
//! `-- none, entrypoint
//! Invariants: tests execute the real engine, no matching logic is duplicated in fixtures.
//! Contract tests: this integration suite

use opencode_tools::files::replacers::{STRATEGIES, Strategy, candidates, replace, trim_diff};
use serde_json::{Value, json};

#[test]
fn every_strategy_yields_original_candidate_spans() {
    let cases = [
        (Strategy::Simple, "irrelevant", "find", vec!["find"]),
        (
            Strategy::LineTrimmed,
            "before\n  one \n\ttwo\nafter",
            "one\ntwo\n",
            vec!["  one \n\ttwo"],
        ),
        (
            Strategy::BlockAnchor,
            "start\nvalue=124\nend",
            "start\nvalue=123\nend",
            vec!["start\nvalue=124\nend"],
        ),
        (
            Strategy::WhitespaceNormalized,
            "before hello\t  world after",
            "hello world",
            vec!["hello\t  world"],
        ),
        (
            Strategy::IndentationFlexible,
            "  start\n    middle\n  end",
            "start\n  middle\nend",
            vec!["  start\n    middle\n  end"],
        ),
        (
            Strategy::EscapeNormalized,
            "alpha\nbeta",
            "alpha\\nbeta",
            vec!["alpha\nbeta", "alpha\nbeta"],
        ),
        (
            Strategy::TrimmedBoundary,
            "before target after",
            "  target  ",
            vec!["target"],
        ),
        (
            Strategy::ContextAware,
            "start\nsame\nchanged\nend",
            "start\nsame\nother\nend",
            vec!["start\nsame\nchanged\nend"],
        ),
        (
            Strategy::MultiOccurrence,
            "foo and foo",
            "foo",
            vec!["foo", "foo"],
        ),
    ];
    for (strategy, content, find, expected) in cases {
        assert_eq!(
            candidates(strategy, content, find),
            expected,
            "{strategy:?}"
        );
    }
    assert_eq!(STRATEGIES.len(), 9);
}

#[test]
fn exact_and_fuzzy_replacement_order() {
    assert_eq!(
        replace("old content here", "old content", "new content", false).unwrap(),
        "new content here"
    );
    assert_eq!(
        replace("  old  \nnext", "old", "new", false).unwrap(),
        "  new  \nnext"
    );
    assert_eq!(
        replace(
            "start\nvalue=124\nend",
            "start\nvalue=123\nend",
            "updated",
            false
        )
        .unwrap(),
        "updated"
    );
    assert_eq!(
        replace("hello\tworld", "hello world", "done", false).unwrap(),
        "done"
    );
    assert_eq!(
        replace("alpha\nbeta", "alpha\\nbeta", "done", false).unwrap(),
        "done"
    );
    // A later full-line candidate can be unique even when an exact substring is ambiguous.
    assert_eq!(
        replace("foo\n  foo  ", "foo", "new", false).unwrap(),
        "foo\nnew"
    );
}

#[test]
fn identical_empty_missing_and_multiple_errors_are_exact() {
    let cases = [
        (
            "content",
            "same",
            "same",
            "No changes to apply: oldString and newString are identical.",
        ),
        (
            "content",
            "",
            "new",
            "oldString cannot be empty when editing an existing file. Provide the exact text to replace, or use write for an intentional full-file replacement.",
        ),
        (
            "actual",
            "missing",
            "new",
            "Could not find oldString in the file. It must match exactly, including whitespace, indentation, and line endings.",
        ),
        (
            "foo foo",
            "foo",
            "new",
            "Found multiple matches for oldString. Provide more surrounding context to make the match unique.",
        ),
        (
            "aaa",
            "aa",
            "new",
            "Found multiple matches for oldString. Provide more surrounding context to make the match unique.",
        ),
    ];
    for (content, old, new, expected) in cases {
        assert_eq!(
            replace(content, old, new, false).unwrap_err().message,
            expected
        );
    }
}

#[test]
fn replace_all_nonoverlap_and_javascript_dollar_templates() {
    assert_eq!(
        replace("foo bar foo baz foo", "foo", "qux", true).unwrap(),
        "qux bar qux baz qux"
    );
    assert_eq!(replace("aaa", "aa", "b", true).unwrap(), "ba");
    assert_eq!(
        replace("ab-ab", "ab", "[$&][$$][$1]", true).unwrap(),
        "[ab][$][$1]-[ab][$][$1]"
    );
    assert_eq!(replace("ab-ab", "ab", "$`", true).unwrap(), "-ab-");
    assert_eq!(replace("ab-ab", "ab", "$'", true).unwrap(), "-ab-");
    assert_eq!(replace("one", "one", "$&", false).unwrap(), "$&");
}

#[test]
fn block_anchor_thresholds_size_limits_and_first_closing_anchor() {
    assert!(
        candidates(
            Strategy::BlockAnchor,
            "start\n1234567890\nend",
            "start\n123456zzzz\nend"
        )
        .is_empty()
    );
    assert_eq!(
        candidates(
            Strategy::BlockAnchor,
            "start\n1234567890\nend",
            "start\n1234567zzz\nend"
        ),
        ["start\n1234567890\nend"]
    );
    assert!(
        candidates(
            Strategy::BlockAnchor,
            "start\na\nb\nc\nd\nend",
            "start\na\nend"
        )
        .is_empty()
    );
    assert!(candidates(Strategy::BlockAnchor, "start\n\nend", "start\n\nend").is_empty());
    assert!(
        candidates(
            Strategy::BlockAnchor,
            "start\na\nend\na\nend",
            "start\na\na\na\nend"
        )
        .is_empty()
    );
    let content = "start\nvalue=000\nend\nstart\nvalue=124\nend";
    assert_eq!(
        candidates(Strategy::BlockAnchor, content, "start\nvalue=123\nend"),
        ["start\nvalue=124\nend"]
    );
    assert_eq!(
        candidates(
            Strategy::BlockAnchor,
            "start\n😀😀😀\nend",
            "start\n😀😀😁\nend"
        ),
        ["start\n😀😀😀\nend"]
    );
}

#[test]
fn unrelated_anchor_blocks_are_rejected_like_baseline_tests() {
    for content in [
        "function configure() {\n  keepImportantState()\n  removeAllUserData()\n  archiveBackups()\n  auditLog()\n}",
        "function configure() {\n  removeAllUserData()\n}",
    ] {
        assert!(
            replace(
                content,
                "function configure() {\n  const enabled = true\n}",
                "function configure() {\n  const enabled = false\n}",
                false
            )
            .unwrap_err()
            .message
            .starts_with("Could not find oldString")
        );
    }
}

#[test]
fn disproportionate_multiline_matches_fail_before_replace_all() {
    let content = format!("{}one\n{}two", " ".repeat(600), " ".repeat(600));
    for replace_all in [false, true] {
        assert_eq!(
            replace(&content, "one\ntwo", "done", replace_all)
                .unwrap_err()
                .message,
            "Refusing replacement because the matched span is much larger than oldString. Re-read the file and provide the full exact oldString for the intended replacement."
        );
    }
}

#[test]
fn crlf_bom_js_whitespace_escapes_and_boundaries() {
    assert_eq!(
        replace("alpha\r\nbeta\r\n", "alpha\r\nbeta", "new\r\ntext", false).unwrap(),
        "new\r\ntext\r\n"
    );
    assert_eq!(
        candidates(Strategy::LineTrimmed, "\u{feff}a\r\n  b\r", "a\nb"),
        ["\u{feff}a\r\n  b\r"]
    );
    assert!(candidates(Strategy::LineTrimmed, "\u{85}a", "a").is_empty());
    assert_eq!(
        candidates(Strategy::WhitespaceNormalized, "x a\u{a0}b y", "a b"),
        ["a\u{a0}b"]
    );
    assert_eq!(
        candidates(
            Strategy::EscapeNormalized,
            "a\t\r'\"`\\\n$",
            "a\\t\\r\\'\\\"\\`\\\\\\\n\\$"
        ),
        ["a\t\r'\"`\\\n$"]
    );
    assert_eq!(
        candidates(Strategy::EscapeNormalized, "a\\q", "a\\q"),
        ["a\\q", "a\\q"]
    );
    assert!(candidates(Strategy::TrimmedBoundary, "a", "a").is_empty());
    assert_eq!(
        candidates(Strategy::MultiOccurrence, "aaaa", "aa"),
        ["aa", "aa"]
    );
    assert!(candidates(Strategy::MultiOccurrence, "a", "").is_empty());
}

#[test]
fn trim_diff_preserves_patch_headers_and_removes_common_indent() {
    let source = "Index: a\n--- a\n+++ a\n@@ -1,2 +1,2 @@\n-    old\n+    new\n     context\n";
    assert_eq!(
        trim_diff(source),
        "Index: a\n--- a\n+++ a\n@@ -1,2 +1,2 @@\n-old\n+new\n context\n"
    );
    assert_eq!(trim_diff("header\n"), "header\n");
    assert_eq!(trim_diff("-\n+   \n"), "-\n+   \n");
}

#[test]
#[ignore = "requires Bun baseline checkout, run explicitly for live differential verification"]
fn live_baseline_differential() {
    let inputs = [
        ("foo foo", "foo"),
        ("aaa", "aa"),
        ("foo\n  foo  ", "foo"),
        ("start\nvalue=124\nend", "start\nvalue=123\nend"),
        ("start\nsame\nchanged\nend", "start\nsame\nother\nend"),
        ("start\n😀😀😀\nend", "start\n😀😀😁\nend"),
        ("hello\tworld", "hello world"),
        ("x a\u{a0}b y", "a b"),
        ("\u{feff}alpha\r\nbeta", "alpha\nbeta\n"),
        ("alpha\nbeta", "alpha\\nbeta"),
        ("a\\q", "a\\q"),
        ("before target after", "  target  "),
        ("\u{85}a", "a"),
        ("  start\n    middle\n  end", "start\n  middle\nend"),
        ("start\n\nend", "start\n\nend"),
        ("content", "missing"),
        ("text", " "),
        ("text", "\n"),
        ("ab-ab", "ab"),
    ];
    let cases: Vec<Value> = inputs
        .into_iter()
        .flat_map(|(content, old)| {
            [false, true].into_iter().flat_map(move |all| {
                ["done", "$&$$$`$'"]
                    .into_iter()
                    .map(move |new| json!({"content":content,"old":old,"new":new,"all":all}))
            })
        })
        .collect();
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packages/opencode");
    let temp = tempfile::tempdir().unwrap();
    let script = r#"const source=await Bun.file('src/tool/edit.ts').text();const code=new Bun.Transpiler({loader:'ts'}).transformSync(source.slice(source.indexOf('export type Replacer'))).replace(/\bexport\s+/g,'');const names=['Simple','LineTrimmed','BlockAnchor','WhitespaceNormalized','IndentationFlexible','EscapeNormalized','TrimmedBoundary','ContextAware','MultiOccurrence'];const api=new Function(code+';return {replace,trimDiff,strategies:['+names.map(name=>name+'Replacer').join(',')+']}')();const cases=JSON.parse(process.env.OPENCODE_REPLACER_CASES);console.log(JSON.stringify(cases.map(item=>{let result;try{result={ok:api.replace(item.content,item.old,item.new,item.all)}}catch(error){result={error:error.message}}return {candidates:api.strategies.map(strategy=>[...strategy(item.content,item.old)]),result}})));"#;
    let output = std::process::Command::new("bun")
        .args(["-e", script])
        .current_dir(directory)
        .env("HOME", temp.path())
        .env("XDG_DATA_HOME", temp.path())
        .env("XDG_CONFIG_HOME", temp.path())
        .env("XDG_CACHE_HOME", temp.path())
        .env(
            "OPENCODE_REPLACER_CASES",
            serde_json::to_string(&cases).unwrap(),
        )
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let expected: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap();
    for (input, expected) in cases.iter().zip(expected) {
        let content = input["content"].as_str().unwrap();
        let old = input["old"].as_str().unwrap();
        let result = match replace(
            content,
            old,
            input["new"].as_str().unwrap(),
            input["all"].as_bool().unwrap(),
        ) {
            Ok(value) => json!({"ok":value}),
            Err(error) => json!({"error":error.message}),
        };
        let actual = json!({"candidates": STRATEGIES.map(|strategy| candidates(strategy, content, old)), "result":result});
        assert_eq!(actual, expected, "{input}");
    }
}
