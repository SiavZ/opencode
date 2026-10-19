//! Purpose: root yargs tree (packages/opencode/src/index.ts) plus the tui, attach, run, acp, mcp,
//! serve and web command builders and the shared network options (cli/network.ts).
//! Scope: pure data.
//! Uses:
//! rust/bins/
//! |-- opencode/src/cli/spec/model.rs
//! |-- opencode/src/cli/spec/tree_debug.rs
//! `-- opencode/src/cli/spec/tree_misc.rs
//! External:
//! |-- tree_debug::debug
//! |-- tree_misc::agent
//! |-- tree_misc::console
//! |-- tree_misc::db
//! |-- tree_misc::export
//! |-- tree_misc::github
//! |-- tree_misc::import
//! |-- tree_misc::models
//! |-- tree_misc::plugin
//! |-- tree_misc::pr
//! |-- tree_misc::providers
//! |-- tree_misc::session
//! |-- tree_misc::stats
//! |-- tree_misc::uninstall
//! `-- tree_misc::upgrade
//! Used by:
//! rust/bins/
//! |-- opencode/src/cli/spec/mod.rs
//! `-- opencode/src/cli/spec/parse.rs
//! Invariants: registration order equals index.ts `.command(...)` order (completion first).
//! Contract tests: opencode/tests/cli_help.rs
use super::model::{CommandId as Id, Default, Kind, Node, Opt};
use super::{tree_debug, tree_misc};

/// The full legacy `opencode` command tree.
pub fn root() -> Node {
    Node::new("", None, None)
        .child(Node::new(
            "completion",
            Some("generate shell completion script"),
            Some(Id::Completion),
        ))
        .child(acp())
        .child(mcp())
        .child(tui())
        .child(attach())
        .child(run())
        .child(Node::new("generate", None, Some(Id::Generate)))
        .child(tree_debug::debug())
        .child(tree_misc::console())
        .child(tree_misc::providers())
        .child(tree_misc::agent())
        .child(tree_misc::upgrade())
        .child(tree_misc::uninstall())
        .child(
            Node::new(
                "serve",
                Some("starts a headless opencode server"),
                Some(Id::Serve),
            )
            .opts(network()),
        )
        .child(
            Node::new(
                "web",
                Some("start opencode server and open web interface"),
                Some(Id::Web),
            )
            .opts(network()),
        )
        .child(tree_misc::models())
        .child(tree_misc::stats())
        .child(tree_misc::export())
        .child(tree_misc::import())
        .child(tree_misc::github())
        .child(tree_misc::pr())
        .child(tree_misc::session())
        .child(tree_misc::plugin())
        .child(tree_misc::db())
}

/// `withNetworkOptions` (cli/network.ts:6-33).
pub fn network() -> Vec<Opt> {
    vec![
        Opt::new("port", Kind::Num, "port to listen on").default(Default::Num(0.0)),
        Opt::new("hostname", Kind::Str, "hostname to listen on").default(Default::Str("127.0.0.1")),
        Opt::new(
            "mdns",
            Kind::Bool,
            "enable mDNS service discovery (defaults hostname to 0.0.0.0)",
        )
        .default(Default::Bool(false)),
        Opt::new(
            "mdns-domain",
            Kind::Str,
            "custom domain name for mDNS service (default: opencode.local)",
        )
        .default(Default::Str("opencode.local")),
        Opt::new("cors", Kind::Array, "additional domains to allow for CORS")
            .default(Default::EmptyList),
    ]
}

fn session_opts(fork: &str) -> Vec<Opt> {
    vec![
        Opt::new("continue", Kind::Bool, "continue the last session").alias("c"),
        Opt::new("session", Kind::Str, "session id to continue").alias("s"),
        Opt::new("fork", Kind::Bool, fork),
    ]
}

fn auth_opts() -> Vec<Opt> {
    vec![
        Opt::new(
            "password",
            Kind::Str,
            "basic auth password (defaults to OPENCODE_SERVER_PASSWORD)",
        )
        .alias("p"),
        Opt::new(
            "username",
            Kind::Str,
            "basic auth username (defaults to OPENCODE_SERVER_USERNAME or 'opencode')",
        )
        .alias("u"),
    ]
}

fn mini_opts() -> Vec<Opt> {
    vec![
        Opt::new(
            "mini",
            Kind::Bool,
            "start the minimal interactive interface",
        )
        .default(Default::Bool(false)),
        Opt::hidden("replay", Kind::Bool),
        Opt::new(
            "no-replay",
            Kind::Bool,
            "disable mini session history replay on resume and after resize",
        ),
        Opt::new(
            "replay-limit",
            Kind::Num,
            "cap visible mini replay to the newest N messages",
        ),
    ]
}

fn auto_opts() -> Vec<Opt> {
    vec![
        Opt::new(
            "auto",
            Kind::Bool,
            "auto-approve permissions that are not explicitly denied (dangerous!)",
        )
        .default(Default::Bool(false)),
        Opt::hidden("yolo", Kind::Bool).default(Default::Bool(false)),
        Opt::hidden("dangerously-skip-permissions", Kind::Bool).default(Default::Bool(false)),
    ]
}

