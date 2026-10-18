//! Purpose: declarative model of the yargs command tree: commands, positionals, options, kinds,
//! defaults and the leaf `CommandId` identifiers dispatch consumes.
//! Scope: pure data, built once per parse.
//! Uses:
//! `-- none
//! External:
//! `-- none
//! Used by:
//! rust/bins/
//! |-- opencode/src/cli/spec/clap_tree.rs
//! |-- opencode/src/cli/spec/complete.rs
//! |-- opencode/src/cli/spec/help.rs
//! |-- opencode/src/cli/spec/invocation.rs
//! |-- opencode/src/cli/spec/mod.rs
//! |-- opencode/src/cli/spec/parse.rs
//! |-- opencode/src/cli/spec/tree.rs
//! |-- opencode/src/cli/spec/tree_debug.rs
//! `-- opencode/src/cli/spec/tree_misc.rs
//! Invariants: `command` strings are the exact yargs `command:` originals (packages/opencode/src/cli/cmd/*.ts),
//! declaration order equals yargs registration order because help output depends on it.
//! Contract tests: opencode/tests/cli_help.rs

/// Value type of an option or positional, mirroring the yargs `type`/`array` settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Bool,
    Str,
    Num,
    /// `type: "string", array: true`.
    Array,
    /// No `type` given (stats `--models`): parse-numbers, bare flag is `true`.
    Any,
}

/// Declared default value, rendered as yargs `defaultString` does.
#[derive(Debug, Clone, PartialEq)]
pub enum Default {
    Bool(bool),
    Num(f64),
    Str(&'static str),
    EmptyList,
    /// `default: process.cwd()` (acp `--cwd`), resolved from the parse context.
    Cwd,
}

#[derive(Debug, Clone)]
pub struct Opt {
    pub name: &'static str,
    pub aliases: Vec<&'static str>,
    pub desc: Option<String>,
    pub kind: Kind,
    pub choices: Vec<&'static str>,
    pub default: Option<Default>,
    pub hidden: bool,
}

impl Opt {
    pub fn new(name: &'static str, kind: Kind, desc: &str) -> Self {
        Opt {
            name,
            aliases: Vec::new(),
            desc: Some(desc.to_string()),
            kind,
            choices: Vec::new(),
            default: None,
            hidden: false,
        }
    }

    /// Hidden option (`hidden: true`), still accepted by the parser.
    pub fn hidden(name: &'static str, kind: Kind) -> Self {
        Opt {
            desc: None,
            hidden: true,
            ..Opt::new(name, kind, "")
        }
    }

    pub fn alias(mut self, alias: &'static str) -> Self {
        self.aliases.push(alias);
        self
    }

