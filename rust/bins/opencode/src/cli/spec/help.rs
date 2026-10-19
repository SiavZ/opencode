//! Purpose: yargs 18 help renderer (yargs/build/lib/usage.js `help()`) and the `show()` wrapper
//! of packages/opencode/src/index.ts:34-43 that prefixes the logo when output lacks a usage line.
//! Scope: pure text rendering for one parse context.
//! Uses:
//! rust/bins/
//! |-- opencode/src/cli/spec/cliui.rs
//! `-- opencode/src/cli/spec/model.rs
//! External:
//! |-- serde_json::to_string
//! `-- str::trim_start
//! Used by:
//! rust/bins/
//! |-- opencode/src/cli/spec/mod.rs
//! `-- opencode/src/cli/spec/parse.rs
//! Invariants: wrap is 100 columns, group order is Positionals then Options, option order is
//! globals then builder order, hidden options and `describe: false` commands never render.
//! Contract tests: opencode/tests/cli_help.rs
use super::cliui::{Col, Ui};
use super::model::{Default, Kind, Node, Opt, Positional, global_options};

const WRAP: usize = 100;
const SCRIPT: &str = "opencode";

/// Help context: the typed command tokens and the chain of nodes they resolved to.
#[derive(Debug)]
pub struct Context<'a> {
    pub tokens: Vec<String>,
    /// Root first, then one node per token.
    pub chain: Vec<&'a Node>,
    /// Default child selected for options/positionals (`$0 [project]`, `db $0 [query]`).
    pub default: Option<&'a Node>,
    pub cwd: String,
}

impl Context<'_> {
    fn node(&self) -> &Node {
        self.chain.last().copied().expect("chain starts at root")
    }

    /// Node whose positionals and options are in effect.
    fn leaf(&self) -> &Node {
        self.default.unwrap_or_else(|| self.node())
    }
}

pub fn render(ctx: &Context) -> String {
    let mut ui = Ui::new(WRAP);
    usage(ctx, &mut ui);
    commands(ctx, &mut ui);
    groups(ctx, &mut ui);
    ui.render().trim_end().to_string()
}

/// `show(out)`: logo + text when the help has no `opencode ...` usage line, raw text otherwise.
pub fn show(text: &str, tty: bool) -> String {
    let trimmed = text.trim_start();
    if trimmed.starts_with("opencode ") {
        return text.to_string();
    }
    format!("{}\n\n{}\n", logo(tty), trimmed)
}

fn usage(ctx: &Context, ui: &mut Ui) {
    let node = ctx.node();
    if ctx.chain.len() == 1 {
        // Root: `.usage("")` (index.ts:79) renders one empty usage line.
        ui.div(vec![Col::text("")]);
        ui.div(vec![]);
        return;
    }
    let parents = &ctx.tokens[..ctx.tokens.len() - 1];
    let mut line = String::from(SCRIPT);
    for token in parents {
        line.push(' ');
        line.push_str(token);
    }
    line.push(' ');
    line.push_str(node.command);
    ui.div(vec![Col::text(line)]);
    if let Some(desc) = node.describe {
        ui.div(vec![Col {
            padding: [1, 0, 0, 0],
            ..Col::text(desc)
        }]);
    }
    ui.div(vec![]);
}

