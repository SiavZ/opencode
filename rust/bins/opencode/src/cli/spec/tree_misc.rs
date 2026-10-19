//! Purpose: remaining legacy command builders: console, providers, agent, upgrade, uninstall,
//! models, stats, export, import, github, pr, session, plugin and db (packages/opencode/src/cli/cmd/*.ts).
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
//! Invariants: option order equals each builder's `.option(...)` call order.
//! Contract tests: opencode/tests/cli_help.rs
use super::model::{CommandId as Id, Default, Kind, Node, Opt};

/// `console` (cli/cmd/account.ts:237-264): `describe: false` hides it from the root list.
pub fn console() -> Node {
    Node::new("console", None, None)
        .child(
            Node::new(
                "login [url]",
                Some("log in to console"),
                Some(Id::ConsoleLogin),
            )
            .pos("url", Kind::Str, Some("server URL")),
        )
        .child(
            Node::new(
                "logout [email]",
                Some("log out from console"),
                Some(Id::ConsoleLogout),
            )
            .pos("email", Kind::Str, Some("account email to log out from")),
        )
        .child(Node::new(
            "switch",
            Some("switch active org"),
            Some(Id::ConsoleSwitch),
        ))
        .child(Node::new("orgs", Some("list orgs"), Some(Id::ConsoleOrgs)))
        .child(Node::new(
            "open",
            Some("open active console account"),
            Some(Id::ConsoleOpen),
        ))
}

/// `providers` alias `auth` (cli/cmd/providers.ts).
pub fn providers() -> Node {
    Node::new(
        "providers",
        Some("manage AI providers and credentials"),
        None,
    )
    .alias("auth")
    .child(
        Node::new(
            "list",
            Some("list providers and credentials"),
            Some(Id::ProvidersList),
        )
        .alias("ls"),
    )
    .child(
        Node::new(
            "login [url]",
            Some("log in to a provider"),
            Some(Id::ProvidersLogin),
        )
        .pos("url", Kind::Str, Some("opencode auth provider"))
        .opt(
            Opt::new(
                "provider",
                Kind::Str,
                "provider id or name to log in to (skips provider selection)",
            )
            .alias("p"),
        )
        .opt(
            Opt::new(
                "method",
                Kind::Str,
                "login method label (skips method selection)",
            )
            .alias("m"),
        ),
    )
    .child(
        Node::new(
            "logout [provider]",
            Some("log out from a configured provider"),
            Some(Id::ProvidersLogout),
        )
        .pos(
            "provider",
            Kind::Str,
            Some("provider id or name to log out from"),
        ),
    )
}

/// `AVAILABLE_PERMISSIONS` (cli/cmd/agent.ts:19-31).
pub const AVAILABLE_PERMISSIONS: [&str; 11] = [
    "bash",
    "read",
    "edit",
    "glob",
    "grep",
    "webfetch",
    "task",
    "todowrite",
    "websearch",
    "lsp",
    "skill",
];

/// `agent` (cli/cmd/agent.ts).
pub fn agent() -> Node {
    let permissions = format!(
        "comma-separated list of permissions to allow (default: all). Available: \"{}\"",
        AVAILABLE_PERMISSIONS.join(", ")
    );
    Node::new("agent", Some("manage agents"), None)
        .child(
            Node::new("create", Some("create a new agent"), Some(Id::AgentCreate))
                .opt(Opt::new(
                    "path",
                    Kind::Str,
                    "directory path to generate the agent file",
                ))
                .opt(Opt::new(
                    "description",
                    Kind::Str,
                    "what the agent should do",
                ))
                .opt(
                    Opt::new("mode", Kind::Str, "agent mode")
                        .choices(&["all", "primary", "subagent"]),
                )
                .opt(Opt::new("permissions", Kind::Str, &permissions).alias("tools"))
                .opt(
                    Opt::new(
                        "model",
                        Kind::Str,
                        "model to use in the format of provider/model",
                    )
                    .alias("m"),
                ),
        )
        .child(Node::new(
            "list",
            Some("list all available agents"),
            Some(Id::AgentList),
        ))
}

pub fn upgrade() -> Node {
    Node::new(
        "upgrade [target]",
        Some("upgrade opencode to the latest or a specific version"),
        Some(Id::Upgrade),
    )
    .pos(
        "target",
        Kind::Str,
        Some("version to upgrade to, for ex '0.1.48' or 'v0.1.48'"),
    )
    .opt(
        Opt::new("method", Kind::Str, "installation method to use")
            .alias("m")
            .choices(&["curl", "npm", "pnpm", "bun", "brew", "choco", "scoop"]),
    )
}

