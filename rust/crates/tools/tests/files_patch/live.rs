//! Purpose: opt-in live differential of both patch engines against the Bun TypeScript baseline.
//! Scope: parse, derive and legacy shell detection on adversarial inputs, compared as JSON.
//! Uses:
//! rust/crates/
//! |-- tools/src/files/common.rs
//! `-- tools/src/files/patch.rs
//! External:
//! |-- serde_json::{Value, json}
//! `-- tempfile::tempdir
//! Used by:
//! rust/crates/
//! `-- tools/tests/files_patch.rs
//! Invariants: baseline runs from packages/opencode with HOME/XDG in a temp dir, read-only.
//! Contract tests: tools/tests/files_patch.rs

use super::chunk;
use opencode_tools::files::Generation::{Core, Legacy};
use opencode_tools::files::patch::{
    Hunk, MaybeApplyPatch, UpdateFileChunk, derive, maybe_parse_apply_patch, parse,
};
use serde_json::{Value, json};

#[test]
#[ignore = "requires Bun baseline checkout, run explicitly for live differential verification"]
fn live_baseline_differential() {
    let patches = [
        "*** Begin Patch\n*** Add File: add.txt\n+added\n+two\n*** Update File: update.txt\n@@ section\n-old\n+new\n*** Delete File: delete.txt\n*** End Patch",
        "cat <<'EOF'\n*** Begin Patch\n*** Add File: a\n+x\n*** End Patch\nEOF",
        "<<EOF  \n\n*** Begin Patch\n*** Add File: a\n+x\n*** End Patch\nEOF\nEOF \n",
        "*** Begin Patch\n*** Add File: a\nbad\n+x\n*** End Patch",
        "*** Begin Patch\n*** Update File: u\n*** Move to: v\n@@\n-a\n+b\n*** End of File\n@@ ctx \n x\n*** End Patch",
        "*** Begin Patch\n*** Update File: u\n*** Move to:\n@@\n-a\n*** End Patch",
        "*** Begin Patch\n*** Update File: u\nstray\n@@\n-a\n?b\n\n+c\n*** End Patch",
        "*** Begin Patch\n*** Update File:\n@@\n-a\n*** Delete File: d\n*** Unknown\n*** End Patch",
        "*** Begin Patch\n*** Delete File: d\nbody\n*** End Patch",
        "*** End Patch\n*** Begin Patch",
        "\u{a0}*** Begin Patch\u{3000}\n*** Add File:\u{a0}a\u{a0}\n+\u{feff}x\n*** End Patch",
        "*** Begin Patch\n*** Update File: u\n*** End Patch",
        "*** Begin Patch\n*** Add File: a\n+x\n*** Add File: b\n*** End Patch\n+tail",
    ];
    let updates = [
        (
            "marker\nmiddle\nmarker\nend\n",
            vec![chunk(&["marker", "end"], &["changed", "end"], None, true)],
        ),
        (
            "\u{feff}old\n",
            vec![chunk(&["  old   "], &["new"], None, false)],
        ),
        ("a\n\n", vec![chunk(&[], &["x"], None, false)]),
        (
            "a\n\nb",
            vec![
                chunk(&[], &["x"], Some("a"), false),
                chunk(&["b"], &["B"], None, false),
            ],
        ),
        ("a\nb\n", vec![chunk(&["b", ""], &["c", ""], None, false)]),
        ("a\nb\n", vec![chunk(&["z"], &["c"], None, false)]),
        ("a\nb\n", vec![chunk(&["a"], &["A"], Some("q"), false)]),
        (
            "x \u{2018}y\u{2019} \u{2026}\u{a0}z\u{2013}\n",
            vec![chunk(&["x 'y' ... z-"], &["ok"], None, false)],
        ),
        (
            "a\nb\nc\nb\n",
            vec![
                chunk(&["b"], &["1"], None, false),
                chunk(&["b"], &["2"], None, false),
            ],
        ),
        (
            "b\na\n",
            vec![
                chunk(&["a"], &["A"], None, false),
                chunk(&["b"], &["B"], None, false),
            ],
        ),
        ("only", vec![chunk(&["only"], &["\u{feff}x"], None, true)]),
        ("", vec![chunk(&["x"], &["y"], None, true)]),
        ("\u{85}a\u{85}\n", vec![chunk(&["a"], &["b"], None, false)]),
        ("one\ntwo\nthree\n", vec![chunk(&["two"], &[], None, false)]),
    ];
    let argvs: Vec<Vec<&str>> = vec![
        vec!["apply_patch", patches[0]],
        vec!["applypatch", "junk"],
        vec![
            "bash",
            "-lc",
            "echo hi && apply_patch  <<\"P\"\n*** Begin Patch\n*** Delete File: d\n*** End Patch\nP\nmore",
        ],
        vec![
            "bash",
            "-lc",
            "apply_patch <<P\n*** Begin Patch\n*** End Patch\nP",
        ],
        vec!["bash", "-c", "apply_patch <<'P'\nx\nP"],
        vec![patches[0]],
    ];
    let cases = json!({
        "patches": patches,
        "updates": updates.iter().map(|(original, chunks)| json!({"original": original, "chunks": chunks.iter().map(|c| json!({"old": c.old_lines, "new": c.new_lines, "context": c.change_context, "eof": c.end_of_file})).collect::<Vec<_>>()})).collect::<Vec<_>>(),
        "argvs": argvs,
    });
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packages/opencode");
    let temp = tempfile::tempdir().unwrap();
    let script = r#"
const { Patch: Core } = await import("@opencode-ai/core/patch")
const { Patch: Legacy } = await import("./src/patch/index.ts")
const input = JSON.parse(process.env.OPENCODE_PATCH_CASES)
const hunk = (h) => h.type === "add" ? { type: "add", path: h.path, contents: h.contents } : h.type === "delete" ? { type: "delete", path: h.path } : {
  type: "update", path: h.path, move: h.movePath ?? h.move_path ?? null,
  chunks: h.chunks.map((c) => ({ old: c.oldLines ?? c.old_lines, new: c.newLines ?? c.new_lines, context: c.changeContext ?? c.change_context ?? null, eof: (c.endOfFile ?? c.is_end_of_file) === true })),
}
const run = (fn) => { try { return { ok: fn() } } catch (error) { return { error: error.message } } }
const toCore = (c) => ({ oldLines: c.old, newLines: c.new, changeContext: c.context ?? undefined, endOfFile: c.eof || undefined })
const toLegacy = (c) => ({ old_lines: c.old, new_lines: c.new, change_context: c.context ?? undefined, is_end_of_file: c.eof || undefined })
console.log(JSON.stringify({
  patches: input.patches.map((p) => ({ core: run(() => Core.parse(p).map(hunk)), legacy: run(() => Legacy.parsePatch(p).hunks.map(hunk)) })),
  updates: input.updates.map((u) => ({
    core: run(() => { const r = Core.derive("f.txt", u.chunks.map(toCore), u.original); return { content: r.content, bom: r.bom, diff: null } }),
    legacy: run(() => { const r = Legacy.deriveNewContentsFromChunks("f.txt", u.chunks.map(toLegacy), u.original); return { content: r.content, bom: r.bom, diff: r.unified_diff } }),
  })),
  argvs: input.argvs.map((a) => { const r = Legacy.maybeParseApplyPatch(a); return r.type === "Body" ? { body: r.args.patch, hunks: r.args.hunks.map(hunk) } : r.type === "PatchParseError" ? { error: r.error.message } : { none: true } }),
}))
"#;
    let output = std::process::Command::new("bun")
        .args(["-e", script])
        .current_dir(directory)
        .env("HOME", temp.path())
        .env("XDG_DATA_HOME", temp.path())
        .env("XDG_CONFIG_HOME", temp.path())
        .env("XDG_CACHE_HOME", temp.path())
        .env("XDG_STATE_HOME", temp.path())
        .env("OPENCODE_PATCH_CASES", cases.to_string())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let expected: Value = serde_json::from_slice(&output.stdout).unwrap();
    let hunk = |h: &Hunk| match h {
        Hunk::Add { path, contents } => json!({"type":"add","path":path,"contents":contents}),
        Hunk::Delete { path } => json!({"type":"delete","path":path}),
        Hunk::Update {
            path,
            move_path,
            chunks,
        } => json!({"type":"update","path":path,"move":move_path,
            "chunks": chunks.iter().map(|c| json!({"old":c.old_lines,"new":c.new_lines,"context":c.change_context,"eof":c.end_of_file})).collect::<Vec<_>>()}),
    };
    let result = |r: Result<Value, String>| match r {
        Ok(value) => json!({"ok": value}),
        Err(error) => json!({"error": error}),
    };
    let parsed = |g, p: &str| {
        result(
            parse(g, p)
                .map(|hs| hs.iter().map(hunk).collect())
                .map_err(|e| e.message),
        )
    };
    let derived = |g, (original, chunks): &(&str, Vec<UpdateFileChunk>)| {
        result(
            derive(g, "f.txt", chunks, original)
                .map(|u| json!({"content":u.content,"bom":u.bom,"diff":u.unified_diff}))
                .map_err(|e| e.message),
        )
    };
    for (index, patch) in patches.iter().enumerate() {
        let actual = json!({"core": parsed(Core, patch), "legacy": parsed(Legacy, patch)});
        assert_eq!(actual, expected["patches"][index], "patch {patch:?}");
    }
    for (index, update) in updates.iter().enumerate() {
        let actual = json!({"core": derived(Core, update), "legacy": derived(Legacy, update)});
        assert_eq!(actual, expected["updates"][index], "update {:?}", update.0);
    }
    for (index, argv) in argvs.iter().enumerate() {
        let actual = match maybe_parse_apply_patch(argv) {
            MaybeApplyPatch::Body { patch, hunks } => {
                json!({"body": patch, "hunks": hunks.iter().map(hunk).collect::<Vec<_>>()})
            }
            MaybeApplyPatch::PatchParseError(error) => json!({"error": error.message}),
            MaybeApplyPatch::NotApplyPatch => json!({"none": true}),
        };
        assert_eq!(actual, expected["argvs"][index], "argv {argv:?}");
    }
}
