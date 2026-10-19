//! Purpose: legacy `opencode` argv parser reproducing yargs 18 command resolution, strict checks,
//! `.fail()` behaviour (help to stderr, exit 1), help/version short-circuits and `populate--`.
//! Scope: pure function from argv to `Invocation`.
//! Uses:
//! rust/bins/
//! |-- opencode/src/cli/spec/argv.rs
//! |-- opencode/src/cli/spec/complete.rs
//! |-- opencode/src/cli/spec/help.rs
//! |-- opencode/src/cli/spec/invocation.rs
//! |-- opencode/src/cli/spec/model.rs
//! `-- opencode/src/cli/spec/tree.rs
//! External:
//! |-- complete::reply
//! |-- complete::script
//! |-- help::render
//! |-- help::show
//! |-- tree::root
//! `-- u8::from
//! Used by:
//! rust/bins/
//! |-- opencode/src/cli/spec/mod.rs
//! `-- opencode/src/serve.rs
//! Invariants: no failure message is printed for the three handled yargs failures (index.ts:105-116);
//! help wins over version; options are not inherited from parent commands except the globals.
//! Contract tests: opencode/tests/cli_parse.rs, opencode/tests/cli_help.rs
use super::argv::{js_number, tokenize};
use super::complete;
use super::help::{self, Context};
use super::invocation::{Args, CommandId, Globals, Invocation, Parsed, Value};
use super::model::{Default, Kind, Node, Opt, global_options};
use super::tree;

/// Process facts that influence parsing output.
#[derive(Debug, Clone)]
pub struct ParseEnv {
    /// `InstallationVersion` (`local` for development builds).
    pub version: String,
    /// `process.cwd()`, default of `acp --cwd`.
    pub cwd: String,
    /// stdout or stderr is a TTY (colored logo).
    pub tty: bool,
    /// `$SHELL`/`$ZSH_NAME` mention zsh (yargs completion flavour).
    pub zsh: bool,
}

pub fn parse(argv: &[String], env: &ParseEnv) -> Invocation {
    let root = tree::root();
    if let Some(at) = argv.iter().position(|a| a == "--get-yargs-completions") {
        return Invocation::Completions(complete::reply(&root, &argv[at + 1..], env.zsh));
    }
    let (before, double_dash) = match argv.iter().position(|a| a == "--") {
        Some(at) => (&argv[..at], argv[at + 1..].to_vec()),
        None => (argv, Vec::new()),
    };
    let resolved = resolve(&root, before);
    let ctx = Context {
        tokens: resolved.tokens.clone(),
        chain: resolved.chain.clone(),
        default: resolved.default,
        cwd: env.cwd.clone(),
    };
    let leaf = resolved
        .default
        .unwrap_or_else(|| *resolved.chain.last().expect("root"));
    let opts: Vec<Opt> = global_options()
        .into_iter()
        .chain(leaf.options.iter().cloned())
        .collect();
    let parsed = tokenize(before, &opts);
    let help_text = || help::show(&help::render(&ctx), env.tty);
    if parsed.help {
        // `completion` is not a real command handler: help renders the root and exits 1.
        let completion = before
            .iter()
            .find(|a| !a.starts_with('-'))
            .is_some_and(|a| a == "completion");
        return Invocation::Help {
            stderr: help_text(),
            code: u8::from(completion),
        };
    }
    if parsed.version {
        return Invocation::Version(format!("{}\n", env.version));
    }
    if resolved.tokens.is_empty()
        && parsed
            .positionals
            .first()
            .is_some_and(|p| p == "completion")
    {
        return Invocation::CompletionScript(complete::script(env.zsh));
    }
    let fail = || Invocation::Fail {
        stderr: help_text(),
    };
    let Some(id) = leaf.id else { return fail() };
    if id == CommandId::Completion {
        return Invocation::CompletionScript(complete::script(env.zsh));
    }
    if !parsed.unknown.is_empty() || !choices_ok(&parsed.values, &opts) {
        return fail();
    }
    let free = &parsed.positionals[resolved.tokens.len()..];
    let Some(values) = positionals(leaf, free) else {
        return fail();
    };
    let mut args = Args {
        values: parsed.values,
    };
    args.values.extend(values);
    apply_defaults(&mut args, leaf, &env.cwd);
    let globals = Globals {
        print_logs: args.bool("print-logs"),
        log_level: args.str("log-level"),
        pure: args.bool("pure"),
    };
    let mut path: Vec<&'static str> = resolved.chain[1..].iter().map(|n| n.name()).collect();
    if resolved
        .default
        .is_some_and(|d| d.id != Some(CommandId::Tui))
    {
        path.push("$0");
    }
    Invocation::Run(Parsed {
        command: id,
        path,
        globals,
        args,
        double_dash,
        raw: argv.to_vec(),
    })
}

