//! Purpose: shell permission scan (tree-sitter-bash), BashArity, shell selection and prompt rendering
//! conformance (ports the permission cases of packages/opencode/test/tool/shell.test.ts and
//! packages/core/test/shell.test.ts) against bundled baseline fixtures.
//! Scope: pure scanning plus real temp dirs for path containment.
//! Uses:
//! rust/crates/
//! |-- tools/src/shell/arity.rs
//! |-- tools/src/shell/mod.rs
//! |-- tools/src/shell/prompt.rs
//! |-- tools/src/shell/scan.rs
//! |-- tools/src/shell/select.rs
//! `-- tools/tests/search_support.rs
//! External:
//! |-- arity::ARITY
//! |-- schemas::PROMPTS
//! |-- schemas::core
//! |-- schemas::legacy
//! |-- select::acceptable
//! |-- select::args
//! |-- select::list
//! |-- serde_json::Value
//! `-- serde_json::from_str
//! Used by:
//! `-- none, entrypoint
//! Invariants: rendered prompts equal the baseline `ShellPrompt.render` output byte for byte.
//! Contract tests: this integration suite

#[path = "search_support.rs"]
mod support;

use opencode_tools::shell::prompt::{Limits, TEMPLATE, render};
use opencode_tools::shell::scan::{Instance, Scan, ScanError, collect, globs};
use opencode_tools::shell::{arity, schemas, select};
use serde_json::Value;
use std::path::{Path, PathBuf};
use support::{temp, write};

fn repository() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .unwrap()
}

fn scan_in(command: &str, dir: &Path) -> Scan {
    let instance = Instance {
        directory: dir.to_path_buf(),
        worktree: dir.to_path_buf(),
    };
    collect(command, dir, "bash", &instance, Path::new("/home/test")).unwrap()
}

#[test]
fn shell_txt_is_byte_identical_and_prompts_match_baseline() {
    let source =
        std::fs::read(repository().join("packages/opencode/src/tool/shell/shell.txt")).unwrap();
    assert_eq!(TEMPLATE.as_bytes(), source);
    let prompts: Value = serde_json::from_str(schemas::PROMPTS).unwrap();
    let limits = Limits {
        max_lines: 2000,
        max_bytes: 51200,
    };
    for (key, name, platform) in [
        ("zsh_darwin", "zsh", "darwin"),
        ("bash_linux", "bash", "linux"),
        ("sh_linux", "sh", "linux"),
        ("pwsh_win32", "pwsh", "win32"),
        ("pwsh_linux", "pwsh", "linux"),
        ("powershell_win32", "powershell", "win32"),
        ("cmd_win32", "cmd", "win32"),
    ] {
        let ours = render(name, platform, "/tmp/opencode", limits, 120_000);
        assert_eq!(ours, prompts[key].as_str().unwrap(), "{key}");
    }
    let custom = render(
        "bash",
        "linux",
        "/tmp/opencode",
        Limits {
            max_lines: 10,
            max_bytes: 99,
        },
        500,
    );
    assert_eq!(custom, prompts["bash_linux_custom"].as_str().unwrap());
    assert!(custom.contains("commands will time out after 500ms"));
}

#[test]
fn bash_schemas_are_captured_baseline() {
    let legacy = schemas::legacy();
    assert_eq!(legacy["required"], serde_json::json!(["command"]));
    assert_eq!(legacy["properties"]["timeout"]["exclusiveMinimum"], 0);
    let core = schemas::core("input");
    assert_eq!(
        core["properties"]["timeout"]["anyOf"][0]["allOf"][1]["maximum"],
        600000
    );
    assert!(
        schemas::core("output")["properties"]
            .get("output")
            .is_none()
    );
    assert!(core["properties"].get("background").is_none());
}

#[test]
fn arity_table_matches_baseline_and_prefixes() {
    let source =
        std::fs::read_to_string(repository().join("packages/opencode/src/permission/arity.ts"))
            .unwrap();
    let table = &source[source.find("const ARITY").unwrap()..];
    let count = table
        .lines()
        .filter(|line| {
            line.trim_start()
                .chars()
                .next()
                .is_some_and(|c| c.is_alphanumeric() || c == '"')
                && line.contains(": ")
                && line.contains("//")
        })
        .count();
    assert_eq!(arity::ARITY.len(), count);
    for (key, value) in arity::ARITY {
        let quoted = if key.contains(' ') || key.contains('-') {
            format!("\"{key}\": {value},")
        } else {
            format!("{key}: {value},")
        };
        assert!(table.contains(&quoted), "{key}");
    }
    let tokens = |text: &str| text.split(' ').map(String::from).collect::<Vec<_>>();
    assert_eq!(
        arity::prefix(&tokens("git checkout main")),
        tokens("git checkout")
    );
    assert_eq!(
        arity::prefix(&tokens("npm run dev --watch")),
        tokens("npm run dev")
    );
    assert_eq!(
        arity::prefix(&tokens("python script.py")),
        tokens("python script.py")
    );
    assert_eq!(arity::prefix(&tokens("unknown a b")), tokens("unknown"));
    assert_eq!(arity::prefix(&tokens("ls -la")), tokens("ls"));
    assert!(arity::prefix(&[]).is_empty());
}

