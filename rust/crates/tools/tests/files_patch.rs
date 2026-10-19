//! Purpose: conformance tests for the Core and Legacy apply_patch engines: parsing, chunk
//! matching, EOF anchoring, BOM handling, shell detection and exact error text.
//! Scope: pure public-boundary tests (ported from packages/core/test/patch.test.ts,
//! packages/opencode/test/patch/patch.test.ts and the engine-level cases of both apply_patch tool
//! suites) plus an opt-in live TypeScript differential oracle.
//! Uses:
//! rust/crates/
//! |-- tools/src/files/mod.rs
//! |-- tools/src/files/patch.rs
//! |-- tools/src/files/patch_derive.rs
//! |-- tools/src/files/patch_parse.rs
//! |-- tools/tests/files_patch/content.rs
//! `-- tools/tests/files_patch/live.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! |-- tools/tests/files_patch/content.rs
//! `-- tools/tests/files_patch/live.rs
//! Invariants: tests execute the real engine, filesystem orchestration is out of scope here.
//! Contract tests: this integration suite

use opencode_tools::files::Generation::{self, Core, Legacy};
use opencode_tools::files::patch::{
    Hunk, MaybeApplyPatch, UpdateFileChunk, derive, join_bom, maybe_parse_apply_patch, parse,
};

fn chunk(old: &[&str], new: &[&str], context: Option<&str>, eof: bool) -> UpdateFileChunk {
    UpdateFileChunk {
        old_lines: old.iter().map(|s| s.to_string()).collect(),
        new_lines: new.iter().map(|s| s.to_string()).collect(),
        change_context: context.map(Into::into),
        end_of_file: eof,
    }
}

fn error(generation: Generation, text: &str) -> String {
    parse(generation, text).unwrap_err().message
}

/// Parses then derives the single update hunk against `original`, like both tools do.
fn apply(generation: Generation, original: &str, patch: &str) -> Result<String, String> {
    let hunks = parse(generation, patch).map_err(|e| e.message)?;
    let Hunk::Update { path, chunks, .. } = &hunks[0] else {
        panic!("expected update hunk")
    };
    derive(generation, path, chunks, original)
        .map(|update| join_bom(&update.content, update.bom))
        .map_err(|e| e.message)
}

// packages/core/test/patch.test.ts

#[test]
fn core_parses_add_update_and_delete_hunks() {
    let hunks = parse(Core, "*** Begin Patch\n*** Add File: add.txt\n+added\n*** Update File: update.txt\n@@ section\n-old\n+new\n*** Delete File: delete.txt\n*** End Patch").unwrap();
    assert_eq!(
        hunks,
        vec![
            Hunk::Add {
                path: "add.txt".into(),
                contents: "added".into()
            },
            Hunk::Update {
                path: "update.txt".into(),
                move_path: None,
                chunks: vec![chunk(&["old"], &["new"], Some("section"), false)]
            },
            Hunk::Delete {
                path: "delete.txt".into()
            },
        ]
    );
}

#[test]
fn both_strip_heredoc_wrappers() {
    for generation in [Core, Legacy] {
        for text in [
            "cat <<'EOF'\n*** Begin Patch\n*** Add File: add.txt\n+added\n*** End Patch\nEOF",
            "<<EOF\n*** Begin Patch\n*** Add File: add.txt\n+added\n*** End Patch\nEOF",
            "cat <<\"X_1\"  \n*** Begin Patch\n*** Add File: add.txt\n+added\n*** End Patch\nX_1",
        ] {
            assert_eq!(
                parse(generation, text).unwrap(),
                vec![Hunk::Add {
                    path: "add.txt".into(),
                    contents: "added".into()
                }]
            );
        }
    }
}

