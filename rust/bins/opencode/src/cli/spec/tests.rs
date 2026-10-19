//! Purpose: in-crate conformance tests for the spec: v2 help vs captured effect-cli fixtures,
//! yargs-parser coercion of the typed parse result, middleware env and clap_complete generation.
//! Scope: unit tests (fixture-based for v2 help, pure for parsing).
//! Uses:
//! rust/bins/
//! `-- opencode/src/cli/spec/mod.rs
//! External:
//! |-- clap_complete::Shell::Bash
//! |-- clap_complete::Shell::Fish
//! `-- clap_complete::Shell::Zsh
//! Used by:
//! rust/bins/
//! `-- opencode/src/cli/spec/mod.rs
//! Invariants: expectations come from baseline captures or probes of yargs 18 (recorded inline).
//! Contract tests: self
use super::*;

fn env() -> ParseEnv {
    ParseEnv {
        version: "local".into(),
        cwd: "/work".into(),
        tty: false,
        zsh: true,
    }
}

fn run(args: &[&str]) -> Parsed {
    let argv: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    match parse(&argv, &env()) {
        Invocation::Run(p) => p,
        other => panic!("expected run for {args:?}, got {other:?}"),
    }
}

fn fails(args: &[&str]) -> bool {
    let argv: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    matches!(parse(&argv, &env()), Invocation::Fail { .. })
}