fn commands(ctx: &Context, ui: &mut Ui) {
    let listed: Vec<&Node> = ctx
        .node()
        .children
        .iter()
        .filter(|c| c.describe.is_some())
        .collect();
    if !(listed.len() > 1 || (listed.len() == 1 && !listed[0].is_default())) {
        return;
    }
    ui.div(vec![Col::text("Commands:")]);
    let parents: String = ctx.tokens.iter().map(|t| format!("{t} ")).collect();
    let modifier = format!("{SCRIPT}{parents}");
    let max = listed
        .iter()
        .map(|c| width(&format!("{modifier} {}", c.command)))
        .max()
        .unwrap_or(0);
    let col_width = max.min(WRAP / 2) + 4;
    for child in listed {
        let shown = child
            .command
            .strip_prefix("$0")
            .map(str::trim_start)
            .unwrap_or(child.command);
        let text = format!("{SCRIPT} {parents}{shown}");
        ui.span(vec![
            Col {
                padding: [0, 2, 0, 2],
                width: Some(col_width),
                ..Col::text(text)
            },
            Col::text(child.describe.unwrap_or("")),
        ]);
        let mut hints = Vec::new();
        if child.is_default() {
            hints.push("[default]".to_string());
        }
        if !child.aliases.is_empty() {
            hints.push(format!("[aliases: {}]", child.aliases.join(", ")));
        }
        hint_row(ui, hints.join(" "));
    }
    ui.div(vec![]);
}

fn hint_row(ui: &mut Ui, text: String) {
    if text.is_empty() {
        ui.div(vec![]);
        return;
    }
    ui.div(vec![Col {
        padding: [0, 0, 0, 2],
        align_right: true,
        ..Col::text(text)
    }]);
}

struct Entry {
    switch: String,
    indent: usize,
    desc: String,
    extra: String,
}

fn groups(ctx: &Context, ui: &mut Ui) {
    let leaf = ctx.leaf();
    let positionals: Vec<Entry> = leaf
        .positionals
        .iter()
        .filter(|p| p.desc.is_some() || p.required || p.default.is_some())
        .map(|p| positional_entry(p, &ctx.cwd))
        .collect();
    let globals = global_options();
    let options: Vec<&Opt> = globals
        .iter()
        .chain(leaf.options.iter())
        .filter(|o| !o.hidden && (o.desc.is_some() || o.default.is_some()))
        .collect();
    let short_used = options
        .iter()
        .any(|o| o.aliases.iter().any(|a| a.len() == 1));
    let options: Vec<Entry> = options
        .iter()
        .map(|o| option_entry(o, short_used, &ctx.cwd))
        .collect();
    for (name, entries) in [("Positionals:", positionals), ("Options:", options)] {
        if entries.is_empty() {
            continue;
        }
        ui.div(vec![Col::text(name)]);
        let max = entries
            .iter()
            .map(|e| width(&e.switch) + e.indent)
            .max()
            .unwrap_or(0);
        let col_width = max.min(WRAP / 2) + 4;
        for entry in entries {
            ui.span(vec![
                Col {
                    padding: [0, 2, 0, 2 + entry.indent],
                    width: Some(col_width),
                    ..Col::text(entry.switch)
                },
                Col::text(entry.desc),
            ]);
            hint_row(ui, entry.extra);
        }
        ui.div(vec![]);
    }
}

fn positional_entry(p: &Positional, cwd: &str) -> Entry {
    let extra = [
        Some(type_tag(p.kind)),
        p.required.then(|| "[required]".to_string()),
        p.default.as_ref().map(|d| default_tag(d, cwd)),
    ];
    Entry {
        switch: p.name.to_string(),
        indent: 0,
        desc: p.desc.unwrap_or("").to_string(),
        extra: join(extra),
    }
}

fn option_entry(o: &Opt, short_used: bool, cwd: &str) -> Entry {
    let mut switches: Vec<String> = std::iter::once(o.name)
        .chain(o.aliases.iter().copied())
        .map(|s| {
            if s.len() > 1 {
                format!("--{s}")
            } else {
                format!("-{s}")
            }
        })
        .collect();
    // Stable sort: short switches before long ones (usage.js:284-288).
    switches.sort_by_key(|s| s.starts_with("--"));
    let long_only = switches.iter().all(|s| s.starts_with("--"));
    let choices = (!o.choices.is_empty()).then(|| {
        let quoted: Vec<String> = o.choices.iter().map(|c| format!("\"{c}\"")).collect();
        format!("[choices: {}]", quoted.join(", "))
    });
    let extra = [
        (o.kind != Kind::Any).then(|| type_tag(o.kind)),
        choices,
        o.default.as_ref().map(|d| default_tag(d, cwd)),
    ];
    Entry {
        switch: switches.join(", "),
        indent: if short_used && long_only { 4 } else { 0 },
        desc: o.desc.clone().unwrap_or_default(),
        extra: join(extra),
    }
}

