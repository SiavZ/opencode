//! Purpose: shell completion: the yargs `completion` script (zsh/bash templates from
//! yargs/build/lib/completion-templates.js) and the `--get-yargs-completions` reply protocol.
//! Scope: pure text generation.
//! Uses:
//! rust/bins/
//! `-- opencode/src/cli/spec/model.rs
//! External:
//! `-- none
//! Used by:
//! rust/bins/
//! |-- opencode/src/cli/spec/mod.rs
//! `-- opencode/src/cli/spec/parse.rs
//! Invariants: script text is byte-identical to the yargs template for app name `opencode`.
//! Contract tests: opencode/tests/cli_help.rs
use super::model::{Node, Opt, global_options};

const ZSH: &str = include_str!("completion.zsh");
const BASH: &str = include_str!("completion.bash");

/// `opencode completion` output (`generateCompletionScript`).
pub fn script(zsh: bool) -> String {
    if zsh {
        ZSH.to_string()
    } else {
        BASH.to_string()
    }
}

/// `--get-yargs-completions <words...>` reply, one completion per line.
pub fn reply(root: &Node, words: &[String], zsh: bool) -> String {
    // First word is the program name.
    let args: Vec<&str> = words.iter().skip(1).map(String::as_str).collect();
    let current = args.last().copied().unwrap_or("");
    let mut node = root;
    // yargs descends into commands by handler name only (no aliases), re-running their builders.
    for (idx, word) in args.iter().enumerate() {
        if let Some(child) = node
            .children
            .iter()
            .find(|c| !c.is_default() && c.name() == *word)
        {
            node = child;
            if idx + 1 == args.len() {
                // Current word is the command itself: nothing to complete.
                return String::new();
            }
        }
    }
    // The root's default command builder does not run during completion.
    let opts: Vec<Opt> = global_options()
        .into_iter()
        .chain(node.options.iter().cloned())
        .collect();
    let mut out: Vec<String> = Vec::new();
    if let Some(choices) = previous_choices(&args, &opts) {
        return lines(choices.iter().map(|c| c.to_string()).collect());
    }
    if !current.starts_with('-') {
        for child in node.children.iter().filter(|c| c.describe.is_some()) {
            if args.contains(&child.name()) {
                continue;
            }
            out.push(entry(child.name(), child.describe.unwrap_or(""), zsh));
        }
    }
    if current.starts_with('-') || (current.is_empty() && out.is_empty()) {
        for opt in opts.iter().filter(|o| !o.hidden) {
            let used = std::iter::once(opt.name)
                .chain(opt.aliases.iter().copied())
                .any(|k| {
                    let flag = if k.len() == 1 {
                        format!("-{k}")
                    } else {
                        format!("--{k}")
                    };
                    args[..args.len().saturating_sub(1)].contains(&flag.as_str())
                });
            if used {
                continue;
            }
            let desc = opt.desc.clone().unwrap_or_default();
            // help/version aliases are registered via `.alias()` on the root and not listed.
            let aliases: &[&str] = if matches!(opt.name, "help" | "version") {
                &[]
            } else {
                &opt.aliases
            };
            for key in std::iter::once(opt.name).chain(aliases.iter().copied()) {
                let dashes = if !current.starts_with("--") && key.len() == 1 {
                    "-"
                } else {
                    "--"
                };
                out.push(format!("{dashes}{}", entry(key, &desc, zsh)));
            }
        }
    }
    lines(out)
}

fn previous_choices<'a>(args: &[&str], opts: &'a [Opt]) -> Option<&'a [&'static str]> {
    let prev = args.len().checked_sub(2).map(|i| args[i])?;
    let key = prev.strip_prefix('-')?.trim_start_matches('-');
    let opt = opts
        .iter()
        .find(|o| o.name == key || o.aliases.contains(&key))?;
    (!opt.choices.is_empty()).then_some(opt.choices.as_slice())
}

fn entry(name: &str, desc: &str, zsh: bool) -> String {
    if zsh {
        format!("{}:{}", name.replace(':', "\\:"), desc.replace('\n', " "))
    } else {
        name.to_string()
    }
}

fn lines(items: Vec<String>) -> String {
    items.into_iter().map(|l| l + "\n").collect()
}
