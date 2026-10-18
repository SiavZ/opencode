//! Purpose: content-level patch conformance: fuzzy matching, EOF anchors, failures, insert
//! placement, legacy diff and BOM lifting for both generations.
//! Scope: cases from the legacy applyPatch suite and both apply_patch tool suites, run through the
//! real parse plus derive path.
//! Uses:
//! rust/crates/
//! |-- tools/src/files/common.rs
//! |-- tools/src/files/patch.rs
//! `-- tools/tests/files_patch.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! `-- tools/tests/files_patch.rs
//! Invariants: expectations were cross-checked by the live Bun differential.
//! Contract tests: tools/tests/files_patch.rs

use super::{apply, chunk};
use opencode_tools::files::Generation::{Core, Legacy};
use opencode_tools::files::patch::{derive, join_bom, unified_diff};

#[test]
fn update_cases_shared_by_both_generations() {
    let cases = [
        (
            "line 1\nline 2\nline 3\n",
            "@@\n line 1\n-line 2\n+line 2 updated\n line 3",
            "line 1\nline 2 updated\nline 3\n",
        ),
        (
            "old content\n",
            "@@\n-old content\n+new content",
            "new content\n",
        ),
        (
            "content 1",
            "@@\n-content 1\n+updated content 1",
            "updated content 1\n",
        ),
        ("", "@@\n+First line", "First line\n"),
        (
            "no newline",
            "@@\n-no newline\n+has newline now",
            "has newline now\n",
        ),
        (
            "line 1\nline 2\nline 3\nline 4\n",
            "@@\n line 1\n-line 2\n+LINE 2\n@@\n line 3\n-line 4\n+LINE 4",
            "line 1\nLINE 2\nline 3\nLINE 4\n",
        ),
        (
            "line1\nline2\nline3\nline4\n",
            "@@\n-line2\n+changed2\n@@\n-line4\n+changed4",
            "line1\nchanged2\nline3\nchanged4\n",
        ),
        (
            "alpha\nomega\n",
            "@@\n alpha\n+beta\n omega",
            "alpha\nbeta\nomega\n",
        ),
        (
            "no newline at end",
            "@@\n-no newline at end\n+first line\n+second line",
            "first line\nsecond line\n",
        ),
        (
            "fn a\nx=10\ny=2\nfn b\nx=10\ny=20\n",
            "@@ fn b\n-x=10\n+x=11",
            "fn a\nx=10\ny=2\nfn b\nx=11\ny=20\n",
        ),
        (
            "line1\nline2  \nline3\n",
            "@@\n-line2\n+changed",
            "line1\nchanged\nline3\n",
        ),
        (
            "line1\n  line2\nline3\n",
            "@@\n-line2\n+changed",
            "line1\nchanged\nline3\n",
        ),
        (
            "He said \u{201c}hello\u{201d}\nsome\u{2014}dash\nend\n",
            "@@\n-He said \"hello\"\n+He said \"hi\"",
            "He said \"hi\"\nsome\u{2014}dash\nend\n",
        ),
        (
            "\u{feff}class Test {}\n",
            "@@\n class Test {}\n+class Next {}",
            "\u{feff}class Test {}\nclass Next {}\n",
        ),
        ("a\n\nb\n", "@@\n-b\n-\n+c\n+", "a\n\nc\n"),
    ];
    for generation in [Core, Legacy] {
        for (original, body, expected) in cases {
            let patch = format!("*** Begin Patch\n*** Update File: f.txt\n{body}\n*** End Patch");
            assert_eq!(
                apply(generation, original, &patch).as_deref(),
                Ok(expected),
                "{generation:?} {body}"
            );
        }
    }
}