/// `$0 [project]` (cli/cmd/tui.ts:72-143).
fn tui() -> Node {
    Node::new("$0 [project]", Some("start opencode tui"), Some(Id::Tui))
        .opts(network())
        .pos("project", Kind::Str, Some("path to start opencode in"))
        .opt(
            Opt::new(
                "model",
                Kind::Str,
                "model to use in the format of provider/model",
            )
            .alias("m"),
        )
        .opts(session_opts(
            "fork the session when continuing (use with --continue or --session)",
        ))
        .opt(Opt::new("prompt", Kind::Str, "prompt to use"))
        .opt(Opt::new("agent", Kind::Str, "agent to use"))
        .opts(auto_opts())
        .opts(mini_opts())
        .opt(Opt::hidden("demo", Kind::Bool))
}

/// `attach <url>` (cli/cmd/attach.ts).
fn attach() -> Node {
    Node::new(
        "attach <url>",
        Some("attach to a running opencode server"),
        Some(Id::Attach),
    )
    .pos("url", Kind::Str, Some("http://localhost:4096"))
    .opt(Opt::new("dir", Kind::Str, "directory to run in"))
    .opts(session_opts(
        "fork the session when continuing (use with --continue or --session)",
    ))
    .opts(auth_opts())
    .opts(mini_opts())
}

/// `run [message..]` (cli/cmd/run.ts:127-262).
fn run() -> Node {
    Node::new(
        "run [message..]",
        Some("run opencode with a message"),
        Some(Id::Run),
    )
    .pos("message", Kind::Array, Some("message to send"))
    .pos_default(Default::EmptyList)
    .opt(Opt::new(
        "command",
        Kind::Str,
        "the command to run, use message for args",
    ))
    .opts(session_opts(
        "fork the session before continuing (requires --continue or --session)",
    ))
    .opt(Opt::new("share", Kind::Bool, "share the session"))
    .opt(
        Opt::new(
            "model",
            Kind::Str,
            "model to use in the format of provider/model",
        )
        .alias("m"),
    )
    .opt(Opt::new("agent", Kind::Str, "agent to use"))
    .opt(
        Opt::new(
            "format",
            Kind::Str,
            "format: default (formatted) or json (raw JSON events)",
        )
        .choices(&["default", "json"])
        .default(Default::Str("default")),
    )
    .opt(Opt::new("file", Kind::Array, "file(s) to attach to message").alias("f"))
    .opt(Opt::new(
        "title",
        Kind::Str,
        "title for the session (uses truncated prompt if no value provided)",
    ))
    .opt(Opt::new(
        "attach",
        Kind::Str,
        "attach to a running opencode server (e.g., http://localhost:4096)",
    ))
    .opts(auth_opts())
    .opt(Opt::new(
        "dir",
        Kind::Str,
        "directory to run in, path on remote server if attaching",
    ))
    .opt(Opt::new(
        "port",
        Kind::Num,
        "port for the local server (defaults to random port if no value provided)",
    ))
    .opt(Opt::new(
        "variant",
        Kind::Str,
        "model variant (provider-specific reasoning effort, e.g., high, max, minimal)",
    ))
    .opt(Opt::new("thinking", Kind::Bool, "show thinking blocks"))
    .opt(Opt::hidden("mini", Kind::Bool).default(Default::Bool(false)))
    .opt(Opt::hidden("replay", Kind::Bool).default(Default::Bool(true)))
    .opt(Opt::hidden("replay-limit", Kind::Num))
    .opt(
        Opt::new(
            "interactive",
            Kind::Bool,
            "run in direct interactive split-footer mode",
        )
        .alias("i")
        .default(Default::Bool(false)),
    )
    .opts(auto_opts())
    .opt(Opt::hidden("demo", Kind::Bool).default(Default::Bool(false)))
}

/// `acp` (cli/cmd/acp.ts:9-18).
fn acp() -> Node {
    Node::new(
        "acp",
        Some("start ACP (Agent Client Protocol) server"),
        Some(Id::Acp),
    )
    .opts(network())
    .opt(Opt::new("cwd", Kind::Str, "working directory").default(Default::Cwd))
}

/// `mcp` and its subcommands (cli/cmd/mcp.ts).
fn mcp() -> Node {
    let name = Some("name of the MCP server");
    Node::new(
        "mcp",
        Some("manage MCP (Model Context Protocol) servers"),
        None,
    )
    .child(
        Node::new("add [name]", Some("add an MCP server"), Some(Id::McpAdd))
            .pos("name", Kind::Str, name)
            .opt(Opt::new("url", Kind::Str, "URL for a remote MCP server"))
            .opt(Opt::new(
                "env",
                Kind::Array,
                "environment variable for a local MCP server (KEY=VALUE)",
            ))
            .opt(Opt::new(
                "header",
                Kind::Array,
                "HTTP header for a remote MCP server (KEY=VALUE)",
            )),
    )
    .child(
        Node::new(
            "list",
            Some("list MCP servers and their status"),
            Some(Id::McpList),
        )
        .alias("ls"),
    )
    .child(
        Node::new(
            "auth [name]",
            Some("authenticate with an OAuth-enabled MCP server"),
            Some(Id::McpAuth),
        )
        .pos("name", Kind::Str, name)
        .child(
            Node::new(
                "list",
                Some("list OAuth-capable MCP servers and their auth status"),
                Some(Id::McpAuthList),
            )
            .alias("ls")
            .help_only_pos("name", Kind::Str, name),
        ),
    )
    .child(
        Node::new(
            "logout [name]",
            Some("remove OAuth credentials for an MCP server"),
            Some(Id::McpLogout),
        )
        .pos("name", Kind::Str, name),
    )
    .child(
        Node::new(
            "debug <name>",
            Some("debug OAuth connection for an MCP server"),
            Some(Id::McpDebug),
        )
        .pos("name", Kind::Str, name),
    )
}