#[derive(Debug)]
struct Resolved<'a> {
    tokens: Vec<String>,
    chain: Vec<&'a Node>,
    default: Option<&'a Node>,
}

/// Walks command words: at each level the remaining argv is tokenized with that level's options and
/// the next free positional selects a child (yargs `runYargsParserAndExecuteCommands`).
fn resolve<'a>(root: &'a Node, argv: &[String]) -> Resolved<'a> {
    let mut chain = vec![root];
    let mut tokens = Vec::new();
    loop {
        let node = *chain.last().expect("root");
        let level_opts: Vec<Opt> = global_options()
            .into_iter()
            .chain(node.options.iter().cloned())
            .collect();
        let free = tokenize(argv, &level_opts).positionals;
        let next = free.get(tokens.len()).and_then(|word| {
            node.children
                .iter()
                .find(|c| c.matches(word))
                .map(|c| (word.clone(), c))
        });
        match next {
            Some((word, child)) if child.id != Some(CommandId::Completion) => {
                tokens.push(word);
                chain.push(child);
            }
            _ => {
                let default = node.default_child();
                return Resolved {
                    tokens,
                    chain,
                    default,
                };
            }
        }
    }
}

fn choices_ok(values: &std::collections::BTreeMap<String, Value>, opts: &[Opt]) -> bool {
    opts.iter()
        .filter(|o| !o.choices.is_empty())
        .all(|o| match values.get(o.name) {
            None => true,
            Some(Value::List(items)) => items
                .iter()
                .all(|v| matches!(v, Value::Str(s) if o.choices.contains(&s.as_str()))),
            Some(Value::Str(s)) => o.choices.contains(&s.as_str()),
            Some(_) => false,
        })
}

/// Binds free positionals; `None` on missing required or surplus arguments.
fn positionals(leaf: &Node, free: &[String]) -> Option<Vec<(String, Value)>> {
    let declared: Vec<_> = leaf.positionals.iter().filter(|p| !p.help_only).collect();
    let required = declared.iter().filter(|p| p.required).count();
    let variadic = declared.iter().any(|p| p.variadic);
    if free.len() < required || (!variadic && free.len() > declared.len()) {
        return None;
    }
    let mut out = Vec::new();
    for (idx, p) in declared.iter().enumerate() {
        let coerce = |raw: &String| match p.kind {
            Kind::Num => js_number(raw),
            _ => Value::Str(raw.clone()),
        };
        if p.variadic {
            let rest: Vec<Value> = free.iter().skip(idx).map(coerce).collect();
            if !rest.is_empty() {
                out.push((p.name.to_string(), Value::List(rest)));
            }
            break;
        }
        if let Some(raw) = free.get(idx) {
            out.push((p.name.to_string(), coerce(raw)));
        }
    }
    Some(out)
}

fn apply_defaults(args: &mut Args, leaf: &Node, cwd: &str) {
    let opts = leaf.options.iter().map(|o| (o.name, o.default.as_ref()));
    let pos = leaf
        .positionals
        .iter()
        .map(|p| (p.name, p.default.as_ref()));
    for (name, default) in opts.chain(pos) {
        let Some(default) = default else { continue };
        if args.has(name) {
            continue;
        }
        let value = match default {
            Default::Bool(b) => Value::Bool(*b),
            Default::Num(n) => Value::Num(*n),
            Default::Str(s) => Value::Str(s.to_string()),
            Default::EmptyList => Value::List(Vec::new()),
            Default::Cwd => Value::Str(cwd.to_string()),
        };
        args.values.insert(name.to_string(), value);
    }
}
