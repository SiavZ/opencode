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

mod terminal;
mod turn;

use std::io::{IsTerminal, Read, Write};
use std::path::PathBuf;

use anyhow::{Context, bail};
use opencode_config::legacy::Diagnostics;
use opencode_config::legacy::parse::load_global;
use opencode_runtime::{Process, ProcessOptions};
use opencode_session_api::ids::{AbsolutePath, AgentID, LocationRef, ModelID, ProviderID};
use opencode_session_api::operations::{CreateRequest, ListRequest, ListScope, SessionService};
use opencode_session_api::{ModelRef, SessionID};

use super::dispatch::CommandError;
use super::spec::{CommandId, Parsed};

pub async fn run(parsed: &Parsed, version: &str) -> Result<(), CommandError> {
    validate(parsed)?;
    let directory = directory(parsed)?;
    let mut options =
        ProcessOptions::from_process(version, option_env!("OPENCODE_CHANNEL").unwrap_or("local"));
    for (key, value) in parsed.globals.middleware_env(std::process::id()) {
        options.environment.set_var(key, value);
    }
    load_global(&options.environment, None, &mut Diagnostics::default())
        .map_err(anyhow::Error::from)?;
    let process = Process::start(options).await.map_err(anyhow::Error::from)?;
    let result = match parsed.command {
        CommandId::Models => models(&process, parsed, directory).await,
        CommandId::Tui => terminal::run(&process, parsed, directory).await,
        CommandId::Run if parsed.args.bool("interactive") || parsed.args.bool("mini") => {
            terminal::run(&process, parsed, directory).await
        }
        CommandId::Run => noninteractive(&process, parsed, directory).await,
        _ => unreachable!("dispatcher only accepts implemented commands"),
    };
    process.shutdown().await;
    result.map_err(CommandError::from)
}

fn validate(parsed: &Parsed) -> Result<(), CommandError> {
    let unsupported = if parsed.command == CommandId::Models {
        vec!["refresh"]
    } else {
        vec![
            "attach",
            "command",
            "fork",
            "share",
            "file",
            "title",
            "auto",
            "yolo",
            "dangerously-skip-permissions",
            "demo",
            "replay-limit",
            "password",
            "username",
        ]
    };
    for key in unsupported {
        if parsed.args.bool(key)
            || parsed.args.str(key).is_some_and(|value| value != "false")
            || !parsed.args.strings(key).is_empty() && key == "file"
        {
            return Err(CommandError::Unsupported(format!("--{key}")));
        }
    }
    if parsed.args.str("variant").is_some() && parsed.args.str("model").is_none() {
        return Err(CommandError::Unsupported(
            "--variant without an explicit --model".into(),
        ));
    }
    if parsed.args.str("format").as_deref() == Some("json") {
        return Err(CommandError::Unsupported(
            "legacy JSON event formatting (--format json)".into(),
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
