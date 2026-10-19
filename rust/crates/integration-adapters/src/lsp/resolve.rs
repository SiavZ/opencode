//! Purpose: per-server command resolution for all 38 builtins plus config-defined servers:
//! PATH lookup, local `node_modules` bins, venv detection, initializationOptions, and the
//! auto-install branches (go/gem/dotnet installs into `LaunchEnv::bin_dir`, honoring
//! `disable_download`). Ports the `spawn` bodies of packages/opencode/src/lsp/server.ts.
//! Scope: decides what to run. Spawning lives in launch.rs.
//! Uses:
//! rust/crates/
//! |-- integration-adapters/src/lsp/inventory.rs
//! |-- integration-adapters/src/lsp/launch.rs
//! |-- integration-adapters/src/lsp/resolve_extra.rs
//! |-- integration-adapters/src/lsp/roots.rs
//! `-- integration-api/src/lib.rs
//! External:
//! |-- extra::clangd
//! |-- extra::fsharp
//! |-- extra::gopls
//! |-- extra::jdtls
//! |-- extra::oxlint
//! |-- extra::python_init
//! |-- extra::razor
//! |-- extra::roslyn
//! |-- extra::rubocop
//! |-- extra::sourcekit
//! |-- extra::ty
//! `-- serde_json::json
//! Used by:
//! rust/crates/
//! |-- integration-adapters/src/lsp/mod.rs
//! |-- integration-adapters/src/lsp/resolve_extra.rs
//! |-- integration-adapters/src/lsp/service.rs
//! `-- integration-adapters/tests/lsp_inventory.rs
//! Invariants: `Ok(None)` is the baseline `undefined` (server unavailable, goes to the broken
//! set), HTTP archive downloads are `NotYetPorted` and only reached when downloads are enabled
//! and nothing is installed, every install writes only below `bin_dir`.
//! Contract tests: integration-adapters/tests/lsp_inventory.rs

use std::path::Path;

use opencode_integration_api::IntegrationError;
use serde_json::json;

use super::inventory::{Launch, ServerDef};
use super::launch::{LaunchEnv, Spec, module_resolve};
use super::resolve_extra as extra;
use super::roots::RootContext;

pub type Resolved = Result<Option<Spec>, IntegrationError>;

const fn download(what: &'static str) -> IntegrationError {
    IntegrationError::NotYetPorted(what)
}

/// `which(bin)`, else (unless downloads are disabled) the `Npm.which(pkg, hint)` cache.
fn which_or_npm(
    env: &LaunchEnv,
    bin: &str,
    pkg: &str,
    hint: Option<&str>,
) -> Result<Option<std::path::PathBuf>, IntegrationError> {
    if let Some(found) = env.which(bin) {
        return Ok(Some(found));
    }
    if env.disable_download {
        return Ok(None);
    }
    env.npm_which(pkg, hint).map(Some)
}

fn simple(env: &LaunchEnv, bin: &str, args: &[&str]) -> Resolved {
    Ok(env.which(bin).map(|b| Spec::new(b, args)))
}

