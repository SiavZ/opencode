//! Purpose: `debug` command family builders (packages/opencode/src/cli/cmd/debug/*.ts).
//! Scope: pure data.
//! Uses:
//! rust/bins/
//! `-- opencode/src/cli/spec/model.rs
//! External:
//! `-- none
//! Used by:
//! rust/bins/
//! |-- opencode/src/cli/spec/mod.rs
//! `-- opencode/src/cli/spec/tree.rs
//! Invariants: subcommand order equals debug/index.ts registration order.
//! Contract tests: opencode/tests/cli_help.rs
use super::model::{CommandId as Id, Kind, Node, Opt};

pub fn debug() -> Node {
    Node::new("debug", Some("debugging and troubleshooting tools"), None)
        .child(Node::new(
            "config",
            Some("show resolved configuration"),
            Some(Id::DebugConfig),
        ))
        .child(lsp())
        .child(rg())
        .child(file())
        .child(Node::new(
            "scrap",
            Some("list all known projects"),
            Some(Id::DebugScrap),
        ))
        .child(Node::new(
            "skill",
            Some("list all available skills"),
            Some(Id::DebugSkill),
        ))
        .child(snapshot())
        .child(Node::new(
            "startup",
            Some("print startup timing"),
            Some(Id::DebugStartup),
        ))
        .child(
            Node::new(
                "agent <name>",
                Some("show agent configuration details"),
                Some(Id::DebugAgent),
            )
            .pos("name", Kind::Str, Some("Agent name"))
            .opt(Opt::new("tool", Kind::Str, "Tool id to execute"))
            .opt(Opt::new(
                "params",
                Kind::Str,
                "Tool params as JSON or a JS object literal",
            )),
        )
        .child(Node::new(
            "v2",
            Some("debug v2 catalog and built-in plugins"),
            Some(Id::DebugV2),
        ))
        .child(Node::new(
            "info",
            Some("show debug information"),
            Some(Id::DebugInfo),
        ))
        .child(Node::new(
            "paths",
            Some("show global paths (data, config, cache, state)"),
            Some(Id::DebugPaths),
        ))
        .child(Node::new(
            "wait",
            Some("wait indefinitely (for debugging)"),
            Some(Id::DebugWait),
        ))
}

fn lsp() -> Node {
    Node::new("lsp", Some("LSP debugging utilities"), None)
        .child(
            Node::new(
                "diagnostics <file>",
                Some("get diagnostics for a file"),
                Some(Id::DebugLspDiagnostics),
            )
            .pos("file", Kind::Str, None),
        )
        .child(
            Node::new(
                "symbols <query>",
                Some("search workspace symbols"),
                Some(Id::DebugLspSymbols),
            )
            .pos("query", Kind::Str, None),
        )
        .child(
            Node::new(
                "document-symbols <uri>",
                Some("get symbols from a document"),
                Some(Id::DebugLspDocumentSymbols),
            )
            .pos("uri", Kind::Str, None),
        )
}

fn rg() -> Node {
    Node::new("rg", Some("ripgrep debugging utilities"), None)
        .child(
            Node::new(
                "files",
                Some("list files using ripgrep"),
                Some(Id::DebugRgFiles),
            )
            .opt(Opt::new("query", Kind::Str, "Filter files by query"))
            .opt(Opt::new("glob", Kind::Str, "Glob pattern to match files"))
            .opt(Opt::new("limit", Kind::Num, "Limit number of results")),
        )
        .child(
            Node::new(
                "search <pattern>",
                Some("search file contents using ripgrep"),
                Some(Id::DebugRgSearch),
            )
            .pos("pattern", Kind::Str, Some("Search pattern"))
            .opt(Opt::new("glob", Kind::Array, "File glob patterns"))
            .opt(Opt::new("limit", Kind::Num, "Limit number of results")),
        )
}

fn file() -> Node {
    Node::new("file", Some("file system debugging utilities"), None)
        .child(
            Node::new(
                "read <path>",
                Some("read file contents as JSON"),
                Some(Id::DebugFileRead),
            )
            .pos("path", Kind::Str, Some("File path to read")),
        )
        .child(
            Node::new(
                "list <path>",
                Some("list files in a directory"),
                Some(Id::DebugFileList),
            )
            .pos("path", Kind::Str, Some("File path to list")),
        )
        .child(
            Node::new(
                "search <query>",
                Some("search files by query"),
                Some(Id::DebugFileSearch),
            )
            .pos("query", Kind::Str, Some("Search query")),
        )
}

fn snapshot() -> Node {
    Node::new("snapshot", Some("snapshot debugging utilities"), None)
        .child(Node::new(
            "track",
            Some("track current snapshot state"),
            Some(Id::DebugSnapshotTrack),
        ))
        .child(
            Node::new(
                "patch <hash>",
                Some("show patch for a snapshot hash"),
                Some(Id::DebugSnapshotPatch),
            )
            .pos("hash", Kind::Str, Some("hash")),
        )
        .child(
            Node::new(
                "diff <hash>",
                Some("show diff for a snapshot hash"),
                Some(Id::DebugSnapshotDiff),
            )
            .pos("hash", Kind::Str, Some("hash")),
        )
}
