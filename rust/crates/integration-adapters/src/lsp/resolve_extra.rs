//! Purpose: the longer builtin resolution bodies: oxlint (`--lsp` probe), gopls/rubocop/fsharp
//! installs into the injectable bin dir, roslyn (csharp/razor) with a serialized dotnet tool
//! install, vscode razor extension discovery, sourcekit via xcrun, clangd bin-dir lookup, jdtls
//! (java >= 21 + installed distribution), ty and python venv detection.
//! Ports packages/opencode/src/lsp/server.ts:224-294,358-583,687-888,935-1067,1147-1271.
//! Scope: called only from resolve.rs.
//! Uses:
//! rust/crates/
//! |-- integration-adapters/src/lsp/launch.rs
//! |-- integration-adapters/src/lsp/resolve.rs
//! |-- integration-adapters/src/lsp/roots.rs
//! `-- integration-api/src/error.rs
//! External:
//! |-- regex::Regex
//! `-- serde_json::{Value, json}
//! Used by:
//! rust/crates/
//! `-- integration-adapters/src/lsp/resolve.rs
//! Invariants: installers run only when the prerequisite tool exists and downloads are enabled,
//! write only into `LaunchEnv::bin_dir` (or dotnet's own global tool dir for roslyn, as baseline),
//! and a failed install returns `Ok(None)`.
//! Contract tests: integration-adapters/tests/lsp_inventory.rs

use std::path::{Path, PathBuf};

use opencode_integration_api::IntegrationError;
use regex::Regex;
use serde_json::{Value, json};

use super::launch::{LaunchEnv, Spec};
use super::resolve::Resolved;
use super::roots::{RootContext, up_first};

const EXE: &str = if cfg!(windows) { ".exe" } else { "" };

pub async fn oxlint(root: &Path, ctx: &RootContext, env: &LaunchEnv) -> Resolved {
    let ext = if cfg!(windows) { ".cmd" } else { "" };
    let resolve_bin = |name: &str| {
        let target = format!("node_modules/.bin/{name}{ext}");
        let local = root.join(&target);
        if local.exists() {
            return Some(local);
        }
        up_first(&[target.as_str()], root, &ctx.worktree)
    };
    if let Some(lint) = resolve_bin("oxlint").or_else(|| env.which("oxlint")) {
        let help = env.run(&lint, &["--help"], None, &[]).await;
        if help.is_some_and(|(_, out, _)| out.contains("--lsp")) {
            return Ok(Some(Spec::new(lint, &["--lsp"])));
        }
    }
    Ok(resolve_bin("oxc_language_server")
        .or_else(|| env.which("oxc_language_server"))
        .map(|b| Spec::new(b, &[])))
}

pub async fn gopls(env: &LaunchEnv) -> Resolved {
    if let Some(bin) = env.which("gopls") {
        return Ok(Some(Spec::new(bin, &[])));
    }
    let Some(go) = env.which("go") else {
        return Ok(None);
    };
    if env.disable_download {
        return Ok(None);
    }
    let bin_dir = env.bin_dir.to_string_lossy().into_owned();
    let ok = env
        .run(
            &go,
            &["install", "golang.org/x/tools/gopls@latest"],
            None,
            &[("GOBIN", &bin_dir)],
        )
        .await;
    if !ok.is_some_and(|(code, _, _)| code == 0) {
        return Ok(None);
    }
    Ok(Some(Spec::new(
        env.bin_dir.join(format!("gopls{EXE}")),
        &[],
    )))
}

pub async fn rubocop(env: &LaunchEnv) -> Resolved {
    if let Some(bin) = env.which("rubocop") {
        return Ok(Some(Spec::new(bin, &["--lsp"])));
    }
    let (Some(_), Some(gem)) = (env.which("ruby"), env.which("gem")) else {
        return Ok(None);
    };
    if env.disable_download {
        return Ok(None);
    }
    let bin_dir = env.bin_dir.to_string_lossy().into_owned();
    let ok = env
        .run(
            &gem,
            &["install", "rubocop", "--bindir", &bin_dir],
            None,
            &[],
        )
        .await;
    if !ok.is_some_and(|(code, _, _)| code == 0) {
        return Ok(None);
    }
    Ok(Some(Spec::new(
        env.bin_dir.join(format!("rubocop{EXE}")),
        &["--lsp"],
    )))
}

