//! Purpose: fixture tests for the builtin server inventory, config merge semantics, root
//! detection rules and command resolution (PATH lookup, download-disable, typed NotYetPorted for
//! archive downloads, install into the injectable bin dir via a local fake `go`).
//! Scope: temp dirs and fake executables only, never a real installer or network.
//! Uses:
//! rust/crates/
//! |-- integration-adapters/src/lsp/inventory.rs
//! |-- integration-adapters/src/lsp/launch.rs
//! |-- integration-adapters/src/lsp/resolve.rs
//! |-- integration-adapters/src/lsp/roots.rs
//! `-- integration-adapters/tests/fixtures/lsp/support.rs
//! External:
//! `-- serde_json::json
//! Used by:
//! `-- none, entrypoint
//! Invariants: fixture-only, PATH is always a temp dir.
//! Contract tests: integration-adapters/tests/lsp_inventory.rs

#[path = "fixtures/lsp/support.rs"]
mod support;

use std::path::Path;

use opencode_integration_adapters::lsp::resolve::resolve;
use opencode_integration_adapters::lsp::roots::is_module_of;
use opencode_integration_adapters::lsp::{LaunchEnv, RootContext, builtins, configured};
use opencode_integration_api::IntegrationError;
use opencode_integration_api::language::{BUILTIN_SERVER_IDS, LspConfig};
use serde_json::json;
use support::Workspace;

