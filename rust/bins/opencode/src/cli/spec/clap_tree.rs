//! Purpose: projection of the yargs model tree into a clap 4 `Command` so clap tooling
//! (clap_complete, introspection) sees every command, alias, option, choice and default.
//! Scope: pure data conversion.
//! Uses:
//! rust/bins/
//! `-- opencode/src/cli/spec/model.rs
//! External:
//! |-- clap::Arg
//! |-- clap::ArgAction
//! `-- clap::Command
//! Used by:
//! rust/bins/
//! `-- opencode/src/cli/spec/mod.rs
//! Invariants: parsing is not done with clap (yargs semantics differ); this tree is for completion
//! and structure only. `$0` default commands fold their args into the parent.
//! Contract tests: opencode/tests/cli_parse.rs
use clap::{Arg, ArgAction, Command};

use super::model::{Kind, Node, Opt, global_options};

pub fn build(root: &Node) -> Command {
    let mut cmd = Command::new("opencode")
        .disable_help_flag(true)
        .disable_version_flag(true);
    for opt in global_options() {
        cmd = cmd.arg(arg(&opt).global(true));
    }
    node(cmd, root)
}

fn node(mut cmd: Command, n: &Node) -> Command {
    let target = n.default_child().unwrap_or(n);
    for opt in target.options.iter() {
        cmd = cmd.arg(arg(opt));
    }
    for p in target.positionals.iter().filter(|p| !p.help_only) {
        let mut a = Arg::new(p.name).required(p.required);
        if p.variadic {
            a = a.num_args(0..);
        }
        if let Some(d) = p.desc {
            a = a.help(d);
        }
        cmd = cmd.arg(a);
    }
    for child in n.children.iter().filter(|c| !c.is_default()) {
        let mut sub = Command::new(child.name()).hide(child.describe.is_none());
        if let Some(d) = child.describe {
            sub = sub.about(d);
        }
        for alias in &child.aliases {
            sub = sub.visible_alias(*alias);
        }
        cmd = cmd.subcommand(node(sub, child));
    }
    cmd
}

fn arg(o: &Opt) -> Arg {
    let mut a = Arg::new(o.name).long(o.name).hide(o.hidden);
    for alias in &o.aliases {
        let mut chars = alias.chars();
        a = match (chars.next(), chars.next()) {
            (Some(c), None) => a.short(c),
            _ => a.visible_alias(*alias),
        };
    }
    if let Some(d) = &o.desc {
        a = a.help(d.clone());
    }
    a = match o.kind {
        Kind::Bool => a.action(ArgAction::SetTrue),
        Kind::Array => a.action(ArgAction::Append).num_args(0..),
        _ => a.action(ArgAction::Set),
    };
    if !o.choices.is_empty() {
        a = a.value_parser(o.choices.clone());
    }
    a
}