pub async fn resolve(def: &ServerDef, root: &Path, ctx: &RootContext, env: &LaunchEnv) -> Resolved {
    if let Launch::Custom {
        command,
        env: vars,
        initialization,
    } = &def.launch
    {
        let Some((program, args)) = command.split_first() else {
            return Err(IntegrationError::Validation(format!(
                "lsp server {} has an empty command",
                def.id
            )));
        };
        let mut spec = Spec::new(program, &[]);
        spec.args = args.to_vec();
        spec.env = vars.clone();
        spec.initialization = initialization.clone();
        return Ok(Some(spec));
    }
    let stdio = |b: Option<std::path::PathBuf>| b.map(|b| Spec::new(b, &["--stdio"]));
    match def.id.as_str() {
        "deno" => simple(env, "deno", &["lsp"]),
        "typescript" => {
            let Some(tsserver) = module_resolve("typescript/lib/tsserver.js", &ctx.directory) else { return Ok(None) };
            let bin = env.npm_which("typescript-language-server", None)?;
            Ok(Some(Spec::new(bin, &["--stdio"]).init(json!({"tsserver": {"path": tsserver}}))))
        }
        "vue" => Ok(stdio(which_or_npm(env, "vue-language-server", "@vue/language-server", None)?).map(|s| s.init(json!({})))),
        "eslint" => {
            if module_resolve("eslint", &ctx.directory).is_none() {
                return Ok(None);
            }
            let server = env.bin_dir.join("vscode-eslint/server/out/eslintServer.js");
            if !server.exists() {
                if env.disable_download {
                    return Ok(None);
                }
                return Err(download("lsp: eslint vscode-eslint zip download + npm compile"));
            }
            Ok(Some(Spec::new("node", &[&server.to_string_lossy(), "--stdio"])))
        }
        "oxlint" => extra::oxlint(root, ctx, env).await,
        "biome" => {
            let local = root.join("node_modules/.bin/biome");
            let bin = if local.exists() { Some(local) } else { env.which("biome") };
            let bin = match bin {
                Some(b) => b,
                None if module_resolve("biome", root).is_none() => return Ok(None),
                None => env.npm_which("biome", None)?,
            };
            Ok(Some(Spec::new(bin, &["lsp-proxy", "--stdio"])))
        }
        "gopls" => extra::gopls(env).await,
        "ruby-lsp" => extra::rubocop(env).await,
        "ty" => Ok(extra::ty(root, env)),
        "pyright" => {
            let Some(bin) = which_or_npm(env, "pyright-langserver", "pyright", Some("pyright-langserver"))? else {
                return Ok(None);
            };
            Ok(Some(Spec::new(bin, &["--stdio"]).init(extra::python_init(root, env))))
        }
        "elixir-ls" => {
            if let Some(bin) = env.which("elixir-ls") {
                return Ok(Some(Spec::new(bin, &[])));
            }
            let script = if cfg!(windows) { "language_server.bat" } else { "language_server.sh" };
            let bin = env.bin_dir.join("elixir-ls-master/release").join(script);
            if bin.exists() {
                return Ok(Some(Spec::new(bin, &[])));
            }
            if env.which("elixir").is_none() || env.disable_download {
                return Ok(None);
            }
            Err(download("lsp: elixir-ls source download + mix release build"))
        }
        "zls" => {
            if let Some(bin) = env.which("zls") {
                return Ok(Some(Spec::new(bin, &[])));
            }
            if env.which("zig").is_none() || env.disable_download {
                return Ok(None);
            }
            Err(download("lsp: zls GitHub release download"))
        }
        "csharp" => Ok(extra::roslyn(env).await?.map(|b| Spec::new(b, &["--stdio", "--autoLoadProjects"]))),
        "razor" => extra::razor(env).await,
        "fsharp" => extra::fsharp(env).await,
        "sourcekit-lsp" => extra::sourcekit(env).await,
        "rust" => simple(env, "rust-analyzer", &[]),
        "clangd" => extra::clangd(env),
        "svelte" => Ok(stdio(which_or_npm(env, "svelteserver", "svelte-language-server", None)?).map(|s| s.init(json!({})))),
        "astro" => {
            let Some(tsserver) = module_resolve("typescript/lib/tsserver.js", &ctx.directory) else { return Ok(None) };
            let tsdk = tsserver.parent().map(Path::to_path_buf).unwrap_or_default();
            let bin = which_or_npm(env, "astro-ls", "@astrojs/language-server", None)?;
            Ok(stdio(bin).map(|s| s.init(json!({"typescript": {"tsdk": tsdk}}))))
        }
        "jdtls" => extra::jdtls(env).await,
        "kotlin-ls" => {
            let script = if cfg!(windows) { "kotlin-lsp.cmd" } else { "kotlin-lsp.sh" };
            let launcher = env.bin_dir.join("kotlin-ls").join(script);
            if launcher.exists() {
                return Ok(Some(Spec::new(launcher, &["--stdio"])));
            }
            if env.disable_download {
                return Ok(None);
            }
            Err(download("lsp: kotlin-lsp JetBrains CDN download"))
        }
        "yaml-ls" => Ok(stdio(which_or_npm(env, "yaml-language-server", "yaml-language-server", None)?)),
        "lua-ls" => downloadable(env, "lua-language-server", &[], "lsp: lua-language-server GitHub release download"),
        "php intelephense" => {
            let bin = which_or_npm(env, "intelephense", "intelephense", None)?;
            Ok(stdio(bin).map(|s| s.init(json!({"telemetry": {"enabled": false}}))))
        }
        "prisma" => simple(env, "prisma", &["language-server"]),
        "dart" => simple(env, "dart", &["language-server", "--lsp"]),
        "ocaml-lsp" => simple(env, "ocamllsp", &[]),
        "bash" => Ok(which_or_npm(env, "bash-language-server", "bash-language-server", None)?.map(|b| Spec::new(b, &["start"]))),
        "terraform" => Ok(downloadable(env, "terraform-ls", &["serve"], "lsp: terraform-ls HashiCorp release download")?
            .map(|s| s.init(json!({"experimentalFeatures": {"prefillRequiredFields": true, "validateOnSave": true}})))),
        "texlab" => downloadable(env, "texlab", &[], "lsp: texlab GitHub release download"),
        "dockerfile" => Ok(stdio(which_or_npm(env, "docker-langserver", "dockerfile-language-server-nodejs", None)?)),
        "gleam" => simple(env, "gleam", &["lsp"]),
        "clojure-lsp" => {
            let bin = env.which("clojure-lsp").or_else(|| if cfg!(windows) { env.which("clojure-lsp.exe") } else { None });
            Ok(bin.map(|b| Spec::new(b, &["listen"])))
        }
        "nixd" => simple(env, "nixd", &[]),
        "tinymist" => downloadable(env, "tinymist", &[], "lsp: tinymist GitHub release download"),
        "haskell-language-server" => simple(env, "haskell-language-server-wrapper", &["--lsp"]),
        "julials" => simple(env, "julia", &["--startup-file=no", "--history-file=no", "-e", "using LanguageServer; runserver()"]),
        other => Err(IntegrationError::NotFound { kind: "lsp server", name: other.to_string() }),
    }
}

/// PATH binary, else (downloads enabled) a typed gap for the archive download.
fn downloadable(env: &LaunchEnv, bin: &str, args: &[&str], gap: &'static str) -> Resolved {
    if let Some(found) = env.which(bin) {
        return Ok(Some(Spec::new(found, args)));
    }
    if env.disable_download {
        return Ok(None);
    }
    Err(download(gap))
}