pub fn uninstall() -> Node {
    let no = || Default::Bool(false);
    Node::new(
        "uninstall",
        Some("uninstall opencode and remove all related files"),
        Some(Id::Uninstall),
    )
    .opt(
        Opt::new("keep-config", Kind::Bool, "keep configuration files")
            .alias("c")
            .default(no()),
    )
    .opt(
        Opt::new("keep-data", Kind::Bool, "keep session data and snapshots")
            .alias("d")
            .default(no()),
    )
    .opt(
        Opt::new(
            "dry-run",
            Kind::Bool,
            "show what would be removed without removing",
        )
        .default(no()),
    )
    .opt(
        Opt::new("force", Kind::Bool, "skip confirmation prompts")
            .alias("f")
            .default(no()),
    )
}

pub fn models() -> Node {
    Node::new(
        "models [provider]",
        Some("list all available models"),
        Some(Id::Models),
    )
    .pos(
        "provider",
        Kind::Str,
        Some("provider ID to filter models by"),
    )
    .opt(Opt::new(
        "verbose",
        Kind::Bool,
        "use more verbose model output (includes metadata like costs)",
    ))
    .opt(Opt::new(
        "refresh",
        Kind::Bool,
        "refresh the models cache from models.dev",
    ))
}

pub fn stats() -> Node {
    Node::new(
        "stats",
        Some("show token usage and cost statistics"),
        Some(Id::Stats),
    )
    .opt(Opt::new(
        "days",
        Kind::Num,
        "show stats for the last N days (default: all time)",
    ))
    .opt(Opt::new(
        "tools",
        Kind::Num,
        "number of tools to show (default: all)",
    ))
    .opt(Opt::new(
        "models",
        Kind::Any,
        "show model statistics (default: hidden). Pass a number to show top N, otherwise shows all",
    ))
    .opt(Opt::new(
        "project",
        Kind::Str,
        "filter by project (default: all projects, empty string: current project)",
    ))
}

pub fn export() -> Node {
    Node::new(
        "export [sessionID]",
        Some("export session data as JSON"),
        Some(Id::Export),
    )
    .pos("sessionID", Kind::Str, Some("session id to export"))
    .opt(Opt::new(
        "sanitize",
        Kind::Bool,
        "redact sensitive transcript and file data",
    ))
}

pub fn import() -> Node {
    Node::new(
        "import <file>",
        Some("import session data from JSON file or URL"),
        Some(Id::Import),
    )
    .pos("file", Kind::Str, Some("path to JSON file or share URL"))
}

pub fn github() -> Node {
    Node::new("github", Some("manage GitHub agent"), None)
        .child(Node::new(
            "install",
            Some("install the GitHub agent"),
            Some(Id::GithubInstall),
        ))
        .child(
            Node::new("run", Some("run the GitHub agent"), Some(Id::GithubRun))
                .opt(Opt::new(
                    "event",
                    Kind::Str,
                    "GitHub mock event to run the agent for",
                ))
                .opt(Opt::new(
                    "token",
                    Kind::Str,
                    "GitHub personal access token (github_pat_********)",
                )),
        )
}

pub fn pr() -> Node {
    Node::new(
        "pr <number>",
        Some("fetch and checkout a GitHub PR branch, then run opencode"),
        Some(Id::Pr),
    )
    .pos("number", Kind::Num, Some("PR number to checkout"))
}

pub fn session() -> Node {
    Node::new("session", Some("manage sessions"), None)
        .child(
            Node::new("list", Some("list sessions"), Some(Id::SessionList))
                .opt(Opt::new("max-count", Kind::Num, "limit to N most recent sessions").alias("n"))
                .opt(
                    Opt::new("format", Kind::Str, "output format")
                        .choices(&["table", "json"])
                        .default(Default::Str("table")),
                ),
        )
        .child(
            Node::new(
                "delete <sessionID>",
                Some("delete a session"),
                Some(Id::SessionDelete),
            )
            .pos("sessionID", Kind::Str, Some("session ID to delete")),
        )
}

pub fn plugin() -> Node {
    Node::new(
        "plugin <module>",
        Some("install plugin and update config"),
        Some(Id::Plugin),
    )
    .alias("plug")
    .pos("module", Kind::Str, Some("npm module name"))
    .opt(
        Opt::new("global", Kind::Bool, "install in global config")
            .alias("g")
            .default(Default::Bool(false)),
    )
    .opt(
        Opt::new("force", Kind::Bool, "replace existing plugin version")
            .alias("f")
            .default(Default::Bool(false)),
    )
}

/// `db` with default subcommand `$0 [query]` (cli/cmd/db.ts).
pub fn db() -> Node {
    Node::new("db", Some("database tools"), None)
        .child(
            Node::new(
                "$0 [query]",
                Some("open an interactive sqlite3 shell or run a query"),
                Some(Id::DbQuery),
            )
            .pos("query", Kind::Str, Some("SQL query to execute"))
            .opt(
                Opt::new("format", Kind::Str, "Output format")
                    .choices(&["json", "tsv"])
                    .default(Default::Str("tsv")),
            ),
        )
        .child(Node::new(
            "path",
            Some("print the database path"),
            Some(Id::DbPath),
        ))
}