pub async fn fsharp(env: &LaunchEnv) -> Resolved {
    if let Some(bin) = env.which("fsautocomplete") {
        return Ok(Some(Spec::new(bin, &[])));
    }
    let Some(dotnet) = env.which("dotnet") else {
        return Ok(None);
    };
    if env.disable_download {
        return Ok(None);
    }
    let bin_dir = env.bin_dir.to_string_lossy().into_owned();
    let ok = env
        .run(
            &dotnet,
            &["tool", "install", "fsautocomplete", "--tool-path", &bin_dir],
            None,
            &[],
        )
        .await;
    if !ok.is_some_and(|(code, _, _)| code == 0) {
        return Ok(None);
    }
    Ok(Some(Spec::new(
        env.bin_dir.join(format!("fsautocomplete{EXE}")),
        &[],
    )))
}

fn venvs(root: &Path, env: &LaunchEnv) -> Vec<PathBuf> {
    env.var("VIRTUAL_ENV")
        .map(PathBuf::from)
        .into_iter()
        .chain([root.join(".venv"), root.join("venv")])
        .collect()
}

fn venv_bin(venv: &Path, name: &str) -> PathBuf {
    if cfg!(windows) {
        venv.join("Scripts").join(format!("{name}.exe"))
    } else {
        venv.join("bin").join(name)
    }
}

/// `{pythonPath}` from the first venv that has a python interpreter, else `{}`.
pub fn python_init(root: &Path, env: &LaunchEnv) -> Value {
    venvs(root, env)
        .iter()
        .map(|v| venv_bin(v, "python"))
        .find(|p| p.exists())
        .map(|p| json!({"pythonPath": p}))
        .unwrap_or_else(|| json!({}))
}

pub fn ty(root: &Path, env: &LaunchEnv) -> Option<Spec> {
    if !env.experimental_ty {
        return None;
    }
    let bin = env.which("ty").or_else(|| {
        venvs(root, env)
            .iter()
            .map(|v| venv_bin(v, "ty"))
            .find(|p| p.exists())
    })?;
    Some(Spec::new(bin, &["server"]).init(python_init(root, env)))
}

fn roslyn_global(env: &LaunchEnv) -> Option<PathBuf> {
    let base = env
        .var("DOTNET_CLI_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| env.home.clone());
    let ext = if cfg!(windows) { ".cmd" } else { "" };
    let bin = base
        .join(".dotnet/tools")
        .join(format!("roslyn-language-server{ext}"));
    bin.exists().then_some(bin)
}

/// server.ts:737-785: PATH, global tool path, then one serialized `dotnet tool install`.
pub async fn roslyn(env: &LaunchEnv) -> Result<Option<PathBuf>, IntegrationError> {
    if let Some(found) = env
        .which("roslyn-language-server")
        .or_else(|| roslyn_global(env))
    {
        return Ok(Some(found));
    }
    let _guard = env.install_lock.lock().await;
    if let Some(found) = env
        .which("roslyn-language-server")
        .or_else(|| roslyn_global(env))
    {
        return Ok(Some(found));
    }
    let Some(dotnet) = env.which("dotnet") else {
        return Ok(None);
    };
    if env.disable_download {
        return Ok(None);
    }
    let args = [
        "tool",
        "install",
        "--global",
        "roslyn-language-server",
        "--prerelease",
    ];
    if !env
        .run(&dotnet, &args, None, &[])
        .await
        .is_some_and(|(c, _, _)| c == 0)
    {
        return Ok(None);
    }
    Ok(env
        .which("roslyn-language-server")
        .or_else(|| roslyn_global(env)))
}

pub async fn razor(env: &LaunchEnv) -> Resolved {
    let Some(bin) = roslyn(env).await? else {
        return Ok(None);
    };
    let Some([compiler, targets, extension]) = razor_extension(env) else {
        return Ok(None);
    };
    let args = [
        "--stdio".to_string(),
        "--autoLoadProjects".to_string(),
        format!("--razorSourceGenerator={}", compiler.display()),
        format!("--razorDesignTimePath={}", targets.display()),
        "--extension".to_string(),
        extension.display().to_string(),
    ];
    let mut spec = Spec::new(bin, &[]);
    spec.args = args.to_vec();
    Ok(Some(spec))
}

fn razor_extension(env: &LaunchEnv) -> Option<[PathBuf; 3]> {
    let home = &env.home;
    let mut roots: Vec<PathBuf> = env
        .var("VSCODE_EXTENSIONS")
        .map(PathBuf::from)
        .into_iter()
        .collect();
    for sub in [
        ".vscode",
        ".vscode-insiders",
        ".vscode-server",
        ".vscode-server-insiders",
    ] {
        let p = home.join(sub).join("extensions");
        if !roots.contains(&p) {
            roots.push(p);
        }
    }
    for root in roots {
        let mut candidates: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(&root)
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.path().is_dir()
                    && e.file_name()
                        .to_string_lossy()
                        .starts_with("ms-dotnettools.csharp-")
            })
            .map(|e| {
                let modified = e
                    .metadata()
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::UNIX_EPOCH);
                (modified, e.path().join(".razorExtension"))
            })
            .collect();
        candidates.sort_by_key(|c| std::cmp::Reverse(c.0));
        for (_, entry) in candidates {
            let set = [
                entry.join("Microsoft.CodeAnalysis.Razor.Compiler.dll"),
                entry.join("Targets/Microsoft.NET.Sdk.Razor.DesignTime.targets"),
                entry.join("Microsoft.VisualStudioCode.RazorExtension.dll"),
            ];
            if set.iter().all(|p| p.exists()) {
                return Some(set);
            }
        }
    }
    None
}

