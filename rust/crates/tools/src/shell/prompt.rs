//! Purpose: legacy shell tool description (packages/opencode/src/tool/shell/prompt.ts + shell.txt):
//! per-shell profile (bash/zsh/sh, PowerShell 5.1/7+, cmd.exe), limits and default timeout.
//! Scope: pure text rendering from bundled baseline templates.
//! Uses:
//! `-- none
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! |-- tools/src/shell/legacy.rs
//! `-- tools/src/shell/mod.rs
//! Invariants: output is byte-identical to the baseline `ShellPrompt.render` (fixtures/prompts.json).
//! Section templates are extracted verbatim from prompt.ts.
//! Contract tests: tools/tests/shell_scan.rs

pub const TEMPLATE: &str = include_str!("descriptions/shell.txt");
const BASH: &str = include_str!("descriptions/section_bash.txt");
const POWERSHELL: &str = include_str!("descriptions/section_powershell.txt");
const CMD: &str = include_str!("descriptions/section_cmd.txt");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_lines: usize,
    pub max_bytes: usize,
}

fn display_name(name: &str) -> &str {
    match name {
        "pwsh" => "PowerShell (7+)",
        "powershell" => "Windows PowerShell (5.1)",
        "cmd" => "cmd.exe",
        other => other,
    }
}

fn powershell_notes(name: &str) -> String {
    let (title, first) = match name {
        "pwsh" => (
            "PowerShell (7+)",
            "- This cross-platform shell supports pipeline chain operators (`&&` and `||`).",
        ),
        "powershell" => (
            "Windows PowerShell (5.1)",
            "- Use `cmd1; if ($?) { cmd2 }` to chain dependent commands.",
        ),
        _ => return String::new(),
    };
    format!(
        "# {title} shell notes\n{first}\n- Use double quotes for interpolated strings (`\"Hello $name\"`), single quotes for verbatim strings.\n- Prefer full cmdlet names like `Get-ChildItem`, `Set-Content`, `Remove-Item`, and `New-Item` over aliases.\n- Use `$(...)` for subexpressions. Use `@(...)` for array expressions.\n- To call a native executable whose path contains spaces, use the call operator: `& \"path/to/exe\" args`.\n- Escape special characters with the PowerShell backtick character."
    )
}

fn chain(name: &str) -> &'static str {
    match name {
        "powershell" => {
            "If the commands depend on each other and must run sequentially, avoid '&&' in this shell because Windows PowerShell (5.1) does not support it. Use PowerShell conditionals such as `cmd1; if ($?) { cmd2 }` when later commands must depend on earlier success."
        }
        "pwsh" => {
            "If the commands depend on each other and must run sequentially, use a single bash tool call with '&&' to chain them together (e.g., `git add . && git commit -m \"message\" && git push`). For instance, if one operation must complete before another starts (like New-Item before Copy-Item, Write before bash for git operations, or git add before git commit), run these operations sequentially instead."
        }
        "cmd" => {
            "If the commands depend on each other and must run sequentially, use a single bash tool call with `&&` to chain them together (e.g., `mkdir out && dir out`). For instance, if one operation must complete before another starts, run these operations sequentially instead."
        }
        _ => {
            "If the commands depend on each other and must run sequentially, use a single Bash call with '&&' to chain them together (e.g., `git add . && git commit -m \"message\" && git push`). For instance, if one operation must complete before another starts (like mkdir before cp, Write before Bash for git operations, or git add before git commit), run these operations sequentially instead."
        }
    }
}

fn section(template: &str, name: &str, limits: Limits, timeout: u64, sep: &str) -> String {
    let bad = if name == "powershell" {
        format!("Set-Location -LiteralPath \"project{sep}subdir\"; if ($?) {{ pytest tests }}")
    } else {
        format!("Set-Location -LiteralPath \"project{sep}subdir\" && pytest tests")
    };
    template
        .replace("{notes}", &powershell_notes(name))
        .replace("{chain}", chain(name))
        .replace("{default_timeout}", &timeout.to_string())
        .replace("{max_lines}", &limits.max_lines.to_string())
        .replace("{max_bytes}", &limits.max_bytes.to_string())
        .replace("{bad_example}", &bad)
        .replace("{sep}", sep)
}

const WORKDIR_OTHER: &str = "All commands run in the current working directory by default. Use the `workdir` parameter if you need to run a command in a different directory. AVOID changing directories inside the command - use `workdir` instead.";

/// `ShellPrompt.render(name, platform, limits, defaultTimeoutMs)` with `tmp` = `Global.Path.tmp`.
/// `platform` uses node names (`darwin`, `linux`, `win32`).
pub fn render(name: &str, platform: &str, tmp: &str, limits: Limits, timeout: u64) -> String {
    let (intro, workdir, commands, git, git_restriction, pr_instruction, pr_example) = match name {
        "cmd" => (
            format!("Executes a given {} command with optional timeout, ensuring proper handling and security measures.", display_name(name)),
            WORKDIR_OTHER,
            section(CMD, name, limits, timeout, "\\"),
            "git commands",
            "git commands",
            "Create PR using a temporary body file so cmd.exe quoting stays simple.",
            "(\n  echo ## Summary\n  echo - ^<1-3 bullet points^>\n) > pr-body.txt\ngh pr create --title \"the pr title\" --body-file pr-body.txt",
        ),
        "pwsh" | "powershell" => (
            format!("Executes a given {} command with optional timeout, ensuring proper handling and security measures.", display_name(name)),
            WORKDIR_OTHER,
            section(POWERSHELL, name, limits, timeout, if platform == "win32" { "\\" } else { "/" }),
            "git commands",
            "git commands",
            "Create PR using gh pr create with a PowerShell here-string to pass the body correctly.",
            "gh pr create --title \"the pr title\" --body @'\n## Summary\n- <1-3 bullet points>\n'@",
        ),
        _ => (
            "Executes a given bash command in a persistent shell session with optional timeout, ensuring proper handling and security measures.".to_string(),
            "All commands run in the current working directory by default. Use the `workdir` parameter if you need to run a command in a different directory. AVOID using `cd <directory> && <command>` patterns - use `workdir` instead.",
            section(BASH, name, limits, timeout, "/"),
            "bash commands",
            "git bash commands",
            "Create PR using gh pr create with the format below. Use a HEREDOC to pass the body to ensure correct formatting.",
            "gh pr create --title \"the pr title\" --body \"$(cat <<'EOF'\n## Summary\n<1-3 bullet points>",
        ),
    };
    // Single pass over `${key}` placeholders, like the baseline `renderPrompt`.
    let values = [
        ("intro", intro.as_str()),
        ("os", platform),
        ("shell", name),
        ("tmp", tmp),
        ("workdirSection", workdir),
        ("commandSection", commands.as_str()),
        ("gitCommands", git),
        ("toolName", "bash"),
        ("gitCommandRestriction", git_restriction),
        ("createPrInstruction", pr_instruction),
        ("createPrExample", pr_example),
    ];
    let mut out = String::with_capacity(TEMPLATE.len() + commands.len());
    let mut rest = TEMPLATE;
    while let Some(start) = rest.find("${") {
        let Some(end) = rest[start..].find('}') else {
            break;
        };
        let key = &rest[start + 2..start + end];
        let value = values.iter().find(|(name, _)| *name == key);
        out.push_str(&rest[..start]);
        match value {
            Some((_, value)) => out.push_str(value),
            None => out.push_str(&rest[start..start + end + 1]),
        }
        rest = &rest[start + end + 1..];
    }
    out.push_str(rest);
    out
}
