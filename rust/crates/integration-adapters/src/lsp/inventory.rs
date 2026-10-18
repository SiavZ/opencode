//! Purpose: the 38 builtin LSP server definitions (id, extensions, root rule) in baseline order,
//! plus config merge semantics (`lsp: false|true|{...}`, disabled, overrides, custom servers,
//! experimental ty/pyright filter). Ports packages/opencode/src/lsp/server.ts (Info rows) and
//! packages/opencode/src/lsp/lsp.ts:96-197.
//! Scope: pure data and merge, no process work (see launch.rs / resolve.rs).
//! Uses:
//! rust/crates/
//! |-- integration-adapters/src/lsp/roots.rs
//! `-- integration-api/src/language.rs
//! External:
//! `-- serde_json::{Map, Value}
//! Used by:
//! rust/crates/
//! |-- integration-adapters/src/lsp/resolve.rs
//! |-- integration-adapters/src/lsp/service.rs
//! `-- integration-adapters/tests/lsp_inventory.rs
//! Invariants: ids and order equal `BUILTIN_SERVER_IDS`, an override keeps the builtin's position
//! and root, custom ids are appended, a custom entry without extensions matches every file.
//! Contract tests: integration-adapters/tests/lsp_inventory.rs

use std::collections::BTreeMap;

use opencode_integration_api::language::{LspConfig, LspServerEntry};
use serde_json::{Map, Value};

use super::roots::RootRule;

const JS_LOCKS: &[&str] = &[
    "package-lock.json",
    "bun.lockb",
    "bun.lock",
    "pnpm-lock.yaml",
    "yarn.lock",
];
const DOTNET: &[&str] = &[".slnx", ".sln", ".csproj", "global.json"];
const PY: &[&str] = &[
    "pyproject.toml",
    "setup.py",
    "setup.cfg",
    "requirements.txt",
    "Pipfile",
    "pyrightconfig.json",
];
const TS_EXT: &[&str] = &[".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs", ".mts", ".cts"];

const fn near(include: &'static [&'static str]) -> RootRule {
    RootRule::Nearest {
        include,
        exclude: &[],
    }
}