pub async fn sourcekit(env: &LaunchEnv) -> Resolved {
    if let Some(bin) = env.which("sourcekit-lsp") {
        return Ok(Some(Spec::new(bin, &[])));
    }
    let Some(xcrun) = env.which("xcrun") else {
        return Ok(None);
    };
    match env
        .run(&xcrun, &["--find", "sourcekit-lsp"], None, &[])
        .await
    {
        Some((0, out, _)) => Ok(Some(Spec::new(out.trim(), &[]))),
        _ => Ok(None),
    }
}

pub fn clangd(env: &LaunchEnv) -> Resolved {
    let args = ["--background-index", "--clang-tidy"];
    if let Some(bin) = env.which("clangd") {
        return Ok(Some(Spec::new(bin, &args)));
    }
    let direct = env.bin_dir.join(format!("clangd{EXE}"));
    if direct.exists() {
        return Ok(Some(Spec::new(direct, &args)));
    }
    let nested = std::fs::read_dir(&env.bin_dir)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir() && e.file_name().to_string_lossy().starts_with("clangd_"))
        .map(|e| e.path().join("bin").join(format!("clangd{EXE}")))
        .find(|p| p.exists());
    if let Some(bin) = nested {
        return Ok(Some(Spec::new(bin, &args)));
    }
    if env.disable_download {
        return Ok(None);
    }
    Err(IntegrationError::NotYetPorted(
        "lsp: clangd GitHub release download",
    ))
}

pub async fn jdtls(env: &LaunchEnv) -> Resolved {
    let Some(java) = env.which("java") else {
        return Ok(None);
    };
    let version = env
        .run(&java, &["-version"], None, &[])
        .await
        .and_then(|(_, _, err)| {
            let re = Regex::new(r#""(\d+)\.\d+\.\d+""#).ok()?;
            re.captures(&err)?[1].parse::<u32>().ok()
        });
    if !version.is_some_and(|v| v >= 21) {
        return Ok(None);
    }
    let dist = env.bin_dir.join("jdtls");
    let launcher_dir = dist.join("plugins");
    if !launcher_dir.exists() {
        if env.disable_download {
            return Ok(None);
        }
        return Err(IntegrationError::NotYetPorted(
            "lsp: jdtls eclipse.org snapshot download",
        ));
    }
    let jar_re = Regex::new(r"^org\.eclipse\.equinox\.launcher_.*\.jar$")
        .map_err(|e| IntegrationError::Validation(e.to_string()))?;
    let jar = std::fs::read_dir(&launcher_dir)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .find(|n| jar_re.is_match(n));
    let Some(jar) = jar.map(|j| launcher_dir.join(j)).filter(|p| p.exists()) else {
        return Ok(None);
    };
    let config = dist.join(if cfg!(target_os = "macos") {
        "config_mac"
    } else if cfg!(windows) {
        "config_win"
    } else {
        "config_linux"
    });
    let data = tempdir_under(&std::env::temp_dir(), "opencode-jdtls-data").map_err(|e| {
        IntegrationError::Failed {
            name: "jdtls".into(),
            message: e.to_string(),
        }
    })?;
    let mut spec = Spec::new(java, &[]);
    spec.args = [
        "-jar",
        &jar.to_string_lossy(),
        "-configuration",
        &config.to_string_lossy(),
        "-data",
        &data.to_string_lossy(),
        "-Declipse.application=org.eclipse.jdt.ls.core.id1",
        "-Dosgi.bundles.defaultStartLevel=4",
        "-Declipse.product=org.eclipse.jdt.ls.core.product",
        "-Dlog.level=ALL",
        "--add-modules=ALL-SYSTEM",
        "--add-opens java.base/java.util=ALL-UNNAMED",
        "--add-opens java.base/java.lang=ALL-UNNAMED",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    Ok(Some(spec))
}

/// `fs.mkdtemp(prefix)`.
fn tempdir_under(base: &Path, prefix: &str) -> std::io::Result<PathBuf> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = base.join(format!("{prefix}{:x}{:x}", std::process::id(), nanos));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}