#[test]
fn eof_marker_only_anchors_in_core() {
    let patch = "*** Begin Patch\n*** Update File: e\n@@\n-marker\n-end\n+marker-changed\n+end\n*** End of File\n*** End Patch";
    let original = "start\nmarker\nend\nmiddle\nmarker\nend\n";
    assert_eq!(
        apply(Core, original, patch).unwrap(),
        "start\nmarker\nend\nmiddle\nmarker-changed\nend\n"
    );
    assert_eq!(
        apply(Legacy, original, patch).unwrap(),
        "start\nmarker-changed\nend\nmiddle\nmarker\nend\n"
    );
    let tail = "*** Begin Patch\n*** Update File: tail.txt\n@@\n-last\n+end\n*** End of File\n*** End Patch";
    for generation in [Core, Legacy] {
        assert_eq!(
            apply(generation, "alpha\nlast\n", tail).unwrap(),
            "alpha\nend\n"
        );
    }
}

#[test]
fn match_failures_report_exact_messages() {
    let missing =
        "*** Begin Patch\n*** Update File: modify.txt\n@@\n-missing\n+changed\n*** End Patch";
    let context = "*** Begin Patch\n*** Update File: m\n@@ nowhere\n-a\n+b\n*** End Patch";
    // Legacy ignores the stray blank line and merges both edits into one chunk that cannot match,
    // Core rejects the blank line as a syntax error.
    let two =
        "*** Begin Patch\n*** Update File: two_chunks.txt\n@@\n-b\n+B\n\n-d\n+D\n*** End Patch";
    for generation in [Core, Legacy] {
        assert_eq!(
            apply(generation, "line\n", missing),
            Err("Failed to find expected lines in modify.txt:\nmissing".into())
        );
        assert_eq!(
            apply(generation, "a\n", context),
            Err("Failed to find context 'nowhere' in m".into())
        );
    }
    assert_eq!(
        apply(Core, "a\nb\nc\nd\n", two),
        Err("Invalid update chunk line: ".into())
    );
    assert_eq!(
        apply(Legacy, "a\nb\nc\nd\n", two),
        Err("Failed to find expected lines in two_chunks.txt:\nb\nd".into())
    );
    assert_eq!(
        apply(
            Legacy,
            "a\nb\nc\n",
            "*** Begin Patch\n*** Update File: x\n@@\n-c\n-b\n*** End Patch"
        ),
        Err("Failed to find expected lines in x:\nc\nb".into())
    );
}

#[test]
fn pure_insert_placement_differs_by_generation() {
    let chunks = [chunk(&[], &["added"], None, false)];
    assert_eq!(
        derive(Core, "f", &chunks, "a\n\n").unwrap().content,
        "a\n\nadded\n"
    );
    assert_eq!(
        derive(Legacy, "f", &chunks, "a\n\n").unwrap().content,
        "a\nadded\n"
    );
    for generation in [Core, Legacy] {
        assert_eq!(
            derive(generation, "f", &chunks, "a\nb").unwrap().content,
            "a\nb\nadded\n"
        );
    }
}

#[test]
fn legacy_unified_diff_is_positional() {
    let update = derive(
        Legacy,
        "f",
        &[chunk(&["b"], &["B"], None, false)],
        "a\nb\nc\n",
    )
    .unwrap();
    assert_eq!(
        update.unified_diff.as_deref(),
        Some("@@ -1 +1 @@\n a\n-b\n+B\n c\n")
    );
    assert_eq!(unified_diff("same\n", "same\n"), "");
    assert_eq!(unified_diff("x\n", "\n"), "@@ -1 +1 @@\n-x\n");
}

#[test]
fn bom_inserted_by_patch_is_lifted_out_of_content() {
    for generation in [Core, Legacy] {
        let update = derive(
            generation,
            "f",
            &[chunk(&["a"], &["\u{feff}a"], None, false)],
            "a\n",
        )
        .unwrap();
        assert_eq!((update.content.as_str(), update.bom), ("a\n", true));
    }
    assert_eq!(join_bom("\u{feff}x", false), "x");
    assert_eq!(join_bom("\u{feff}x", true), "\u{feff}x");
}
