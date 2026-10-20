//! Purpose: emit baseline JSON run events from real legacy Session projections.
//! Scope: one admitted legacy prompt and its process-local event subscription.
//! Uses: runtime/src/process_services.rs, event-api/src/store.rs
//! Used by: opencode/src/cli/commands/mod.rs
//! Invariants: part IDs, state and times are domain-produced; no synthesized step records.
//! Contract tests: opencode/tests/admin/json_run.rs

use crate::cli::spec::Parsed;
use anyhow::Context;
use opencode_runtime::{EventStore, Process};
use serde_json::{Value, json};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

pub async fn run(
    process: &Process,
    parsed: &Parsed,
    directory: PathBuf,
    text: String,
) -> anyhow::Result<()> {
    let mut parts = Vec::new();
    for file in parsed.args.strings("file") {
        let path = directory.join(&file);
        let metadata =
            std::fs::metadata(&path).with_context(|| format!("File not found: {file}"))?;
        let url = url::Url::from_file_path(&path)
            .map_err(|()| anyhow::anyhow!("Invalid file path: {file}"))?;
        parts.push(json!({"type":"file","url":url.as_str(),"filename":path.file_name().context("Invalid file name")?.to_string_lossy(),"mime":if metadata.is_dir() {"application/x-directory"} else {"text/plain"}}));
    }
    parts.push(json!({"type":"text","text":text}));
    let existing = if let Some(id) = parsed.args.str("session") {
        Some(
            process
                .legacy_command(
                    "session.get",
                    directory.clone(),
                    object(json!({"sessionID":id})),
                    Default::default(),
                    None,
                )
                .await?,
        )
    } else if parsed.args.bool("continue") {
        let sessions = process
            .legacy_command(
                "session.list",
                directory.clone(),
                Default::default(),
                object(json!({"roots":true,"limit":1})),
                None,
            )
            .await?;
        sessions
            .as_array()
            .and_then(|sessions| sessions.first())
            .cloned()
    } else {
        None
    };
    let session = if let Some(existing) = existing {
        if parsed.args.bool("fork") {
            process
                .legacy_command(
                    "session.fork",
                    directory.clone(),
                    object(json!({"sessionID":existing["id"]})),
                    Default::default(),
                    Some(json!({})),
                )
                .await?
        } else {
            existing
        }
    } else {
        let mut input = json!({"permission":[
            {"permission":"question","action":"deny","pattern":"*"},
            {"permission":"plan_enter","action":"deny","pattern":"*"},
            {"permission":"plan_exit","action":"deny","pattern":"*"}
        ]});
        if let Some(title) = parsed.args.str("title") {
            input["title"] = json!(if title.is_empty() {
                format!(
                    "{}{}",
                    text.chars().take(50).collect::<String>(),
                    if text.chars().count() > 50 { "..." } else { "" }
                )
            } else {
                title
            });
        }
        process
            .legacy_command(
                "session.create",
                directory.clone(),
                Default::default(),
                Default::default(),
                Some(input),
            )
            .await?
    };
    let id = session["id"]
        .as_str()
        .context("Failed to create session")?
        .to_owned();
    let mut input = json!({"parts":parts});
    if let Some(model) = parsed.args.str("model") {
        let (provider, model) = model
            .split_once('/')
            .filter(|(provider, model)| !provider.is_empty() && !model.is_empty())
            .context("model must use provider/model format")?;
        input["model"] = json!({"providerID":provider,"modelID":model});
    }
    for key in ["agent", "variant"] {
        if let Some(value) = parsed.args.str(key) {
            input[key] = json!(value);
        }
    }
    let (sender, mut receiver) = tokio::sync::mpsc::channel(1024);
    let overflow = Arc::new(AtomicBool::new(false));
    let failed = overflow.clone();
    let active = id.clone();
    let unsubscribe = process.events.listen(Arc::new(move |event| {
        if event.data["sessionID"] == active
            && sender
                .try_send((event.event_type.clone(), event.data.clone()))
                .is_err()
        {
            failed.store(true, Ordering::Release);
        }
        Ok(())
    }));
    let prompt = process.legacy_command(
        "session.prompt",
        directory.clone(),
        object(json!({"sessionID":id})),
        Default::default(),
        Some(input),
    );
    tokio::pin!(prompt);
    let mut output = std::io::stdout();
    let mut errors = Vec::new();
    let mut printed = std::collections::HashSet::new();
    let mut interrupted = false;
    let result = async { loop {
        tokio::select! {
            result = &mut prompt => break result.map(|_| ()).map_err(anyhow::Error::from),
            event = receiver.recv() => {
                if let Some((kind,data)) = event {
                    if kind == "session.error" { errors.push(error_message(&data)); }
                    if kind == "permission.asked" {
                        eprintln!("permission requested: {} ({}); auto-rejecting",data["permission"].as_str().unwrap_or_default(),data["patterns"].as_array().into_iter().flatten().filter_map(Value::as_str).collect::<Vec<_>>().join(", "));
                        process.legacy_command("permission.reply",directory.clone(),object(json!({"requestID":data["id"]})),Default::default(),Some(json!({"reply":"reject"}))).await?;
                    }
                    emit(&mut output, &id, &kind, &data, parsed.args.bool("thinking"), parsed.args.str("format").as_deref() == Some("json"), &mut printed)?;
                }
            }
            result = tokio::signal::ctrl_c() => {
                result?;
                interrupted = true;
                // The domain abort joins the active provider task and persists its terminal
                // assistant before returning. Do not wait for the prompt's fallback reply.
                process.legacy_command("session.abort",directory.clone(),object(json!({"sessionID":id})),Default::default(),None).await?;
                break Err(anyhow::anyhow!("Session interrupted"));
            }
        }
    } }.await;
    unsubscribe();
    if result.is_err() && !interrupted {
        let _ = process
            .legacy_command(
                "session.abort",
                directory,
                object(json!({"sessionID":id})),
                Default::default(),
                None,
            )
            .await;
    }
    while let Ok((kind, data)) = receiver.try_recv() {
        if kind == "session.error" {
            errors.push(error_message(&data));
        }
        emit(
            &mut output,
            &id,
            &kind,
            &data,
            parsed.args.bool("thinking"),
            parsed.args.str("format").as_deref() == Some("json"),
            &mut printed,
        )?;
    }
    result?;
    anyhow::ensure!(
        !overflow.load(Ordering::Acquire),
        "Session event subscriber overflow"
    );
    if !errors.is_empty() {
        anyhow::bail!(errors.join("\n"));
    }
    Ok(())
}