fn exe(path: &Path, body: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

fn env_with_path(ws: &Workspace) -> (LaunchEnv, std::path::PathBuf) {
    let path = ws.tmp.path().canonicalize().unwrap().join("path");
    std::fs::create_dir_all(&path).unwrap();
    let mut env = ws.launch();
    env.path = Some(path.clone().into_os_string());
    (env, path)
}

#[test]
fn builtins_match_contract_ids_and_order() {
    let ids: Vec<String> = builtins().into_iter().map(|s| s.id).collect();
    assert_eq!(ids, BUILTIN_SERVER_IDS.map(String::from).to_vec());
    let dockerfile = builtins()
        .into_iter()
        .find(|s| s.id == "dockerfile")
        .unwrap();
    assert!(dockerfile.matches("Dockerfile") && dockerfile.matches(".dockerfile"));
}

#[test]
fn config_enable_override_disable_custom() {
    assert!(configured(None, false).is_empty());
    assert!(configured(Some(&LspConfig::Enabled(false)), false).is_empty());
    let all = configured(Some(&LspConfig::Enabled(true)), false);
    assert_eq!(all.len(), 37);
    assert!(all.iter().any(|s| s.id == "pyright") && !all.iter().any(|s| s.id == "ty"));
    let ty = configured(Some(&LspConfig::Enabled(true)), true);
    assert!(ty.iter().any(|s| s.id == "ty") && !ty.iter().any(|s| s.id == "pyright"));

    let cfg: LspConfig = serde_json::from_value(json!({
        "rust": {"command": ["my-ra"]},
        "gopls": {"disabled": true},
        "mine": {"command": ["x", "--y"], "extensions": [".foo"]}
    }))
    .unwrap();
    let servers = configured(Some(&cfg), false);
    let pos = |id: &str| servers.iter().position(|s| s.id == id);
    assert_eq!(pos("gopls"), None);
    let rust = &servers[pos("rust").unwrap()];
    assert_eq!(
        rust.extensions,
        [".rs"],
        "override keeps builtin extensions"
    );
    assert_eq!(
        rust.root,
        builtins()
            .into_iter()
            .find(|s| s.id == "rust")
            .unwrap()
            .root,
        "and root"
    );
    assert_eq!(pos("mine"), Some(servers.len() - 1), "custom appended");
}

#[test]
fn roots_nearest_strict_exclude_and_special_rules() {
    let ws = Workspace::new();
    let ctx = RootContext {
        directory: ws.dir.clone(),
        worktree: ws.dir.clone(),
    };
    let get = |id: &str| builtins().into_iter().find(|s| s.id == id).unwrap().root;
    ws.write("web/package-lock.json", "");
    let f = ws.write("web/src/a.ts", "");
    assert_eq!(
        get("typescript").resolve(&f, &ctx),
        Some(ws.dir.join("web"))
    );
    assert_eq!(get("deno").resolve(&f, &ctx), None, "strict without marker");
    assert_eq!(
        get("typescript").resolve(&ws.write("loose/b.ts", ""), &ctx),
        Some(ws.dir.clone()),
        "fallback"
    );
    ws.write("d/deno.json", "");
    let d = ws.write("d/x.ts", "");
    assert_eq!(
        get("typescript").resolve(&d, &ctx),
        None,
        "excluded by deno.json"
    );
    assert_eq!(get("deno").resolve(&d, &ctx), Some(ws.dir.join("d")));

    ws.write("Cargo.toml", "[workspace]\nmembers=[\"crates/*\"]\n");
    ws.write("crates/x/Cargo.toml", "[package]\n");
    let rs = ws.write("crates/x/src/lib.rs", "");
    assert_eq!(
        get("rust").resolve(&rs, &ctx),
        Some(ws.dir.clone()),
        "workspace root wins"
    );

    ws.write("g/go.work", "");
    ws.write("g/m/go.mod", "");
    assert_eq!(
        get("gopls").resolve(&ws.write("g/m/x.go", ""), &ctx),
        Some(ws.dir.join("g"))
    );

    ws.write(
        "j/pom.xml",
        "<modules><!-- <module>no</module> --><module>./core/</module></modules>",
    );
    ws.write("j/core/pom.xml", "");
    let java = ws.write("j/core/src/A.java", "");
    assert_eq!(get("jdtls").resolve(&java, &ctx), Some(ws.dir.join("j")));
    assert!(is_module_of(
        "<modules><module>core</module></modules>",
        "core/"
    ));
    assert!(!is_module_of(
        "<modules><!--<module>core</module>--></modules>",
        "core"
    ));

    assert_eq!(
        get("bash").resolve(&ws.write("deep/x.sh", ""), &ctx),
        Some(ws.dir.clone())
    );
    let mut nix_ctx = ctx.clone();
    nix_ctx.directory = ws.dir.join("deep");
    assert_eq!(
        get("nixd").resolve(&ws.write("deep/a.nix", ""), &nix_ctx),
        Some(ws.dir.clone()),
        "worktree"
    );
}

#[tokio::test]
async fn resolution_path_disable_and_typed_download_gaps() {
    let ws = Workspace::new();
    let ctx = ws.ctx();
    let (mut env, path) = env_with_path(&ws);
    let def = |id: &str| builtins().into_iter().find(|s| s.id == id).unwrap();

    // Nothing installed, downloads disabled: every builtin resolves to None, no error, no spawn.
    for server in builtins() {
        let got = resolve(&server, &ws.dir, &ctx, &env).await;
        assert!(matches!(got, Ok(None)), "{}: {got:?}", server.id);
    }

    // PATH binaries win, with baseline args and initialization.
    exe(&path.join("rust-analyzer"), "exit 0");
    exe(&path.join("clangd"), "exit 0");
    exe(&path.join("terraform-ls"), "exit 0");
    let ra = resolve(&def("rust"), &ws.dir, &ctx, &env)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ra.program, path.join("rust-analyzer"));
    let clangd = resolve(&def("clangd"), &ws.dir, &ctx, &env)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(clangd.args, ["--background-index", "--clang-tidy"]);
    let tf = resolve(&def("terraform"), &ws.dir, &ctx, &env)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(tf.args, ["serve"]);
    assert_eq!(
        json!(tf.initialization),
        json!({"experimentalFeatures": {"prefillRequiredFields": true, "validateOnSave": true}})
    );

    // Pyright venv detection, npm cache lookup.
    exe(&ws.dir.join(".venv/bin/python"), "exit 0");
    let npm_bin = env
        .npm_cache
        .join("pyright/node_modules/.bin/pyright-langserver");
    exe(&npm_bin, "exit 0");
    env.disable_download = false;
    let py = resolve(&def("pyright"), &ws.dir, &ctx, &env)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(py.program, npm_bin);
    assert_eq!(
        json!(py.initialization),
        json!({"pythonPath": ws.dir.join(".venv/bin/python")})
    );

    // Downloads enabled and nothing installed: typed gaps, never silent success.
    for id in ["lua-ls", "texlab", "tinymist", "kotlin-ls", "yaml-ls"] {
        let got = resolve(&def(id), &ws.dir, &ctx, &env).await;
        assert!(
            matches!(got, Err(IntegrationError::NotYetPorted(_))),
            "{id}: {got:?}"
        );
    }
}

#[tokio::test]
async fn gopls_install_targets_injected_bin_dir_via_local_fake_go() {
    let ws = Workspace::new();
    let ctx = ws.ctx();
    let (mut env, path) = env_with_path(&ws);
    let gopls = builtins().into_iter().find(|s| s.id == "gopls").unwrap();
    // Fake `go install` writes gopls into $GOBIN, proving the install target is env.bin_dir.
    exe(
        &path.join("go"),
        "/bin/mkdir -p \"$GOBIN\" && printf '#!/bin/sh\\n' > \"$GOBIN/gopls\" && echo \"$@\" > \"$GOBIN/args\"",
    );
    assert!(
        matches!(resolve(&gopls, &ws.dir, &ctx, &env).await, Ok(None)),
        "disabled: no install"
    );
    assert!(!env.bin_dir.join("args").exists());
    env.disable_download = false;
    let spec = resolve(&gopls, &ws.dir, &ctx, &env).await.unwrap().unwrap();
    assert_eq!(spec.program, env.bin_dir.join("gopls"));
    let args = std::fs::read_to_string(env.bin_dir.join("args")).unwrap();
    assert_eq!(args.trim(), "install golang.org/x/tools/gopls@latest");
}
