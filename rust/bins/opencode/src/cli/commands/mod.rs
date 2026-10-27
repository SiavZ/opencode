//! Purpose: production CLI composition for local run, models, and a basic interactive terminal.
//! Scope: process command lifetime.
//! Uses:
//! rust/bins/
//! |-- opencode/src/cli/commands/terminal.rs
//! |-- opencode/src/cli/commands/turn.rs
//! |-- opencode/src/cli/dispatch.rs
//! `-- opencode/src/cli/spec/mod.rs
//! rust/crates/
//! |-- config/src/legacy/mod.rs
//! |-- config/src/legacy/parse.rs
//! |-- runtime/src/lib.rs
//! |-- session-api/src/ids.rs
//! |-- session-api/src/lib.rs
//! `-- session-api/src/operations.rs
//! External:
//! |-- anyhow::Context
//! |-- anyhow::Error::from
//! |-- anyhow::Result
//! |-- anyhow::bail
//! |-- serde_json::from_value
//! `-- serde_json::json
//! Used by:
//! rust/bins/
//! |-- opencode/src/cli/commands/terminal.rs
//! |-- opencode/src/cli/dispatch.rs
//! `-- opencode/src/cli/mod.rs
//! Invariants: execution uses durable admission then an explicitly joined process drain.
//! Unsupported options fail before admission rather than silently changing user intent.
//! Contract tests: opencode/tests/commands.rs

mod agents;
mod config;
mod database;
mod diagnostics;
mod integrations;
mod json_run;
mod providers;
mod sanitize;
mod sessions;
mod terminal;
mod turn;

use std::io::{IsTerminal, Read, Write};
use std::path::PathBuf;

use anyhow::{Context, bail};
use opencode_runtime::{Process, ProcessOptions};
use opencode_session_api::ids::{AbsolutePath, AgentID, LocationRef, ModelID, ProviderID};
use opencode_session_api::operations::{CreateRequest, ListRequest, ListScope, SessionService};
use opencode_session_api::{ModelRef, SessionID};

use super::dispatch::CommandError;
use super::spec::{CommandId, Parsed};

pub async fn run(parsed: &Parsed, version: &str) -> Result<(), CommandError> {
    if parsed.command == CommandId::DebugPaths {
        let paths = ProcessOptions::from_process(version, "local").paths;
        paths.ensure().await.map_err(anyhow::Error::from)?;
        let mut output = std::io::stdout().lock();
        for (name, path) in [
            ("home", &paths.home),
            ("data", &paths.data),
            ("bin", &paths.bin),
            ("log", &paths.log),
            ("repos", &paths.repos),
            ("cache", &paths.cache),
            ("config", &paths.config),
            ("state", &paths.state),
            ("tmp", &paths.tmp),
        ] {
            writeln!(output, "{name:10} {}", path.display()).map_err(anyhow::Error::from)?;
        }
        return Ok(());
    }
    if parsed.command == CommandId::DebugWait {
        tokio::select! {
            () = tokio::time::sleep(std::time::Duration::from_secs(86400)) => {}
            result = tokio::signal::ctrl_c() => result.map_err(anyhow::Error::from)?,
        }
        return Ok(());
    }
    validate(parsed)?;
    let directory = directory(parsed)?;
    let mut options =
        ProcessOptions::from_process(version, option_env!("OPENCODE_CHANNEL").unwrap_or("local"));
    for (key, value) in parsed.globals.middleware_env(std::process::id()) {
        options.environment.set_var(key, value);
    }
    if !matches!(
        parsed.command,
        CommandId::DbQuery
            | CommandId::DbPath
            | CommandId::ProvidersList
            | CommandId::ProvidersLogout
    ) {
        config::validate(&options.environment, &directory).map_err(anyhow::Error::from)?;
    }
    let process = Process::start(options).await.map_err(anyhow::Error::from)?;
    let result: anyhow::Result<()> = async {
        match parsed.command {
            CommandId::Models => models(&process, parsed, directory).await,
            CommandId::Tui => terminal::run(&process, parsed, directory).await,
            CommandId::Run if parsed.args.bool("interactive") || parsed.args.bool("mini") => {
                terminal::run(&process, parsed, directory).await
            }
            CommandId::Run => noninteractive(&process, parsed, directory).await,
            CommandId::ProvidersList | CommandId::ProvidersLogout => {
                providers::run(&process, parsed).await
            }
            CommandId::AgentList => agents::list(&process, directory).await,
            CommandId::DbQuery => database::run(&process, parsed).await,
            CommandId::DbPath => {
                println!("{}", process.database_path().display());
                Ok(())
            }
            CommandId::DebugConfig => {
                let mut config = process.resolved_config(directory).await?;
                diagnostics::redact(&mut config, false);
                println!(
                    "{}",
                    serde_json::to_string_pretty(&config).map_err(anyhow::Error::from)?
                );
                Ok(())
            }
            CommandId::McpList
            | CommandId::McpAuthList
            | CommandId::McpLogout
            | CommandId::DebugSkill => integrations::run(&process, parsed, directory).await,
            CommandId::SessionList
            | CommandId::SessionDelete
            | CommandId::Import
            | CommandId::Export => sessions::run(&process, parsed, directory).await,
            _ => unreachable!("dispatcher only accepts implemented commands"),
        }
    }
    .await;
    process.shutdown().await;
    result.map_err(CommandError::from)
}

