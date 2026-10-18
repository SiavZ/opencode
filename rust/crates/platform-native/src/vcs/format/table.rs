//! Purpose: built-in formatter table of packages/opencode/src/format/formatter.ts: names,
//! extensions, environment and the enablement probe of each formatter.
//! Scope: pure table plus async detection against the filesystem, PATH and npm package bins.
//! Uses:
//! rust/crates/
//! `-- platform-native/src/vcs/format/mod.rs
//! External:
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! `-- platform-native/src/vcs/format/mod.rs
//! Invariants: table order matches the baseline module export order (status listing order);
//! a probe returns the command template containing `$FILE`, or None when disabled.
//! Contract tests: platform-native/tests/format.rs

use super::FormatContext;
use std::path::{Path, PathBuf};

/// How a built-in decides whether it is enabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Probe {
    /// `which(bin)` then `[match, ...args]`.
    Which(&'static str, &'static [&'static str]),
    /// package.json dependency `pkg` then npm bin `[bin, ...args]`; oxfmt also needs the flag.
    PackageJson(&'static str, &'static [&'static str]),
    Oxfmt,
    Biome,
    Clang,
    Ruff,
    Air,
    Uv,
    Ocamlformat,
    Pint,
}

#[derive(Debug, Clone, Copy)]
pub struct BuiltIn {
    pub name: &'static str,
    pub extensions: &'static [&'static str],
    pub environment: &'static [(&'static str, &'static str)],
    pub probe: Probe,
}

const BUN: &[(&str, &str)] = &[("BUN_BE_BUN", "1")];
const WEB: &[&str] = &[
    ".js", ".jsx", ".mjs", ".cjs", ".ts", ".tsx", ".mts", ".cts", ".html", ".htm", ".css", ".scss",
    ".sass", ".less", ".vue", ".svelte", ".json", ".jsonc", ".yaml", ".yml", ".toml", ".xml",
    ".md", ".mdx", ".graphql", ".gql",
];
const RUBY: &[&str] = &[".rb", ".rake", ".gemspec", ".ru"];

const fn which(
    name: &'static str,
    extensions: &'static [&'static str],
    bin: &'static str,
    args: &'static [&'static str],
) -> BuiltIn {
    BuiltIn {
        name,
        extensions,
        environment: &[],
        probe: Probe::Which(bin, args),
    }
}

/// Built-ins in `Object.values(Formatter)` order.
pub const BUILT_INS: &[BuiltIn] = &[
    which("gofmt", &[".go"], "gofmt", &["-w", "$FILE"]),
    which(
        "mix",
        &[".ex", ".exs", ".eex", ".heex", ".leex", ".neex", ".sface"],
        "mix",
        &["format", "$FILE"],
    ),
    BuiltIn {
        name: "prettier",
        extensions: WEB,
        environment: BUN,
        probe: Probe::PackageJson("prettier", &["--write", "$FILE"]),
    },
    BuiltIn {
        name: "oxfmt",
        extensions: &[".js", ".jsx", ".mjs", ".cjs", ".ts", ".tsx", ".mts", ".cts"],
        environment: BUN,
        probe: Probe::Oxfmt,
    },
    BuiltIn {
        name: "biome",
        extensions: WEB,
        environment: BUN,
        probe: Probe::Biome,
    },
    which("zig", &[".zig", ".zon"], "zig", &["fmt", "$FILE"]),
    BuiltIn {
        name: "clang-format",
        extensions: &[
            ".c", ".cc", ".cpp", ".cxx", ".c++", ".h", ".hh", ".hpp", ".hxx", ".h++", ".ino", ".C",
            ".H",
        ],
        environment: &[],
        probe: Probe::Clang,
    },
    which("ktlint", &[".kt", ".kts"], "ktlint", &["-F", "$FILE"]),
    BuiltIn {
        name: "ruff",
        extensions: &[".py", ".pyi"],
        environment: &[],
        probe: Probe::Ruff,
    },
    BuiltIn {
        name: "air",
        extensions: &[".R"],
        environment: &[],
        probe: Probe::Air,
    },
    BuiltIn {
        name: "uv",
        extensions: &[".py", ".pyi"],
        environment: &[],
        probe: Probe::Uv,
    },
    which("rubocop", RUBY, "rubocop", &["--autocorrect", "$FILE"]),
    which("standardrb", RUBY, "standardrb", &["--fix", "$FILE"]),
    which(
        "htmlbeautifier",
        &[".erb", ".html.erb"],
        "htmlbeautifier",
        &["$FILE"],
    ),
    which("dart", &[".dart"], "dart", &["format", "$FILE"]),
    BuiltIn {
        name: "ocamlformat",
        extensions: &[".ml", ".mli"],
        environment: &[],
        probe: Probe::Ocamlformat,
    },
    which(
        "terraform",
        &[".tf", ".tfvars"],
        "terraform",
        &["fmt", "$FILE"],
    ),
    which(
        "latexindent",
        &[".tex"],
        "latexindent",
        &["-w", "-s", "$FILE"],
    ),
    which("gleam", &[".gleam"], "gleam", &["format", "$FILE"]),
    which("shfmt", &[".sh", ".bash"], "shfmt", &["-w", "$FILE"]),
    which("nixfmt", &[".nix"], "nixfmt", &["$FILE"]),
    which("rustfmt", &[".rs"], "rustfmt", &["$FILE"]),
    BuiltIn {
        name: "pint",
        extensions: &[".php"],
        environment: &[],
        probe: Probe::Pint,
    },
    which("ormolu", &[".hs"], "ormolu", &["-i", "$FILE"]),
    which(
        "cljfmt",
        &[".clj", ".cljs", ".cljc", ".edn"],
        "cljfmt",
        &["fix", "--quiet", "$FILE"],
    ),
    which("dfmt", &[".d"], "dfmt", &["-i", "$FILE"]),
];

pub fn built_in(name: &str) -> Option<&'static BuiltIn> {
    BUILT_INS.iter().find(|item| item.name == name)
}

/// `Filesystem.findUp(target, start, stop)`: matches from `start` upward, stopping at `stop`.
pub fn find_up(target: &str, start: &Path, stop: Option<&Path>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for dir in start.ancestors() {
        let candidate = dir.join(target);
        if candidate.exists() {
            out.push(candidate);
        }
        if Some(dir) == stop {
            break;
        }
    }
    out
}

fn command(bin: impl Into<String>, args: &[&str]) -> Vec<String> {
    std::iter::once(bin.into())
        .chain(args.iter().map(|arg| arg.to_string()))
        .collect()
}

fn has_dependency(file: &Path, keys: [&str; 2], package: &str) -> bool {
    let Ok(text) = std::fs::read_to_string(file) else {
        return false;
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
        return false;
    };
    keys.iter().any(|key| {
        json.get(*key)
            .and_then(|deps| deps.get(package))
            .is_some_and(|value| !value.is_null() && value != "" && value != false)
    })
}

impl Probe {
    /// `Info.enabled(context)`.
    pub async fn enabled(self, ctx: &FormatContext) -> Option<Vec<String>> {
        let up = |target: &str| find_up(target, &ctx.directory, Some(&ctx.worktree));
        let read = |file: &PathBuf| std::fs::read_to_string(file).unwrap_or_default();
        let ruff = || command("ruff", &["format", "$FILE"]);
        match self {
            Probe::Which(bin, args) => ctx.which(bin).map(|path| command(path, args)),
            Probe::PackageJson(package, args) => {
                for item in up("package.json") {
                    if has_dependency(&item, ["dependencies", "devDependencies"], package)
                        && let Some(bin) = ctx.npm_which(package).await
                    {
                        return Some(command(bin, args));
                    }
                }
                None
            }
            Probe::Oxfmt => {
                if !ctx.experimental_oxfmt {
                    return None;
                }
                Box::pin(Probe::PackageJson("oxfmt", &["$FILE"]).enabled(ctx)).await
            }
            Probe::Biome => {
                for config in ["biome.json", "biome.jsonc"] {
                    if !up(config).is_empty()
                        && let Some(bin) = ctx.npm_which("@biomejs/biome").await
                    {
                        return Some(command(bin, &["format", "--write", "$FILE"]));
                    }
                }
                None
            }
            Probe::Clang => {
                if up(".clang-format").is_empty() {
                    return None;
                }
                ctx.which("clang-format")
                    .map(|path| command(path, &["-i", "$FILE"]))
            }
            Probe::Ruff => {
                ctx.which("ruff")?;
                for config in ["pyproject.toml", "ruff.toml", ".ruff.toml"] {
                    let Some(found) = up(config).into_iter().next() else {
                        continue;
                    };
                    if config != "pyproject.toml" || read(&found).contains("[tool.ruff]") {
                        return Some(ruff());
                    }
                }
                for dep in ["requirements.txt", "pyproject.toml", "Pipfile"] {
                    if let Some(found) = up(dep).into_iter().next()
                        && read(&found).contains("ruff")
                    {
                        return Some(ruff());
                    }
                }
                None
            }
            Probe::Air => {
                let air = ctx.which("air")?;
                let output = ctx.probe(&air, &["--help"]).await?;
                let first = output.1.lines().next().unwrap_or_default();
                (output.0 == 0 && first.contains("R language") && first.contains("formatter"))
                    .then(|| command(air, &["format", "$FILE"]))
            }
            Probe::Uv => {
                if Box::pin(Probe::Ruff.enabled(ctx)).await.is_some() {
                    return None;
                }
                let uv = ctx.which("uv")?;
                let output = ctx.probe(&uv, &["format", "--help"]).await?;
                (output.0 == 0).then(|| command(uv, &["format", "--", "$FILE"]))
            }
            Probe::Ocamlformat => {
                ctx.which("ocamlformat")?;
                (!up(".ocamlformat").is_empty()).then(|| command("ocamlformat", &["-i", "$FILE"]))
            }
            Probe::Pint => up("composer.json")
                .iter()
                .any(|item| has_dependency(item, ["require", "require-dev"], "laravel/pint"))
                .then(|| command("./vendor/bin/pint", &["$FILE"])),
        }
    }
}
