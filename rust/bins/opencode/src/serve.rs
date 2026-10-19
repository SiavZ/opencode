//! Purpose: `opencode serve` (packages/opencode/src/cli/cmd/serve.ts) and the network option
//! resolution it shares with `web` (packages/opencode/src/cli/network.ts:42-82): explicit argv
//! flags win, then the global config `server` section, then the yargs defaults.
//! Scope: the serve command handler over `opencode_runtime::Process`.
//! Uses:
//! rust/bins/
//! |-- opencode/src/cli/spec/mod.rs
//! `-- opencode/src/cli/spec/parse.rs
//! rust/crates/
//! |-- config/src/env.rs
//! |-- config/src/legacy/mod.rs
//! |-- config/src/legacy/parse.rs
//! `-- runtime/src/lib.rs
//! External:
//! |-- serde_json::Value
//! |-- serde_json::json
//! |-- str::to_owned
//! |-- tokio::runtime::Builder::new_multi_thread
//! |-- tokio::select
//! |-- tokio::signal::ctrl_c
//! |-- tokio::signal::unix::SignalKind::terminate
//! `-- tokio::signal::unix::signal
//! Used by:
//! rust/bins/
//! `-- opencode/src/main.rs
//! Invariants: stdout carries exactly the baseline lines (unsecured warning, readiness line).
//! The server runs until SIGINT/SIGTERM, then stops the listener and shuts the process down in
//! order. mDNS publication is not ported and is reported on stderr, never silently claimed.
//! Contract tests: opencode/tests/serve.rs

use std::process::ExitCode;

use opencode_config::env::Environment;
use opencode_config::legacy::Diagnostics;
use opencode_config::legacy::parse::load_global;
use opencode_runtime::{ListenOptions, Process, ProcessOptions};
use serde_json::Value;

use crate::cli::spec::Parsed;

/// Resolved `NetworkOptions`.
#[derive(Debug, Clone, PartialEq)]
pub struct Network {
    pub hostname: String,
    pub port: u16,
    pub mdns: bool,
    pub mdns_domain: String,
    pub cors: Vec<String>,
}

pub fn run(parsed: &Parsed, version: &str) -> ExitCode {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("Error: failed to start runtime: {error}");
            return ExitCode::from(1);
        }
    };
    runtime.block_on(serve(parsed, version))
}

async fn serve(parsed: &Parsed, version: &str) -> ExitCode {
    let mut env = Environment::from_process();
    for (key, value) in parsed.globals.middleware_env(std::process::id()) {
        env.set_var(key, value);
    }
    let config = match load_global(&env, None, &mut Diagnostics::default()) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("Error: {error}");
            return ExitCode::from(1);
        }
    };
    let network = match resolve(parsed, config.get("server")) {
        Ok(network) => network,
        Err(error) => {
            eprintln!("Error: {error}");
            return ExitCode::from(1);
        }
    };
    if std::env::var("OPENCODE_SERVER_PASSWORD").map_or(true, |p| p.is_empty()) {
        println!("Warning: OPENCODE_SERVER_PASSWORD is not set; server is unsecured.");
    }
    let channel = option_env!("OPENCODE_CHANNEL").unwrap_or("local");
    let mut options = ProcessOptions::from_process(version, channel);
    options.environment = env;
    let process = match Process::start(options).await {
        Ok(process) => process,
        Err(error) => {
            eprintln!("Error: {error}");
            return ExitCode::from(1);
        }
    };
    let listener = match process
        .serve(
            ListenOptions {
                hostname: network.hostname.clone(),
                port: network.port,
            },
            network.cors.clone(),
        )
        .await
    {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("Error: {error}");
            process.shutdown().await;
            return ExitCode::from(1);
        }
    };
    if network.mdns {
        eprintln!(
            "opencode (rust port): mDNS publication ({}) is not yet ported",
            network.mdns_domain
        );
    }
    println!("{}", listener.readiness_line());
    wait_for_shutdown().await;
    listener.stop(true).await;
    process.shutdown().await;
    ExitCode::SUCCESS
}

#[cfg(unix)]
async fn wait_for_shutdown() {
    let mut terminate =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok();
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = async {
            match terminate.as_mut() {
                Some(signal) => { signal.recv().await; }
                None => std::future::pending::<()>().await,
            }
        } => {}
    }
}

#[cfg(not(unix))]
async fn wait_for_shutdown() {
    let _ = tokio::signal::ctrl_c().await;
}

#[derive(Debug, thiserror::Error)]
#[error("invalid port: expected a finite integer in 0..=65535")]
pub struct InvalidPort;