fn validate(parsed: &Parsed) -> Result<(), CommandError> {
    let unsupported = match parsed.command {
        CommandId::McpLogout if parsed.args.str("name").is_none() => {
            Some("interactive MCP logout picker; provide name")
        }
        CommandId::ProvidersLogout if parsed.args.str("provider").is_none() => {
            Some("interactive provider logout picker; provide provider")
        }
        CommandId::SessionList if parsed.args.str("format").as_deref() != Some("json") => {
            Some("session list table locale and pager; use --format json")
        }
        CommandId::Import
            if parsed.args.str("file").is_some_and(|file| {
                file.starts_with("http://") || file.starts_with("https://")
            }) =>
        {
            Some("share URL import")
        }
        CommandId::Export if parsed.args.str("sessionID").is_none() => {
            Some("interactive export session picker; provide sessionID")
        }
        _ => None,
    };
    if let Some(unsupported) = unsupported {
        return Err(CommandError::Unsupported(unsupported.into()));
    }
    if parsed.command == CommandId::DbQuery
        && parsed
            .args
            .str("query")
            .is_none_or(|query| query.is_empty())
    {
        return Err(CommandError::Unsupported(
            "interactive sqlite3 shell; provide SQL query".into(),
        ));
    }
    if parsed.args.bool("fork")
        && !parsed.args.bool("continue")
        && parsed.args.str("session").is_none()
    {
        return Err(anyhow::anyhow!("--fork requires --continue or --session").into());
    }
    if !matches!(
        parsed.command,
        CommandId::Run | CommandId::Tui | CommandId::Models
    ) {
        return Ok(());
    }
    let unsupported = if parsed.command == CommandId::Models {
        vec![]
    } else {
        vec![
            "attach",
            "command",
            "share",
            "file",
            "auto",
            "yolo",
            "dangerously-skip-permissions",
            "demo",
            "replay-limit",
            "password",
            "username",
        ]
    };
    if parsed.args.str("title").is_some()
        && (parsed.command != CommandId::Run
            || parsed.args.bool("interactive")
            || parsed.args.bool("mini"))
    {
        return Err(CommandError::Unsupported("interactive --title".into()));
    }
    for key in unsupported {
        if key == "file"
            && parsed.command == CommandId::Run
            && !parsed.args.bool("interactive")
            && !parsed.args.bool("mini")
        {
            continue;
        }
        if parsed.args.bool(key)
            || parsed.args.str(key).is_some_and(|value| value != "false")
            || !parsed.args.strings(key).is_empty() && key == "file"
        {
            return Err(CommandError::Unsupported(format!("--{key}")));
        }
    }
    if parsed.args.str("format").as_deref() == Some("json")
        && (parsed.command != CommandId::Run
            || parsed.args.bool("interactive")
            || parsed.args.bool("mini"))
    {
        return Err(CommandError::Unsupported(
            "interactive JSON event formatting (--format json)".into(),
        ));
    }
    if parsed.command == CommandId::Run
        && parsed
            .raw
            .iter()
            .take_while(|arg| arg.as_str() != "--")
            .any(|arg| arg == "--port" || arg.starts_with("--port="))
    {
        return Err(CommandError::Unsupported(
            "run --port (no local HTTP listener is needed)".into(),
        ));
    }
    if parsed.command == CommandId::Tui {
        for key in [
            "--port",
            "--hostname",
            "--mdns",
            "--no-mdns",
            "--mdns-domain",
            "--cors",
            "--no-replay",
            "--replay",
        ] {
            if parsed
                .raw
                .iter()
                .take_while(|arg| arg.as_str() != "--")
                .any(|arg| arg == key || arg.starts_with(&format!("{key}=")))
            {
                return Err(CommandError::Unsupported(format!("TUI {key}")));
            }
        }
    }
    Ok(())
}

fn directory(parsed: &Parsed) -> anyhow::Result<PathBuf> {
    let cwd = std::env::current_dir()?;
    let value = if parsed.command == CommandId::Tui {
        parsed.args.str("project")
    } else {
        parsed.args.str("dir")
    };
    let path = value.map_or(cwd.clone(), |value| cwd.join(value));
    let path = path
        .canonicalize()
        .with_context(|| format!("Failed to change directory to {}", path.display()))?;
    if !path.is_dir() {
        bail!("Not a directory: {}", path.display());
    }
    Ok(path)
}

