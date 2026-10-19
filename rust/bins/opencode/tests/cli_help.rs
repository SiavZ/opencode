//! Purpose: golden help conformance: runs the built `opencode` binary for every captured baseline
//! help fixture (tests/fixtures/help/*.{stdout,stderr,code}) and compares stdout, stderr and exit code.
//! Scope: fixture-based process test (no network, temp HOME).
//! Uses:
//! `-- none
//! External:
//! |-- str::to_string
//! `-- tempfile::tempdir
//! Used by:
//! `-- none, entrypoint
//! Invariants: fixtures are byte-exact baseline captures; the only normalisation is the acp
//! `--cwd` default, which embeds the capture directory.
//! Contract tests: self
use std::path::{Path, PathBuf};
use std::process::Command;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/help")
}

/// Fixture name to argv, mirroring capture-all.sh plus the explicit edge captures.
fn argv(name: &str) -> Vec<String> {
    let explicit: &[(&str, &[&str])] = &[
        ("root", &["--help"]),
        ("completion", &["completion"]),
        ("err_unknown_flag", &["--definitely-unknown"]),
        ("err_unknown_command", &["session", "bogus"]),
        ("err_bad_choice", &["--log-level", "LOUD"]),
        ("err_missing_positional", &["attach"]),
        ("version", &["--version"]),
        ("err_mcp_no_sub", &["mcp"]),
        ("err_two_positionals", &["foo", "bar"]),
        ("help_after_bad_sub", &["session", "bogus", "--help"]),
        ("version_in_sub", &["mcp", "-v"]),
        (
            "err_bad_number_choice",
            &["session", "list", "--format", "xml"],
        ),
        ("err_debug_no_sub", &["debug"]),
        ("err_unknown_in_sub", &["mcp", "add", "--bogus"]),
        ("help_short_in_sub", &["run", "-h"]),
        ("help_default_project", &["foo", "--help"]),
        ("help_db_query", &["db", "x", "--help"]),
        ("completion_help", &["completion", "--help"]),
        ("help_mcp_auth_list_ls", &["mcp", "auth", "ls", "--help"]),
        ("help_providers_alias", &["auth", "--help"]),
        ("help_mcp_auth_alias", &["mcp", "auth", "ls", "--help"]),
        ("err_root_option_in_sub", &["serve", "--prompt", "x"]),
        ("err_completion_extra", &["completion", "bash"]),
        (
            "complete_root",
            &["--get-yargs-completions", "opencode", "mc"],
        ),
        (
            "complete_mcp",
            &["--get-yargs-completions", "opencode", "mcp", ""],
        ),
        (
            "complete_mcp_exact",
            &["--get-yargs-completions", "opencode", "mcp"],
        ),
        (
            "complete_mcp_list",
            &["--get-yargs-completions", "opencode", "mcp", "list", "--"],
        ),
        (
            "complete_session_list",
            &[
                "--get-yargs-completions",
                "opencode",
                "session",
                "list",
                "--",
            ],
        ),
        (
            "complete_run",
            &["--get-yargs-completions", "opencode", "run", "--"],
        ),
        (
            "complete_choices",
            &[
                "--get-yargs-completions",
                "opencode",
                "session",
                "list",
                "--format",
                "",
            ],
        ),
        (
            "complete_tui_opts",
            &["--get-yargs-completions", "opencode", "--"],
        ),
        (
            "complete_used",
            &[
                "--get-yargs-completions",
                "opencode",
                "serve",
                "--port",
                "3",
                "--",
            ],
        ),
    ];
    if let Some((_, args)) = explicit.iter().find(|(n, _)| *n == name) {
        return args.iter().map(|s| s.to_string()).collect();
    }
    let mut out: Vec<String> = name.split('_').map(str::to_string).collect();
    out.push("--help".into());
    out
}

fn run(args: &[String], cwd: &Path, home: &Path) -> (String, String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_opencode"))
        .args(args)
        .current_dir(cwd)
        .env("HOME", home)
        .env("SHELL", "/bin/zsh")
        .env_remove("ZSH_NAME")
        .output()
        .expect("spawn opencode");
    let s = |b: Vec<u8>| String::from_utf8_lossy(&b).into_owned();
    (
        s(out.stdout),
        s(out.stderr),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn help_fixtures_match_baseline() {
    let home = tempfile::tempdir().expect("tempdir");
    let cwd = tempfile::tempdir().expect("tempdir");
    let cwd_path = cwd.path().canonicalize().expect("canonical");
    let mut names: Vec<String> = std::fs::read_dir(fixtures())
        .expect("fixtures dir")
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter_map(|f| f.strip_suffix(".code").map(str::to_string))
        .collect();
    names.sort();
    assert!(
        names.len() > 70,
        "expected the full captured tree, got {}",
        names.len()
    );
    let mut failures = Vec::new();
    for name in &names {
        let read = |ext: &str| {
            std::fs::read_to_string(fixtures().join(format!("{name}.{ext}"))).expect("fixture")
        };
        let want_err = read("stderr").replace(
            "\"/Users/siavash/Projects/opencode/packages/opencode\"",
            &format!("\"{}\"", cwd_path.display()),
        );
        let want_code: i32 = read("code").trim().parse().expect("code");
        let (out, err, code) = run(&argv(name), &cwd_path, home.path());
        let (err, want_err) = if name == "acp" {
            (strip_cwd_row(&err), strip_cwd_row(&want_err))
        } else {
            (err, want_err)
        };
        if out != read("stdout") || err != want_err || code != want_code {
            failures.push(format!(
                "== {name} (code {code} want {want_code})\n-- stdout\n{out}\n-- want stdout\n{}\n-- stderr\n{err}\n-- want stderr\n{want_err}",
                read("stdout")
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} fixture(s) differ:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// The acp `--cwd` default embeds the process cwd, so its row layout depends on the path length
/// (cliui inlines or wraps it). This is ACP's final option, including any wrapped type line.
fn strip_cwd_row(text: &str) -> String {
    text.lines()
        .take_while(|line| !line.contains("--cwd"))
        .map(|line| format!("{line}\n"))
        .collect()
}

#[test]
fn acp_help_normalizes_short_and_long_cwd_without_dropping_other_options() {
    let root = tempfile::tempdir().unwrap();
    // Help does not open application state. Use the filesystem root as a genuinely short,
    // read-only cwd while HOME remains isolated, even if TMPDIR itself has a long path.
    let short = Path::new(std::path::MAIN_SEPARATOR_STR)
        .canonicalize()
        .unwrap();
    let long = root
        .path()
        .canonicalize()
        .unwrap()
        .join("long-working-directory-for-acp-help-wrapping".repeat(3));
    std::fs::create_dir(&long).unwrap();
    let expected = strip_cwd_row(&std::fs::read_to_string(fixtures().join("acp.stderr")).unwrap());
    for cwd in [&short, &long] {
        let (out, err, code) = run(&["acp".into(), "--help".into()], cwd, root.path());
        assert_eq!(code, 0);
        assert!(out.is_empty());
        assert!(err.contains("--cwd"));
        let row = err.lines().find(|line| line.contains("--cwd")).unwrap();
        assert_eq!(row.contains("[default:"), cwd == &short, "{err}");
        assert!(err.contains("working directory"));
        let compact = err.split_whitespace().collect::<String>();
        assert!(
            compact.contains(&format!("[default:\"{}\"]", cwd.display())),
            "{err}"
        );
        assert_eq!(strip_cwd_row(&err), expected);
    }
}
