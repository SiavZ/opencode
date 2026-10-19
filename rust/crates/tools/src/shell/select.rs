//! Purpose: shell discovery and invocation (packages/core/src/shell.ts): name/login/posix/ps
//! metadata, configured-shell resolution with deny-list fallback, `/etc/shells` listing and
//! per-family argument vectors.
//! Scope: POSIX resolution plus portable metadata. Windows Git Bash/PowerShell resolution is a
//! typed gap recorded in status.md.
//! Uses:
//! `-- none
//! External:
//! |-- serde_json::to_string
//! |-- str::to_string
//! `-- which::which
//! Used by:
//! rust/crates/
//! |-- tools/src/shell/core.rs
//! |-- tools/src/shell/legacy.rs
//! |-- tools/src/shell/mod.rs
//! |-- tools/src/shell/scan.rs
//! `-- tools/tests/shell_scan.rs
//! Invariants: fish and nu are never "acceptable"; fallback is /bin/zsh on macOS, else bash on
//! PATH, else /bin/sh.
//! Contract tests: tools/tests/shell_scan.rs

use std::path::Path;

struct Meta {
    deny: bool,
    login: bool,
    posix: bool,
    ps: bool,
}

fn meta(name: &str) -> Option<Meta> {
    let (deny, login, posix, ps) = match name {
        "bash" | "dash" | "ksh" | "sh" | "zsh" => (false, true, true, false),
        "fish" => (true, true, false, false),
        "nu" => (true, false, false, false),
        "powershell" | "pwsh" => (false, false, false, true),
        _ => return None,
    };
    Some(Meta {
        deny,
        login,
        posix,
        ps,
    })
}

/// `Shell.name`: lowercase basename (Windows: stem without extension).
pub fn name(file: &str) -> String {
    let base = Path::new(file)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| file.to_string());
    if cfg!(windows) {
        let stem = Path::new(&base)
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or(base);
        return stem.to_lowercase();
    }
    base.to_lowercase()
}

pub fn login(file: &str) -> bool {
    meta(&name(file)).is_some_and(|meta| meta.login)
}

pub fn posix(file: &str) -> bool {
    meta(&name(file)).is_some_and(|meta| meta.posix)
}

pub fn ps(file: &str) -> bool {
    meta(&name(file)).is_some_and(|meta| meta.ps)
}

fn ok(file: &str) -> bool {
    !meta(&name(file)).is_some_and(|meta| meta.deny)
}

fn resolve(file: &str) -> Option<String> {
    if Path::new(file).is_absolute() {
        return Path::new(file).is_file().then(|| file.to_string());
    }
    which::which(file)
        .ok()
        .map(|path| path.to_string_lossy().into_owned())
}

fn fallback() -> String {
    if cfg!(target_os = "macos") {
        return "/bin/zsh".into();
    }
    resolve("bash").unwrap_or_else(|| "/bin/sh".into())
}

fn select(file: Option<&str>, acceptable: bool) -> String {
    if let Some(file) = file
        && (!acceptable || ok(file))
        && let Some(shell) = resolve(file)
    {
        return shell;
    }
    fallback()
}

/// `Shell.preferred(configShell)`; `env_shell` is `$SHELL` when no config is set.
pub fn preferred(config: Option<&str>, env_shell: Option<&str>) -> String {
    select(config.or(env_shell), false)
}

/// `Shell.acceptable(configShell)`: like preferred but skips terminal-only shells (fish, nu).
pub fn acceptable(config: Option<&str>, env_shell: Option<&str>) -> String {
    select(config.or(env_shell), true)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub path: String,
    pub name: String,
    pub acceptable: bool,
}

/// `Shell.list` on POSIX: `/etc/shells` entries that resolve, else the default trio.
pub fn list(etc_shells: Option<&str>) -> Vec<Item> {
    let mut seen = Vec::<String>::new();
    let shells: Vec<String> = match etc_shells.filter(|text| !text.is_empty()) {
        Some(text) => text
            .split('\n')
            .filter(|line| !line.trim().is_empty() && !line.starts_with('#'))
            .map(str::to_string)
            .collect(),
        None => ["/bin/bash", "/bin/zsh", "/bin/sh"]
            .map(String::from)
            .into(),
    };
    for shell in shells {
        if !seen.contains(&shell) {
            seen.push(shell);
        }
    }
    seen.into_iter()
        .filter(|shell| resolve(shell).is_some())
        .map(|shell| {
            let short = name(&shell);
            Item {
                name: if resolve(&short).is_some() {
                    short
                } else {
                    shell.clone()
                },
                acceptable: ok(&shell),
                path: shell,
            }
        })
        .collect()
}

/// `Shell.args(file, command, cwd)`.
pub fn args(file: &str, command: &str, cwd: &str) -> Vec<String> {
    let kind = name(file);
    let quoted = serde_json::to_string(command).unwrap_or_default();
    let login_script = |rc: &str| -> Vec<String> {
        vec![
            "-l".into(),
            "-c".into(),
            format!("{rc}\n        cd -- \"$1\"\n        eval {quoted}\n      "),
            "opencode".into(),
            cwd.into(),
        ]
    };
    match kind.as_str() {
        "nu" | "fish" => vec!["-c".into(), command.into()],
        "zsh" => login_script(
            "\n        [[ -f ~/.zshenv ]] && source ~/.zshenv >/dev/null 2>&1 || true\n        [[ -f \"${ZDOTDIR:-$HOME}/.zshrc\" ]] && source \"${ZDOTDIR:-$HOME}/.zshrc\" >/dev/null 2>&1 || true",
        ),
        "bash" => login_script(
            "\n        shopt -s expand_aliases\n        [[ -f ~/.bashrc ]] && source ~/.bashrc >/dev/null 2>&1 || true",
        ),
        "cmd" => vec!["/c".into(), command.into()],
        _ if ps(file) => vec!["-NoProfile".into(), "-Command".into(), command.into()],
        _ => vec!["-c".into(), command.into()],
    }
}

/// `ShellID.toKind`: the shell kind used by permission scanning.
pub fn kind(name: &str) -> &'static str {
    match name {
        "pwsh" => "pwsh",
        "powershell" => "powershell",
        "cmd" => "cmd",
        _ => "bash",
    }
}