async fn models(process: &Process, parsed: &Parsed, directory: PathBuf) -> anyhow::Result<()> {
    if parsed.args.bool("refresh") {
        process.refresh_models(directory.clone(), true).await?;
        println!("Models cache refreshed");
    }
    let mut models = process.available_models(directory).await?;
    if let Some(provider) = parsed.args.str("provider") {
        models.retain(|model| model.provider_id.as_str() == provider);
        if models.is_empty() {
            bail!("Provider not found: {provider}");
        }
    }
    models.sort_by(|a, b| {
        (
            !a.provider_id.as_str().starts_with("opencode"),
            a.provider_id.as_str(),
            a.id.as_str(),
        )
            .cmp(&(
                !b.provider_id.as_str().starts_with("opencode"),
                b.provider_id.as_str(),
                b.id.as_str(),
            ))
    });
    let mut output = std::io::stdout().lock();
    for model in models {
        writeln!(output, "{}/{}", model.provider_id, model.id)?;
        if parsed.args.bool("verbose") {
            writeln!(output, "{}", serde_json::to_string_pretty(&model)?)?;
        }
    }
    Ok(())
}

async fn session(
    process: &Process,
    parsed: &Parsed,
    directory: PathBuf,
) -> anyhow::Result<SessionID> {
    let model = parsed
        .args
        .str("model")
        .map(|model| -> anyhow::Result<ModelRef> {
            let (provider, id) = model
                .split_once('/')
                .context("model must use provider/model format")?;
            if provider.is_empty() || id.is_empty() {
                bail!("model must use provider/model format");
            }
            Ok(ModelRef {
                provider_id: ProviderID::new(provider),
                id: ModelID::new(id),
                variant: parsed
                    .args
                    .str("variant")
                    .map(|variant| serde_json::from_value(serde_json::json!(variant)))
                    .transpose()?,
            })
        })
        .transpose()?;
    let existing = if let Some(id) = parsed.args.str("session") {
        Some(process.session.get(&SessionID::new(id)?).await?)
    } else if parsed.args.bool("continue") {
        process
            .session
            .list(ListRequest {
                scope: ListScope::Directory(directory.to_string_lossy().into_owned()),
                limit: Some(1),
                ..ListRequest::default()
            })
            .await?
            .into_iter()
            .next()
    } else {
        None
    };
    let model = if model.is_none() && parsed.args.str("variant").is_some() {
        let mut selected = match existing.as_ref().and_then(|session| session.model.clone()) {
            Some(model) => Some(model),
            None => process.default_model(directory.clone()).await?,
        }
        .context("No model is available")?;
        selected.variant = parsed
            .args
            .str("variant")
            .map(|variant| serde_json::from_value(serde_json::json!(variant)))
            .transpose()?;
        Some(selected)
    } else {
        model
    };
    let existing = if let Some(session) = existing {
        Some(if parsed.args.bool("fork") {
            process.session.fork(&session.id, None).await?
        } else {
            session
        })
    } else {
        None
    };
    if let Some(session) = existing {
        if let Some(model) = model {
            process.session.switch_model(&session.id, model).await?;
        }
        if let Some(agent) = parsed.args.str("agent") {
            process.session.switch_agent(&session.id, &agent).await?;
        }
        return Ok(session.id);
    }
    Ok(process
        .session
        .create(CreateRequest {
            id: None,
            agent: parsed.args.str("agent").map(AgentID::new),
            model,
            location: LocationRef {
                directory: AbsolutePath::new(directory.to_string_lossy().into_owned()),
                workspace_id: None,
            },
        })
        .await?
        .id)
}

async fn noninteractive(
    process: &Process,
    parsed: &Parsed,
    directory: PathBuf,
) -> anyhow::Result<()> {
    let mut message = parsed.args.strings("message");
    message.extend(parsed.double_dash.iter().cloned());
    let message = message.join(" ");
    let mut piped = String::new();
    if !std::io::stdin().is_terminal() {
        std::io::stdin().read_to_string(&mut piped)?;
    }
    let text = match (message.is_empty(), piped.is_empty()) {
        (false, false) => format!("{message}\n{piped}"),
        (true, _) => piped,
        _ => message,
    };
    if text.trim().is_empty() {
        bail!("You must provide a message or a command");
    }
    if parsed.args.str("format").as_deref() == Some("json")
        || !parsed.args.strings("file").is_empty()
        || parsed.args.str("title").is_some()
    {
        return json_run::run(process, parsed, directory, text).await;
    }
    let id = session(process, parsed, directory).await?;
    let mut output = std::io::stdout();
    turn::execute(process, &id, text, parsed.args.bool("thinking"), |text| {
        output.write_all(text.as_bytes())?;
        output.flush()?;
        Ok(())
    })
    .await?;
    writeln!(output)?;
    Ok(())
}