    pub fn choices(mut self, choices: &[&'static str]) -> Self {
        self.choices = choices.to_vec();
        self
    }

    pub fn default(mut self, value: Default) -> Self {
        self.default = Some(value);
        self
    }
}

/// Positional details from `.positional(name, {...})`. Order and required/variadic come from the
/// command string (`<x>` required, `[x..]` variadic).
#[derive(Debug, Clone)]
pub struct Positional {
    pub name: &'static str,
    pub desc: Option<&'static str>,
    pub kind: Kind,
    pub required: bool,
    pub variadic: bool,
    pub default: Option<Default>,
    /// Inherited from a parent builder: listed in help but rejected by strict parsing
    /// (`mcp auth list` shows the `name` positional of `mcp auth [name]`).
    pub help_only: bool,
}

/// Leaf commands that run a handler. Group commands with `demandCommand()` have no id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CommandId {
    Tui,
    Completion,
    Acp,
    McpAdd,
    McpList,
    McpAuth,
    McpAuthList,
    McpLogout,
    McpDebug,
    Attach,
    Run,
    Generate,
    DebugConfig,
    DebugLspDiagnostics,
    DebugLspSymbols,
    DebugLspDocumentSymbols,
    DebugRgFiles,
    DebugRgSearch,
    DebugFileRead,
    DebugFileList,
    DebugFileSearch,
    DebugScrap,
    DebugSkill,
    DebugSnapshotTrack,
    DebugSnapshotPatch,
    DebugSnapshotDiff,
    DebugStartup,
    DebugAgent,
    DebugV2,
    DebugInfo,
    DebugPaths,
    DebugWait,
    ConsoleLogin,
    ConsoleLogout,
    ConsoleSwitch,
    ConsoleOrgs,
    ConsoleOpen,
    ProvidersList,
    ProvidersLogin,
    ProvidersLogout,
    AgentCreate,
    AgentList,
    Upgrade,
    Uninstall,
    Serve,
    Web,
    Models,
    Stats,
    Export,
    Import,
    GithubInstall,
    GithubRun,
    Pr,
    SessionList,
    SessionDelete,
    Plugin,
    DbQuery,
    DbPath,
}

#[derive(Debug, Clone)]
pub struct Node {
    /// yargs command original, e.g. `add [name]`, `$0 [project]`; empty for the root.
    pub command: &'static str,
    pub aliases: Vec<&'static str>,
    /// `describe`; `None` means `describe: false`/absent: hidden from lists, no description line.
    pub describe: Option<&'static str>,
    pub id: Option<CommandId>,
    pub positionals: Vec<Positional>,
    pub options: Vec<Opt>,
    pub children: Vec<Node>,
}

impl Node {
    pub fn new(
        command: &'static str,
        describe: Option<&'static str>,
        id: Option<CommandId>,
    ) -> Self {
        Node {
            command,
            aliases: Vec::new(),
            describe,
            id,
            positionals: Vec::new(),
            options: Vec::new(),
            children: Vec::new(),
        }
    }

    /// Command token (first word of the command string), `$0` for default commands.
    pub fn name(&self) -> &'static str {
        self.command.split(' ').next().unwrap_or("")
    }

    pub fn is_default(&self) -> bool {
        self.name() == "$0"
    }

    pub fn default_child(&self) -> Option<&Node> {
        self.children.iter().find(|c| c.is_default())
    }

    pub fn matches(&self, token: &str) -> bool {
        !self.is_default() && (self.name() == token || self.aliases.contains(&token))
    }

    pub fn alias(mut self, alias: &'static str) -> Self {
        self.aliases.push(alias);
        self
    }

    /// Adds a positional; required/variadic are derived from the command string.
    pub fn pos(mut self, name: &'static str, kind: Kind, desc: Option<&'static str>) -> Self {
        let required = self.command.contains(&format!("<{name}"));
        let variadic = self.command.contains(&format!("{name}.."));
        self.positionals.push(Positional {
            name,
            desc,
            kind,
            required,
            variadic,
            default: None,
            help_only: false,
        });
        self
    }

    pub fn pos_default(mut self, value: Default) -> Self {
        if let Some(last) = self.positionals.last_mut() {
            last.default = Some(value);
        }
        self
    }

    pub fn help_only_pos(
        mut self,
        name: &'static str,
        kind: Kind,
        desc: Option<&'static str>,
    ) -> Self {
        self = self.pos(name, kind, desc);
        if let Some(last) = self.positionals.last_mut() {
            last.help_only = true;
        }
        self
    }

    pub fn opt(mut self, opt: Opt) -> Self {
        self.options.push(opt);
        self
    }

    pub fn opts(mut self, opts: Vec<Opt>) -> Self {
        self.options.extend(opts);
        self
    }

    pub fn child(mut self, node: Node) -> Self {
        self.children.push(node);
        self
    }
}

/// Global options registered on the root yargs instance (packages/opencode/src/index.ts:46-65).
pub fn global_options() -> Vec<Opt> {
    vec![
        Opt::new("help", Kind::Bool, "show help").alias("h"),
        Opt::new("version", Kind::Bool, "show version number").alias("v"),
        Opt::new("print-logs", Kind::Bool, "print logs to stderr"),
        Opt::new("log-level", Kind::Str, "log level").choices(&["DEBUG", "INFO", "WARN", "ERROR"]),
        Opt::new("pure", Kind::Bool, "run without external plugins"),
    ]
}