fn fixture(name: &str) -> String {
    let path = format!(
        "{}/tests/fixtures/help/v2/{name}.stdout",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(path).expect("v2 fixture")
}

#[test]
fn v2_help_matches_fixtures() {
    let cases: &[(&str, &[&str])] = &[
        ("root", &[]),
        ("api", &["api"]),
        ("debug", &["debug"]),
        ("debug_agents", &["debug", "agents"]),
        ("migrate", &["migrate"]),
        ("service", &["service"]),
        ("service_start", &["service", "start"]),
        ("service_restart", &["service", "restart"]),
        ("service_status", &["service", "status"]),
        ("service_stop", &["service", "stop"]),
        ("service_password", &["service", "password"]),
        ("serve", &["serve"]),
        ("help_short", &["service", "password"]),
        ("err_service_no_sub", &["service"]),
    ];
    for (name, path) in cases {
        assert_eq!(
            v2_help(path).as_deref(),
            Some(fixture(name).as_str()),
            "v2 fixture {name}"
        );
    }
    assert!(v2_help(&["bogus"]).is_none());
}

#[test]
fn v2_clap_tree_parses_commands() {
    let m = v2_command()
        .try_get_matches_from(["opencode", "serve", "--port", "4096", "--register"])
        .expect("serve");
    let serve = m.subcommand_matches("serve").expect("serve");
    assert_eq!(serve.get_one::<i64>("port"), Some(&4096));
    assert_eq!(
        serve.get_one::<String>("hostname").map(String::as_str),
        Some("127.0.0.1")
    );
    assert!(serve.get_flag("register"));
    assert!(
        v2_command()
            .try_get_matches_from(["opencode", "serve", "--port", "abc"])
            .is_err()
    );
    assert!(
        v2_command()
            .try_get_matches_from(["opencode", "api"])
            .is_err()
    );
    let m = v2_command()
        .try_get_matches_from(["opencode", "service", "password", "x"])
        .expect("password");
    let pw = m
        .subcommand_matches("service")
        .and_then(|s| s.subcommand_matches("password"))
        .expect("pw");
    assert_eq!(pw.get_one::<String>("value").map(String::as_str), Some("x"));
}

#[test]
fn default_tui_command_and_network_defaults() {
    let p = run(&["proj", "--port", "0x10", "--cors", "a", "b", "--no-mdns"]);
    assert_eq!(p.command, CommandId::Tui);
    assert!(p.path.is_empty());
    assert_eq!(p.args.str("project").as_deref(), Some("proj"));
    assert_eq!(p.args.num("port"), Some(16.0));
    assert_eq!(p.args.strings("cors"), vec!["a", "b"]);
    assert_eq!(p.args.bool_opt("mdns"), Some(false));
    assert_eq!(p.args.str("hostname").as_deref(), Some("127.0.0.1"));
    assert_eq!(p.args.bool_opt("auto"), Some(false));
}

#[test]
fn run_collects_message_and_double_dash() {
    let p = run(&["run", "hello", "world", "-m", "a/b", "--", "--literal", "x"]);
    assert_eq!(p.command, CommandId::Run);
    assert_eq!(p.args.strings("message"), vec!["hello", "world"]);
    assert_eq!(p.args.str("model").as_deref(), Some("a/b"));
    assert_eq!(p.double_dash, vec!["--literal", "x"]);
    assert_eq!(p.args.str("format").as_deref(), Some("default"));
    assert_eq!(p.args.bool_opt("replay"), Some(true));
    // yargs: `run` with no message yields the `[]` default.
    assert_eq!(
        run(&["run"]).args.get("message"),
        Some(&Value::List(vec![]))
    );
}

#[test]
fn yargs_coercion_rules() {
    // `-n5` short with numeric remainder; camelCase alias of kebab option.
    assert_eq!(
        run(&["session", "list", "-n5"]).args.num("max-count"),
        Some(5.0)
    );
    assert_eq!(
        run(&["session", "list", "--maxCount", "3"])
            .args
            .num("max-count"),
        Some(3.0)
    );
    // NaN numbers become null.
    assert_eq!(run(&["--port", "abc"]).args.get("port"), Some(&Value::Null));
    // Untyped `stats --models`: bare is true, numeric coerces, other strings stay strings.
    assert_eq!(
        run(&["stats", "--models"]).args.get("models"),
        Some(&Value::Bool(true))
    );
    assert_eq!(
        run(&["stats", "--models", "5"]).args.num("models"),
        Some(5.0)
    );
    assert_eq!(
        run(&["stats", "--models", "007"])
            .args
            .str("models")
            .as_deref(),
        Some("007")
    );
    // String option without value is the empty string; negative numbers are values.
    assert_eq!(run(&["--model"]).args.str("model").as_deref(), Some(""));
    assert_eq!(
        run(&["--model", "-5"]).args.str("model").as_deref(),
        Some("-5")
    );
    // Booleans: `--x=1` is false, explicit `false` consumes the token.
    assert_eq!(run(&["--mini=1"]).args.bool_opt("mini"), Some(false));
    let p = run(&["--mini", "false", "dir"]);
    assert_eq!(
        (p.args.bool_opt("mini"), p.args.str("project").as_deref()),
        (Some(false), Some("dir"))
    );
    // Explicit `no-replay` option vs negation of `replay`.
    assert_eq!(run(&["--no-replay"]).args.bool_opt("replay"), Some(false));
    // Positional numbers coerce.
    assert_eq!(run(&["pr", "42"]).args.num("number"), Some(42.0));
    // acp cwd default comes from the parse env.
    assert_eq!(run(&["acp"]).args.str("cwd").as_deref(), Some("/work"));
}

#[test]
fn aliases_and_nested_defaults() {
    assert_eq!(run(&["auth", "ls"]).command, CommandId::ProvidersList);
    assert_eq!(run(&["plug", "x", "-g"]).command, CommandId::Plugin);
    assert_eq!(run(&["mcp", "auth", "ls"]).command, CommandId::McpAuthList);
    assert_eq!(
        run(&["mcp", "auth", "srv"]).args.str("name").as_deref(),
        Some("srv")
    );
    let db = run(&["db", "select 1", "--format", "json"]);
    assert_eq!(
        (db.command, db.path.clone()),
        (CommandId::DbQuery, vec!["db", "$0"])
    );
    assert_eq!(run(&["db", "path"]).command, CommandId::DbPath);
    assert_eq!(run(&["console", "login"]).command, CommandId::ConsoleLogin);
    assert_eq!(
        run(&["agent", "create", "--tools", "bash"])
            .args
            .str("permissions")
            .as_deref(),
        Some("bash")
    );
}

#[test]
fn strict_failures() {
    assert!(fails(&["--definitely-unknown"]));
    assert!(fails(&["mcp", "auth", "list", "extra"]));
    assert!(fails(&["serve", "--prompt", "x"]));
    assert!(fails(&["upgrade", "--method", "apt"]));
    assert!(fails(&["import"]));
    assert!(fails(&["github"]));
    assert!(fails(&["a", "b"]));
}

#[test]
fn globals_and_middleware_env() {
    let p = run(&["--print-logs", "--log-level", "DEBUG", "--pure", "models"]);
    assert_eq!(p.command, CommandId::Models);
    assert_eq!(
        p.globals,
        Globals {
            print_logs: true,
            log_level: Some("DEBUG".into()),
            pure: true
        }
    );
    let env = p.globals.middleware_env(7);
    let keys: Vec<_> = env.iter().map(|(k, _)| *k).collect();
    assert_eq!(
        keys,
        [
            "OPENCODE_PRINT_LOGS",
            "OPENCODE_LOG_LEVEL",
            "OPENCODE_PURE",
            "AGENT",
            "OPENCODE",
            "OPENCODE_PID"
        ]
    );
    assert_eq!(env[5].1, "7");
    assert_eq!(Globals::default().middleware_env(1).len(), 3);
}

#[test]
fn clap_projection_and_completions() {
    let cmd = command();
    let names: Vec<_> = cmd
        .get_subcommands()
        .map(|c| c.get_name().to_string())
        .collect();
    for expected in [
        "acp",
        "mcp",
        "attach",
        "run",
        "debug",
        "providers",
        "session",
        "plugin",
        "db",
        "console",
    ] {
        assert!(names.iter().any(|n| n == expected), "missing {expected}");
    }
    for shell in [
        clap_complete::Shell::Bash,
        clap_complete::Shell::Zsh,
        clap_complete::Shell::Fish,
    ] {
        let script = clap_completion(shell);
        assert!(
            script.contains("opencode") && script.contains("mdns-domain"),
            "{shell:?}"
        );
    }
}