fn emit(
    output: &mut impl Write,
    session: &str,
    kind: &str,
    data: &Value,
    thinking: bool,
    json: bool,
    printed: &mut std::collections::HashSet<String>,
) -> anyhow::Result<()> {
    if data["sessionID"] != session {
        return Ok(());
    }
    let payload = if kind == "session.error" && !data["error"].is_null() {
        ("error", json!({"error":data["error"]}))
    } else if kind == "message.part.updated" {
        let part = &data["part"];
        let kind = match part["type"].as_str() {
            Some("step-start") => "step_start",
            Some("step-finish") => "step_finish",
            Some("text") if json || part.pointer("/time/end").is_some() => "text",
            Some("reasoning") if thinking && (json || part.pointer("/time/end").is_some()) => {
                "reasoning"
            }
            Some("tool")
                if matches!(
                    part["state"]["status"].as_str(),
                    Some("completed" | "error")
                ) =>
            {
                "tool_use"
            }
            _ => return Ok(()),
        };
        (kind, json!({"part":part}))
    } else if kind == "message.part.delta" && json {
        ("part_delta", data.clone())
    } else {
        return Ok(());
    };
    if !json {
        if matches!(payload.0, "text" | "reasoning") {
            let Some(part_id) = payload.1["part"]["id"].as_str() else {
                return Ok(());
            };
            if !printed.insert(part_id.to_owned()) {
                return Ok(());
            }
            writeln!(
                output,
                "{}",
                payload.1["part"]["text"]
                    .as_str()
                    .context("Invalid text part")?
            )?;
            output.flush()?;
        }
        return Ok(());
    }
    let mut record = json!({"type":payload.0,"timestamp":std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?.as_millis(),"sessionID":session});
    record
        .as_object_mut()
        .expect("literal object")
        .extend(object(payload.1));
    writeln!(output, "{}", serde_json::to_string(&record)?)?;
    output.flush()?;
    Ok(())
}

fn error_message(data: &Value) -> String {
    data.pointer("/error/data/message")
        .and_then(Value::as_str)
        .or_else(|| data.pointer("/error/name").and_then(Value::as_str))
        .unwrap_or("Session error")
        .to_owned()
}

fn object(value: Value) -> serde_json::Map<String, Value> {
    value.as_object().expect("literal object").clone()
}