#[test]
fn core_derives_fuzzy_updates_preserving_bom() {
    let update = derive(
        Core,
        "update.txt",
        &[chunk(&["  old   "], &["new"], None, false)],
        "\u{feff}old\n",
    )
    .unwrap();
    assert_eq!((update.content.as_str(), update.bom), ("new\n", true));
    assert_eq!(update.unified_diff, None);
    assert_eq!(join_bom(&update.content, update.bom), "\u{feff}new\n");
}

#[test]
fn eof_anchored_chunks_match_from_the_end() {
    for generation in [Core, Legacy] {
        let update = derive(
            generation,
            "update.txt",
            &[chunk(
                &["marker", "end"],
                &["marker changed", "end"],
                None,
                true,
            )],
            "marker\nmiddle\nmarker\nend\n",
        )
        .unwrap();
        assert_eq!(update.content, "marker\nmiddle\nmarker changed\nend\n");
    }
}

#[test]
fn core_records_eof_marker_but_legacy_never_does() {
    let text = "*** Begin Patch\n*** Update File: update.txt\n@@\n-last\n+end\n*** End of File\n*** End Patch";
    let expect = |eof| Hunk::Update {
        path: "update.txt".into(),
        move_path: None,
        chunks: vec![chunk(&["last"], &["end"], None, eof)],
    };
    assert_eq!(parse(Core, text).unwrap(), vec![expect(true)]);
    assert_eq!(parse(Legacy, text).unwrap(), vec![expect(false)]);
}

#[test]
fn core_rejects_malformed_bodies_and_legacy_skips_them() {
    let cases = [
        (
            "*** Begin Patch\n*** Add File: add.txt\nmissing plus\n*** End Patch",
            "Invalid add file line: missing plus",
        ),
        (
            "*** Begin Patch\n*** Update File: update.txt\n*** End Patch",
            "Invalid update hunk for update.txt: expected at least one @@ chunk",
        ),
        (
            "*** Begin Patch\n*** Delete File: delete.txt\nunexpected body\n*** End Patch",
            "Invalid patch line: unexpected body",
        ),
        (
            "*** Begin Patch\n*** Update File: u\nstray\n@@\n-a\n*** End Patch",
            "Invalid update file line: stray",
        ),
        (
            "*** Begin Patch\n*** Update File: u\n@@\n-a\n?b\n*** End Patch",
            "Invalid update chunk line: ?b",
        ),
        (
            "*** Begin Patch\n*** Frobnicate File: foo\n*** End Patch",
            "Invalid patch line: *** Frobnicate File: foo",
        ),
        (
            "*** Begin Patch\n*** Add File:   \n*** End Patch",
            "Invalid add file path",
        ),
        (
            "*** Begin Patch\n*** Delete File:\n*** End Patch",
            "Invalid delete file path",
        ),
        (
            "*** Begin Patch\n*** Update File: \n@@\n*** End Patch",
            "Invalid update file path",
        ),
        (
            "*** Begin Patch\n*** Update File: a\n*** Move to:  \n@@\n*** End Patch",
            "Invalid move file path",
        ),
    ];
    for (text, message) in cases {
        assert_eq!(error(Core, text), message, "{text}");
        assert!(parse(Legacy, text).is_ok(), "{text}");
    }
    assert_eq!(
        parse(Legacy, cases[0].0).unwrap(),
        vec![Hunk::Add {
            path: "add.txt".into(),
            contents: "".into()
        }]
    );
    assert_eq!(
        parse(Legacy, cases[9].0).unwrap(),
        vec![Hunk::Update {
            path: "a".into(),
            move_path: Some("".into()),
            chunks: vec![chunk(&[], &[], None, false)]
        }]
    );
}

// packages/opencode/test/patch/patch.test.ts (parse and shell detection)