#[test]
fn bash_patterns_multiple_commands_redirects_and_always() {
    let (_g, dir) = temp();
    let scan = scan_in("echo hello", &dir);
    assert_eq!(scan.patterns.items(), ["echo hello"]);
    let scan = scan_in("echo foo && echo bar", &dir);
    assert_eq!(scan.patterns.items(), ["echo foo", "echo bar"]);
    let scan = scan_in("echo test > output.txt", &dir);
    assert_eq!(scan.patterns.items(), ["echo test > output.txt"]);
    let scan = scan_in("ls -la", &dir);
    assert_eq!(scan.always.items(), ["ls *"]);
    let scan = scan_in("git log --oneline -5", &dir);
    assert_eq!(scan.always.items(), ["git log *"]);
    let scan = scan_in("cd .", &dir);
    assert!(scan.patterns.is_empty());
    let scan = scan_in("cd sub && npm run build", &dir);
    assert_eq!(scan.patterns.items(), ["npm run build"]);
    let nested = scan_in("echo $(cat /etc/hosts)", &dir);
    assert_eq!(
        nested.patterns.items(),
        ["echo $(cat /etc/hosts)", "cat /etc/hosts"]
    );
}

#[test]
fn external_directories_from_file_command_arguments() {
    let (_g, root) = temp();
    let dir = root.join("project");
    write(&dir.join("tmpfile"), "x");
    write(&root.join("outer/outside.txt"), "x");
    let outside = root.join("outer/outside.txt");
    let scan = scan_in(&format!("cat {}", outside.display()), &dir);
    assert_eq!(scan.dirs.items(), [root.join("outer").to_string_lossy()]);
    assert_eq!(
        globs(&scan),
        [format!("{}/*", root.join("outer").display())]
    );
    let inside = scan_in(&format!("rm -rf {}", dir.join("nested").display()), &dir);
    assert!(inside.dirs.is_empty());
    let wildcard = scan_in("cat /etc/*", &dir);
    assert_eq!(globs(&wildcard), ["/etc/*"]);
    let directory = scan_in(&format!("rm -r {}", root.join("outer").display()), &dir);
    assert_eq!(
        directory.dirs.items(),
        [root.join("outer").to_string_lossy()]
    );
    let dynamic = scan_in("cat $HOME/x ~/y", &dir);
    assert_eq!(dynamic.dirs.items(), ["/home/test"]);
    let flags = scan_in("chmod +x /opt/tool/bin -R", &dir);
    assert_eq!(flags.dirs.items(), ["/opt/tool"]);
    let relative = scan_in("cp ../escape.txt .", &dir);
    assert_eq!(relative.dirs.items(), [root.to_string_lossy()]);
    let other = scan_in("ls /etc", &dir);
    assert!(other.dirs.is_empty(), "only file commands are scanned");
}

#[test]
fn worktree_root_does_not_contain_everything() {
    let (_g, dir) = temp();
    let instance = Instance {
        directory: dir.clone(),
        worktree: PathBuf::from("/"),
    };
    let scan = collect("cat /etc/hosts", &dir, "bash", &instance, Path::new("/h")).unwrap();
    assert_eq!(scan.dirs.items(), ["/etc"]);
    assert_eq!(
        collect("Remove-Item x", &dir, "pwsh", &instance, Path::new("/h")).unwrap_err(),
        ScanError::PowerShellNotPorted
    );
}

#[test]
fn shell_selection_metadata_and_args() {
    assert_eq!(select::name("/usr/bin/Bash"), "bash");
    assert!(select::login("/bin/zsh") && select::posix("/bin/sh"));
    assert!(!select::posix("/usr/bin/fish") && select::ps("pwsh"));
    let fallback = select::acceptable(Some("fish"), None);
    assert_ne!(select::name(&fallback), "fish");
    assert_eq!(
        select::acceptable(Some("/definitely/missing/shell"), None),
        fallback
    );
    assert_eq!(select::preferred(Some("/bin/sh"), None), "/bin/sh");
    assert_eq!(
        select::args("/bin/sh", "echo hi", "/tmp"),
        ["-c", "echo hi"]
    );
    assert_eq!(select::args("fish", "echo hi", "/tmp"), ["-c", "echo hi"]);
    assert_eq!(
        select::args("pwsh", "ls", "/tmp"),
        ["-NoProfile", "-Command", "ls"]
    );
    let bash = select::args("/bin/bash", "echo \"x\"", "/w");
    assert_eq!(bash[0], "-l");
    assert!(
        bash[2].contains("shopt -s expand_aliases") && bash[2].contains("eval \"echo \\\"x\\\"\"")
    );
    assert_eq!(&bash[3..], ["opencode", "/w"]);
    let listed = select::list(Some("# comment\n/bin/sh\n/bin/sh\n/no/such/shell\n"));
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].name, "sh");
    assert!(listed[0].acceptable);
    assert_eq!(select::kind("zsh"), "bash");
}
