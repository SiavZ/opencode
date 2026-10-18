//! Purpose: edit-tool conformance suites for both generations over real temp files, plus the
//! opt-in live jsdiff differential against the baseline `diff` package run by Bun.
//! Scope: integration entrypoint wiring the core/legacy edit modules and shared fixtures.
//! Uses:
//! rust/crates/
//! |-- tools/src/files/jsdiff.rs
//! |-- tools/tests/files_mutation/core_edit.rs
//! |-- tools/tests/files_mutation/legacy_edit.rs
//! |-- tools/tests/files_mutation/legacy_edit_lines.rs
//! `-- tools/tests/files_mutation/support.rs
//! External:
//! `-- tempfile::tempdir
//! Used by:
//! `-- none, entrypoint
//! Invariants: tests drive the real tools through `Tool::execute`, filesystem and FileMutation are
//! labelled dev adapters until p05.native-fs lands the production adapters.
//! Contract tests: this integration suite

#[path = "files_mutation/core_edit.rs"]
mod core_edit;
#[path = "files_mutation/legacy_edit.rs"]
mod legacy_edit;
#[path = "files_mutation/legacy_edit_lines.rs"]
mod legacy_edit_lines;
#[path = "files_mutation/support.rs"]
mod support;

use opencode_tools::files::jsdiff::{create_two_files_patch, diff_lines};
use serde_json::{Value, json};

fn diff_cases() -> Vec<(String, String)> {
    let base = "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk\nl\nm\n";
    let pairs = [
        ("", ""),
        ("", "x"),
        ("x", ""),
        ("x\n", "x"),
        ("x", "x\n"),
        ("a\nb\nc", "a\nB\nc"),
        ("a\r\nb\r\n", "a\nb\n"),
        ("a\r\nb\r\nc", "a\r\nX\r\nc"),
        ("one\ntwo\nthree\n", "zero\none\nthree\nfour\n"),
        ("\n\n\n", "\n\nx\n"),
        ("same", "same"),
        ("x\ny\nx\ny\nx\n", "y\nx\ny\n"),
        ("😀\nb\n", "😀\nc\n"),
    ];
    let mut cases: Vec<(String, String)> = pairs
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
    cases.push((
        base.into(),
        base.replace("b\n", "B\n").replace("l\n", "L\n"),
    ));
    cases.push((
        base.into(),
        base.replace("b\n", "B\n").replace("f\n", "F\n"),
    ));
    cases.push((
        base.into(),
        base.replace("a\n", "").replace("m\n", "m\nn\n"),
    ));
    cases.push((base.repeat(3), base.repeat(2) + "tail\n"));
    // Deterministic pseudo-random line soups over a tiny alphabet stress Myers tie-breaking.
    let mut seed: u64 = 0x5eed;
    let mut next = move || {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (seed >> 33) as usize
    };
    for _ in 0..300 {
        let mut soup = |limit: usize| {
            let lines = next() % limit;
            let mut text: String = (0..lines)
                .map(|_| {
                    ["a", "b", "c", "", " a"][next() % 5].to_string()
                        + ["\n", "\r\n"][usize::from(next() % 7 == 0)]
                })
                .collect();
            if next() % 3 == 0 {
                text.push('z');
            }
            text
        };
        cases.push((soup(14), soup(14)));
    }
    cases
}

#[test]
fn jsdiff_known_outputs() {
    assert_eq!(
        create_two_files_patch("a", "a", "", ""),
        "Index: a\n===================================================================\n--- a\n+++ a\n"
    );
    assert_eq!(
        create_two_files_patch("a", "b", "x\n", "y\n"),
        "===================================================================\n--- a\n+++ b\n@@ -1,1 +1,1 @@\n-x\n+y\n"
    );
    let changes = diff_lines("a\nb\n", "a\nc\nd\n");
    let counts: Vec<_> = changes
        .iter()
        .map(|c| (c.count, c.added, c.removed))
        .collect();
    assert_eq!(
        counts,
        [(1, false, false), (1, false, true), (2, true, false)]
    );
}

#[test]
#[ignore = "requires Bun baseline checkout, run explicitly for live differential verification"]
fn live_jsdiff_differential() {
    let cases: Vec<Value> = diff_cases()
        .into_iter()
        .map(|(old, new)| json!({"old": old, "new": new}))
        .collect();
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packages/core");
    let temp = tempfile::tempdir().unwrap();
    let script = r#"import { createTwoFilesPatch, diffLines } from "diff";const cases=JSON.parse(process.env.OPENCODE_DIFF_CASES);console.log(JSON.stringify(cases.map(c=>({patch:createTwoFilesPatch("f","f",c.old,c.new),changes:diffLines(c.old,c.new).map(x=>[x.value,x.count,!!x.added,!!x.removed])}))));"#;
    let output = std::process::Command::new("bun")
        .args(["-e", script])
        .current_dir(directory)
        .env("HOME", temp.path())
        .env("XDG_DATA_HOME", temp.path())
        .env("XDG_CONFIG_HOME", temp.path())
        .env("XDG_CACHE_HOME", temp.path())
        .env(
            "OPENCODE_DIFF_CASES",
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
    assert_eq!(expected.len(), cases.len());
    eprintln!("comparing {} jsdiff cases against Bun", cases.len());
    for (case, expected) in cases.iter().zip(expected) {
        let (old, new) = (case["old"].as_str().unwrap(), case["new"].as_str().unwrap());
        let changes: Vec<Value> = diff_lines(old, new)
            .into_iter()
            .map(|c| json!([c.value, c.count, c.added, c.removed]))
            .collect();
        let actual =
            json!({"patch": create_two_files_patch("f", "f", old, new), "changes": changes});
        assert_eq!(actual, expected, "{case}");
    }
}