fn join<const N: usize>(parts: [Option<String>; N]) -> String {
    parts.into_iter().flatten().collect::<Vec<_>>().join(" ")
}

fn type_tag(kind: Kind) -> String {
    match kind {
        Kind::Bool => "[boolean]",
        Kind::Str => "[string]",
        Kind::Num => "[number]",
        Kind::Array => "[array]",
        Kind::Any => "",
    }
    .to_string()
}

/// `defaultString` (usage.js:480-499).
fn default_tag(value: &Default, cwd: &str) -> String {
    let text = match value {
        Default::Bool(b) => b.to_string(),
        Default::Num(n) => js_number(*n),
        Default::Str(s) => format!("\"{s}\""),
        Default::EmptyList => "[]".to_string(),
        Default::Cwd => serde_json::to_string(cwd).unwrap_or_default(),
    };
    format!("[default: {text}]")
}

/// JS `String(number)` for the integral defaults used by the CLI.
fn js_number(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e21 {
        return format!("{}", n as i64);
    }
    n.to_string()
}

fn width(s: &str) -> usize {
    s.chars().count()
}

const LOGO_LEFT: [&str; 4] = [
    "                   ",
    "█▀▀█ █▀▀█ █▀▀█ █▀▀▄",
    "█__█ █__█ █^^^ █__█",
    "▀▀▀▀ █▀▀▀ ▀▀▀▀ ▀~~▀",
];
const LOGO_RIGHT: [&str; 4] = [
    "             ▄     ",
    "█▀▀▀ █▀▀█ █▀▀█ █▀▀█",
    "█___ █__█ █__█ █^^^",
    "▀▀▀▀ ▀▀▀▀ ▀▀▀▀ ▀▀▀▀",
];
const WORDMARK: [&str; 4] = [
    "⠀                                ▄     ",
    "█▀▀█ █▀▀█ █▀▀█ █▀▀▄ █▀▀▀ █▀▀█ █▀▀█ █▀▀█",
    "█  █ █  █ █▀▀▀ █  █ █    █  █ █  █ █▀▀▀",
    "▀▀▀▀ █▀▀▀ ▀▀▀▀ ▀  ▀ ▀▀▀▀ ▀▀▀▀ ▀▀▀▀ ▀▀▀▀",
];

/// `UI.logo()` (packages/opencode/src/cli/ui.ts:48-94): plain wordmark unless stdout or stderr is a TTY.
pub fn logo(tty: bool) -> String {
    if !tty {
        return WORDMARK.join("\n").trim_end().to_string();
    }
    const RESET: &str = "\x1b[0m";
    let draw = |line: &str, fg: &str, shadow: &str, bg: &str| -> String {
        line.chars()
            .map(|c| match c {
                '_' => format!("{bg} {RESET}"),
                '^' => format!("{fg}{bg}▀{RESET}"),
                '~' => format!("{shadow}▀{RESET}"),
                ' ' => " ".to_string(),
                c => format!("{fg}{c}{RESET}"),
            })
            .collect()
    };
    let mut out = String::new();
    for (i, left) in LOGO_LEFT.iter().enumerate() {
        out.push_str(&draw(left, "\x1b[90m", "\x1b[38;5;235m", "\x1b[48;5;235m"));
        out.push(' ');
        out.push_str(&draw(
            LOGO_RIGHT[i],
            RESET,
            "\x1b[38;5;238m",
            "\x1b[48;5;238m",
        ));
        out.push('\n');
    }
    out.trim_end().to_string()
}
