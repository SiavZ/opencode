//! Purpose: v2 `opencode` command tree (packages/cli/src/commands/commands.ts, effect/unstable/cli)
//! as a clap 4 command plus a byte-compatible renderer of the effect CliOutput help layout.
//! Scope: pure; distinct executable entry (`v2_command`, `v2_help`).
//! Uses:
//! `-- none
//! External:
//! `-- clap::{Arg, ArgAction, Command}
//! Used by:
//! rust/bins/
//! `-- opencode/src/cli/spec/mod.rs
//! Invariants: subcommand order and descriptions equal commands.ts; global flags equal effect's
//! built-ins (help, version, completions, log-level).
//! Contract tests: opencode/tests/cli_help_v2.rs
use clap::{Arg, ArgAction, Command};

const LOG_LEVELS: [&str; 9] = [
    "all", "trace", "debug", "info", "warn", "warning", "error", "fatal", "none",
];

/// clap tree of the v2 CLI. Parsing only; help text comes from [`help`].
pub fn command() -> Command {
    let leaf = |name: &'static str, about: &'static str| Command::new(name).about(about);
    Command::new("opencode")
        .about("OpenCode 2.0 preview command line interface")
        .disable_help_flag(true)
        .disable_version_flag(true)
        .arg(
            Arg::new("help")
                .long("help")
                .short('h')
                .action(ArgAction::SetTrue)
                .global(true),
        )
        .arg(
            Arg::new("version")
                .long("version")
                .short('v')
                .action(ArgAction::SetTrue)
                .global(true),
        )
        .arg(
            Arg::new("completions")
                .long("completions")
                .value_parser(["bash", "zsh", "fish", "sh"])
                .global(true),
        )
        .arg(
            Arg::new("log-level")
                .long("log-level")
                .value_parser(LOG_LEVELS)
                .global(true),
        )
        .subcommand(
            leaf("api", "Make a request to the running server")
                .arg(Arg::new("request").num_args(1..=2).required(true))
                .arg(Arg::new("data").long("data").short('d'))
                .arg(
                    Arg::new("header")
                        .long("header")
                        .short('H')
                        .action(ArgAction::Append),
                )
                .arg(Arg::new("param").long("param")),
        )
        .subcommand(
            leaf("debug", "Debugging and troubleshooting tools")
                .subcommand(leaf("agents", "List all agents")),
        )
        .subcommand(leaf("migrate", "Migrate v1 data to v2"))
        .subcommand(
            leaf("service", "Manage the background server")
                .subcommand(leaf("start", "Start the background server"))
                .subcommand(leaf("restart", "Restart the background server"))
                .subcommand(leaf("status", "Show background server status"))
                .subcommand(leaf("stop", "Stop the background server"))
                .subcommand(
                    leaf("password", "Get or set the server password").arg(Arg::new("value")),
                ),
        )
        .subcommand(
            leaf("serve", "Start the v2 API server")
                .arg(
                    Arg::new("hostname")
                        .long("hostname")
                        .default_value("127.0.0.1"),
                )
                .arg(
                    Arg::new("port")
                        .long("port")
                        .value_parser(clap::value_parser!(i64)),
                )
                .arg(
                    Arg::new("register")
                        .long("register")
                        .action(ArgAction::SetTrue),
                ),
        )
}

/// Help text for a v2 command path (`[]` is the root), matching effect `formatHelpDocImpl`.
pub fn help(path: &[&str]) -> Option<String> {
    let mut cmd = command();
    for p in path {
        cmd = cmd.find_subcommand(p)?.clone();
    }
    let mut s: Vec<String> = Vec::new();
    s.push("DESCRIPTION".into());
    s.push(format!(
        "  {}",
        cmd.get_about().map(|a| a.to_string()).unwrap_or_default()
    ));
    s.push(String::new());
    s.push("USAGE".into());
    let prefix = std::iter::once("opencode")
        .chain(path.iter().copied())
        .collect::<Vec<_>>()
        .join(" ");
    let has_subs = cmd.get_subcommands().next().is_some();
    let args = match path {
        ["api"] => " <operation | method path...>",
        ["service", "password"] => " [<value>]",
        _ => "",
    };
    s.push(format!(
        "  {prefix} {}[flags]{args}",
        if has_subs { "<subcommand> " } else { "" }
    ));
    s.push(String::new());
    match path {
        ["api"] => {
            s.push("ARGUMENTS".into());
            s.push("  operation | method path... stringOpenAPI operation ID, or an HTTP method followed by a path".into());
            s.push(String::new());
            s.push("FLAGS".into());
            s.push(table(
                &[
                    ("--data, -d string", "Request body"),
                    ("--header, -H string", "Request header in name:value form"),
                    ("--param key=value", "OpenAPI path or query parameter"),
                ],
                None,
            ));
            s.push(String::new());
        }
        ["service", "password"] => {
            s.push("ARGUMENTS".into());
            s.push(table(&[("value string", " (optional)")], Some(25)));
            s.push(String::new());
        }
        ["serve"] => {
            s.push("FLAGS".into());
            s.push(table(
                &[
                    ("--hostname string", ""),
                    ("--port integer", ""),
                    ("--register", ""),
                ],
                None,
            ));
            s.push(String::new());
        }
        _ => {}
    }
    s.push("GLOBAL FLAGS".into());
    let levels = LOG_LEVELS.join("|");
    let completions = "Print shell completion script (choices: bash, zsh, fish, sh)".to_string();
    let log = format!(
        "Sets the minimum log level (choices: {})",
        LOG_LEVELS.join(", ")
    );
    let log_left = format!("--log-level <{levels}>");
    s.push(table(
        &[
            ("--help, -h", "Show help information"),
            ("--version, -v", "Show version information"),
            ("--completions <bash|zsh|fish|sh>", &completions),
            (&log_left, &log),
        ],
        None,
    ));
    s.push(String::new());
    if has_subs {
        s.push("SUBCOMMANDS".into());
        let rows: Vec<(String, String)> = cmd
            .get_subcommands()
            .map(|c| {
                (
                    c.get_name().to_string(),
                    c.get_about().map(|a| a.to_string()).unwrap_or_default(),
                )
            })
            .collect();
        let refs: Vec<(&str, &str)> = rows.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
        s.push(table(&refs, Some(20)));
    }
    if s.last().is_some_and(String::is_empty) {
        s.pop();
    }
    Some(s.join("\n") + "\n")
}

fn table(rows: &[(&str, &str)], cap: Option<usize>) -> String {
    let max = rows.iter().map(|r| r.0.chars().count()).max().unwrap_or(0) + 4;
    let col = cap.map_or(max, |c| max.min(c));
    rows.iter()
        .map(|(l, r)| {
            format!(
                "  {l}{}{r}",
                " ".repeat(col.saturating_sub(l.chars().count()))
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}