#[test]
fn legacy_parses_add_delete_multi_and_move() {
    let add = parse(
        Legacy,
        "*** Begin Patch\n*** Add File: test.txt\n+Hello World\n*** End Patch",
    )
    .unwrap();
    assert_eq!(
        add,
        vec![Hunk::Add {
            path: "test.txt".into(),
            contents: "Hello World".into()
        }]
    );
    let delete = parse(
        Legacy,
        "*** Begin Patch\n*** Delete File: old.txt\n*** End Patch",
    )
    .unwrap();
    assert_eq!(
        delete,
        vec![Hunk::Delete {
            path: "old.txt".into()
        }]
    );
    let multi = parse(Legacy, "*** Begin Patch\n*** Add File: new.txt\n+This is a new file\n*** Update File: existing.txt\n@@\n old line\n-new line\n+updated line\n*** End Patch").unwrap();
    assert!(matches!(
        multi.as_slice(),
        [Hunk::Add { .. }, Hunk::Update { .. }]
    ));
    let moved = parse(Legacy, "*** Begin Patch\n*** Update File: old-name.txt\n*** Move to: new-name.txt\n@@\n-Old content\n+New content\n*** End Patch").unwrap();
    assert_eq!(
        moved,
        vec![Hunk::Update {
            path: "old-name.txt".into(),
            move_path: Some("new-name.txt".into()),
            chunks: vec![chunk(&["Old content"], &["New content"], None, false)]
        }]
    );
    assert_eq!(
        parse(
            Core,
            "*** Begin Patch\n*** Update File: a\n*** Move to: b\n@@\n-x\n*** End Patch"
        )
        .unwrap()[0],
        Hunk::Update {
            path: "a".into(),
            move_path: Some("b".into()),
            chunks: vec![chunk(&["x"], &[], None, false)]
        }
    );
}

#[test]
fn both_reject_missing_or_misordered_markers() {
    for generation in [Core, Legacy] {
        for text in [
            "This is not a valid patch",
            "*** End Patch\n*** Begin Patch",
            "*** Begin Patch\n*** Add File: a\n+x",
        ] {
            assert_eq!(
                error(generation, text),
                "Invalid patch format: missing Begin/End markers"
            );
        }
        assert_eq!(
            parse(generation, "  *** Begin Patch  \n *** End Patch\t").unwrap(),
            vec![]
        );
    }
    let error = parse(Legacy, "nope").unwrap_err();
    assert_eq!(
        error.js_string(),
        "Error: Invalid patch format: missing Begin/End markers"
    );
}

#[test]
fn legacy_maybe_parse_apply_patch_detects_shell_invocations() {
    let patch = "*** Begin Patch\n*** Add File: test.txt\n+Content\n*** End Patch";
    for command in ["apply_patch", "applypatch"] {
        let MaybeApplyPatch::Body { patch: body, hunks } =
            maybe_parse_apply_patch(&[command, patch])
        else {
            panic!("expected body")
        };
        assert_eq!((body.as_str(), hunks.len()), (patch, 1));
    }
    let script = format!("apply_patch <<'PATCH'\n{patch}\nPATCH");
    let MaybeApplyPatch::Body { patch: body, hunks } =
        maybe_parse_apply_patch(&["bash", "-lc", &script])
    else {
        panic!("expected heredoc body")
    };
    assert_eq!((body.as_str(), hunks.len()), (patch, 1));
    assert_eq!(
        maybe_parse_apply_patch(&["echo", "hello"]),
        MaybeApplyPatch::NotApplyPatch
    );
    assert_eq!(
        maybe_parse_apply_patch(&["bash", "-lc", "apply_patch <<PATCH\nx\nPATCH"]),
        MaybeApplyPatch::NotApplyPatch
    );
    let MaybeApplyPatch::PatchParseError(error) =
        maybe_parse_apply_patch(&["apply_patch", "garbage"])
    else {
        panic!("expected parse error")
    };
    assert_eq!(
        error.message,
        "Invalid patch format: missing Begin/End markers"
    );
}

#[path = "files_patch/content.rs"]
mod content;
#[path = "files_patch/live.rs"]
mod live;