/// How a server's command is found. Builtins dispatch on `id` in resolve.rs.
#[derive(Debug, Clone, PartialEq)]
pub enum Launch {
    Builtin,
    Custom {
        command: Vec<String>,
        env: BTreeMap<String, String>,
        initialization: Option<Map<String, Value>>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ServerDef {
    pub id: String,
    pub extensions: Vec<String>,
    pub root: RootRule,
    pub launch: Launch,
}

fn builtin(id: &str, extensions: &[&str], root: RootRule) -> ServerDef {
    ServerDef {
        id: id.to_string(),
        extensions: extensions.iter().map(|s| s.to_string()).collect(),
        root,
        launch: Launch::Builtin,
    }
}

/// Every builtin `LSPServer.Info` in server.ts export order.
pub fn builtins() -> Vec<ServerDef> {
    vec![
        builtin(
            "deno",
            &[".ts", ".tsx", ".js", ".jsx", ".mjs"],
            RootRule::Strict {
                include: &["deno.json", "deno.jsonc"],
            },
        ),
        builtin(
            "typescript",
            TS_EXT,
            RootRule::Nearest {
                include: JS_LOCKS,
                exclude: &["deno.json", "deno.jsonc"],
            },
        ),
        builtin("vue", &[".vue"], near(JS_LOCKS)),
        builtin(
            "eslint",
            &[
                ".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs", ".mts", ".cts", ".vue",
            ],
            near(JS_LOCKS),
        ),
        builtin(
            "oxlint",
            &[
                ".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs", ".mts", ".cts", ".vue", ".astro",
                ".svelte",
            ],
            near(&[
                ".oxlintrc.json",
                "package-lock.json",
                "bun.lockb",
                "bun.lock",
                "pnpm-lock.yaml",
                "yarn.lock",
                "package.json",
            ]),
        ),
        builtin(
            "biome",
            &[
                ".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs", ".mts", ".cts", ".json", ".jsonc",
                ".vue", ".astro", ".svelte", ".css", ".graphql", ".gql", ".html",
            ],
            near(&[
                "biome.json",
                "biome.jsonc",
                "package-lock.json",
                "bun.lockb",
                "bun.lock",
                "pnpm-lock.yaml",
                "yarn.lock",
            ]),
        ),
        builtin("gopls", &[".go"], RootRule::Gopls),
        builtin(
            "ruby-lsp",
            &[".rb", ".rake", ".gemspec", ".ru"],
            near(&["Gemfile"]),
        ),
        builtin(
            "ty",
            &[".py", ".pyi"],
            near(&[
                "pyproject.toml",
                "ty.toml",
                "setup.py",
                "setup.cfg",
                "requirements.txt",
                "Pipfile",
                "pyrightconfig.json",
            ]),
        ),
        builtin("pyright", &[".py", ".pyi"], near(PY)),
        builtin(
            "elixir-ls",
            &[".ex", ".exs"],
            near(&["mix.exs", "mix.lock"]),
        ),
        builtin("zls", &[".zig", ".zon"], near(&["build.zig"])),
        builtin("csharp", &[".cs", ".csx"], near(DOTNET)),
        builtin("razor", &[".razor", ".cshtml"], near(DOTNET)),
        builtin(
            "fsharp",
            &[".fs", ".fsi", ".fsx", ".fsscript"],
            near(&[".slnx", ".sln", ".fsproj", "global.json"]),
        ),
        builtin(
            "sourcekit-lsp",
            &[".swift", ".objc", "objcpp"],
            near(&["Package.swift", "*.xcodeproj", "*.xcworkspace"]),
        ),
        builtin("rust", &[".rs"], RootRule::Rust),
        builtin(
            "clangd",
            &[
                ".c", ".cpp", ".cc", ".cxx", ".c++", ".h", ".hpp", ".hh", ".hxx", ".h++",
            ],
            near(&["compile_commands.json", "compile_flags.txt", ".clangd"]),
        ),
        builtin("svelte", &[".svelte"], near(JS_LOCKS)),
        builtin("astro", &[".astro"], near(JS_LOCKS)),
        builtin("jdtls", &[".java"], RootRule::Jdtls),
        builtin("kotlin-ls", &[".kt", ".kts"], RootRule::Kotlin),
        builtin("yaml-ls", &[".yaml", ".yml"], near(JS_LOCKS)),
        builtin(
            "lua-ls",
            &[".lua"],
            near(&[
                ".luarc.json",
                ".luarc.jsonc",
                ".luacheckrc",
                ".stylua.toml",
                "stylua.toml",
                "selene.toml",
                "selene.yml",
            ]),
        ),
        builtin(
            "php intelephense",
            &[".php"],
            near(&["composer.json", "composer.lock", ".php-version"]),
        ),
        builtin(
            "prisma",
            &[".prisma"],
            RootRule::Nearest {
                include: &["schema.prisma", "prisma/schema.prisma", "prisma"],
                exclude: &["package.json"],
            },
        ),
        builtin(
            "dart",
            &[".dart"],
            near(&["pubspec.yaml", "analysis_options.yaml"]),
        ),
        builtin(
            "ocaml-lsp",
            &[".ml", ".mli"],
            near(&["dune-project", "dune-workspace", ".merlin", "opam"]),
        ),
        builtin(
            "bash",
            &[".sh", ".bash", ".zsh", ".ksh"],
            RootRule::Directory,
        ),
        builtin(
            "terraform",
            &[".tf", ".tfvars"],
            near(&[".terraform.lock.hcl", "terraform.tfstate", "*.tf"]),
        ),
        builtin(
            "texlab",
            &[".tex", ".bib"],
            near(&[".latexmkrc", "latexmkrc", ".texlabroot", "texlabroot"]),
        ),
        builtin(
            "dockerfile",
            &[".dockerfile", "Dockerfile"],
            RootRule::Directory,
        ),
        builtin("gleam", &[".gleam"], near(&["gleam.toml"])),
        builtin(
            "clojure-lsp",
            &[".clj", ".cljs", ".cljc", ".edn"],
            near(&[
                "deps.edn",
                "project.clj",
                "shadow-cljs.edn",
                "bb.edn",
                "build.boot",
            ]),
        ),
        builtin("nixd", &[".nix"], RootRule::Nixd),
        builtin("tinymist", &[".typ", ".typc"], near(&["typst.toml"])),
        builtin(
            "haskell-language-server",
            &[".hs", ".lhs"],
            near(&["stack.yaml", "cabal.project", "hie.yaml", "*.cabal"]),
        ),
        builtin(
            "julials",
            &[".jl"],
            near(&["Project.toml", "Manifest.toml", "*.jl"]),
        ),
    ]
}

/// lsp.ts:154-197: the enabled server list for a Location.
pub fn configured(config: Option<&LspConfig>, experimental_ty: bool) -> Vec<ServerDef> {
    let servers_cfg = match config {
        None | Some(LspConfig::Enabled(false)) => return Vec::new(),
        Some(LspConfig::Enabled(true)) => None,
        Some(LspConfig::Servers(map)) => Some(map),
    };
    let drop_id = if experimental_ty { "pyright" } else { "ty" };
    let mut servers: Vec<ServerDef> = builtins().into_iter().filter(|s| s.id != drop_id).collect();
    for (name, item) in servers_cfg.into_iter().flatten() {
        apply_entry(&mut servers, name, item);
    }
    servers
}

fn apply_entry(servers: &mut Vec<ServerDef>, name: &str, item: &LspServerEntry) {
    let position = servers.iter().position(|s| s.id == name);
    if item.disabled == Some(true) {
        if let Some(i) = position {
            servers.remove(i);
        }
        return;
    }
    let existing = position.map(|i| servers[i].clone());
    let def = ServerDef {
        id: name.to_string(),
        root: existing
            .as_ref()
            .map(|e| e.root.clone())
            .unwrap_or(RootRule::Directory),
        extensions: item
            .extensions
            .clone()
            .or_else(|| existing.as_ref().map(|e| e.extensions.clone()))
            .unwrap_or_default(),
        launch: Launch::Custom {
            command: item.command.clone(),
            env: item.env.clone().unwrap_or_default(),
            initialization: item.initialization.clone(),
        },
    };
    match position {
        Some(i) => servers[i] = def,
        None => servers.push(def),
    }
}

impl ServerDef {
    /// lsp.ts:243 `server.extensions.length && !server.extensions.includes(extension)`.
    pub fn matches(&self, extension: &str) -> bool {
        self.extensions.is_empty() || self.extensions.iter().any(|e| e == extension)
    }
}