/// `resolveNetworkOptionsNoConfig(args, config.server)`.
pub fn resolve(parsed: &Parsed, server: Option<&Value>) -> Result<Network, InvalidPort> {
    let args = &parsed.args;
    let flags: Vec<&str> = parsed
        .raw
        .iter()
        .take_while(|arg| arg.as_str() != "--")
        .map(String::as_str)
        .collect();
    let has = |name: &str| {
        flags
            .iter()
            .any(|arg| *arg == name || arg.starts_with(&format!("{name}=")))
    };
    let has_bool = |name: &str| {
        let negated = format!("--no-{}", &name[2..]);
        flags.iter().any(|arg| {
            *arg == name
                || *arg == format!("{name}=true")
                || *arg == format!("{name}=false")
                || *arg == negated
        })
    };
    let config = |key: &str| server.and_then(|s| s.get(key)).filter(|v| !v.is_null());
    let config_str = |key: &str| config(key).and_then(Value::as_str).map(str::to_owned);
    let mdns = if has_bool("--mdns") {
        args.bool("mdns")
    } else {
        config("mdns")
            .and_then(Value::as_bool)
            .unwrap_or_else(|| args.bool("mdns"))
    };
    let arg_str = |key: &str, fallback: &str| args.str(key).unwrap_or_else(|| fallback.to_owned());
    let mdns_domain = if has("--mdns-domain") {
        arg_str("mdns-domain", "opencode.local")
    } else {
        config_str("mdnsDomain").unwrap_or_else(|| arg_str("mdns-domain", "opencode.local"))
    };
    let arg_port = if has("--port") {
        args.num("port").ok_or(InvalidPort)?
    } else {
        args.num("port").unwrap_or(0.0)
    };
    let port = if has("--port") {
        arg_port
    } else {
        config("port").and_then(Value::as_f64).unwrap_or(arg_port)
    };
    let hostname = if has("--hostname") {
        arg_str("hostname", "127.0.0.1")
    } else if mdns && config_str("hostname").is_none() {
        "0.0.0.0".to_owned()
    } else {
        config_str("hostname").unwrap_or_else(|| arg_str("hostname", "127.0.0.1"))
    };
    let mut cors: Vec<String> = config("cors")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    cors.extend(args.strings("cors"));
    if !port.is_finite() || port.fract() != 0.0 || !(0.0..=65535.0).contains(&port) {
        return Err(InvalidPort);
    }
    Ok(Network {
        hostname,
        port: port as u16,
        mdns,
        mdns_domain,
        cors,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::cli::spec::{Invocation, ParseEnv, parse};

    fn parsed(argv: &[&str]) -> Parsed {
        let env = ParseEnv {
            version: "local".into(),
            cwd: "/work".into(),
            tty: false,
            zsh: false,
        };
        let argv: Vec<String> = argv.iter().map(|a| (*a).to_owned()).collect();
        match parse(&argv, &env) {
            Invocation::Run(parsed) => parsed,
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn defaults_without_config() {
        let network = resolve(&parsed(&["serve"]), None).unwrap();
        assert_eq!(
            network,
            Network {
                hostname: "127.0.0.1".into(),
                port: 0,
                mdns: false,
                mdns_domain: "opencode.local".into(),
                cors: vec![],
            }
        );
    }

    #[test]
    fn valid_port_boundaries_are_preserved() {
        assert_eq!(
            resolve(&parsed(&["serve", "--port=0"]), None).unwrap().port,
            0
        );
        assert_eq!(
            resolve(&parsed(&["serve", "--port=65535"]), None)
                .unwrap()
                .port,
            u16::MAX
        );
    }

    #[test]
    fn config_wins_over_defaults_and_flags_win_over_config() {
        let server = json!({ "port": 4096, "hostname": "0.0.0.0", "cors": ["https://a"] });
        let network = resolve(&parsed(&["serve"]), Some(&server)).unwrap();
        assert_eq!(network.port, 4096);
        assert_eq!(network.hostname, "0.0.0.0");
        assert_eq!(network.cors, vec!["https://a".to_owned()]);

        let network = resolve(
            &parsed(&[
                "serve",
                "--port=5000",
                "--hostname",
                "localhost",
                "--cors",
                "https://b",
            ]),
            Some(&server),
        )
        .unwrap();
        assert_eq!(network.port, 5000);
        assert_eq!(network.hostname, "localhost");
        assert_eq!(
            network.cors,
            vec!["https://a".to_owned(), "https://b".to_owned()]
        );
    }

    #[test]
    fn mdns_defaults_hostname_to_all_interfaces_unless_configured() {
        let network = resolve(&parsed(&["serve", "--mdns"]), None).unwrap();
        assert!(network.mdns);
        assert_eq!(network.hostname, "0.0.0.0");
        let network = resolve(&parsed(&["serve"]), Some(&json!({ "mdns": true }))).unwrap();
        assert!(network.mdns);
        assert_eq!(network.hostname, "0.0.0.0");
        let network = resolve(
            &parsed(&["serve"]),
            Some(&json!({ "mdns": true, "hostname": "10.0.0.1", "mdnsDomain": "x.local" })),
        )
        .unwrap();
        assert_eq!(network.hostname, "10.0.0.1");
        assert_eq!(network.mdns_domain, "x.local");
        let network = resolve(
            &parsed(&["serve", "--no-mdns"]),
            Some(&json!({ "mdns": true })),
        )
        .unwrap();
        assert!(!network.mdns);
        assert_eq!(network.hostname, "127.0.0.1");
    }
}
