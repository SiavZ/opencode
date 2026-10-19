//! Purpose: present and settle real location-owned permission requests during provider execution.
//! Scope: one active session execution.
//! Uses:
//! rust/crates/
//! |-- runtime/src/lib.rs
//! |-- ui-terminal/src/app.rs
//! `-- ui-terminal/src/dialog/mod.rs
//! External:
//! `-- anyhow::Context
//! Used by:
//! rust/bins/
//! `-- opencode/src/cli/commands/terminal.rs
//! Invariants: no approval is inferred; each choice replies to an actual pending request ID.
//! Contract tests: opencode/tests/terminal_app.rs

use anyhow::Context;
use opencode_runtime::{Process, Reply, ReplyInput};
use opencode_ui_terminal::app::App;
use opencode_ui_terminal::dialog::{Choice, Dialog, Kind};
use std::path::Path;

pub async fn poll(
    process: &Process,
    app: &mut App,
    directory: &Path,
    session: &str,
) -> anyhow::Result<()> {
    let request = process
        .pending_permissions(directory.to_path_buf())
        .await?
        .into_iter()
        .find(|request| request.session_id == session);
    if let Some(request) = request {
        if app
            .dialogs
            .top()
            .is_some_and(|dialog| matches!(dialog.kind, Kind::Permission | Kind::PermissionConfirm))
        {
            return Ok(());
        }
        let choices = [
            ("once", "Allow once"),
            ("always", "Always allow"),
            ("reject", "Reject"),
        ]
        .into_iter()
        .map(|(value, title)| {
            Choice::new(
                format!("{}/{value}", request.id),
                title,
                request.resources.join(", "),
            )
        })
        .collect();
        app.dialogs.push(Dialog::new(
            Kind::Permission,
            format!("Permission required: {}", request.action),
            choices,
        ));
    }
    Ok(())
}

pub async fn reply(
    process: &Process,
    app: &mut App,
    directory: &Path,
    value: &str,
) -> anyhow::Result<()> {
    if value.is_empty() {
        return Ok(());
    }
    let (id, reply) = value
        .rsplit_once('/')
        .context("Invalid permission choice")?;
    if reply == "always" {
        app.picker(
            Kind::PermissionConfirm,
            "Always allow this permission?",
            vec![
                Choice::new(
                    format!("{id}/always-confirm"),
                    "Confirm",
                    "Save this grant for future requests",
                ),
                Choice::new("", "Cancel", "Return without approving"),
            ],
        );
        return Ok(());
    }
    let reply = match reply {
        "once" => Reply::Once,
        "always-confirm" => Reply::Always,
        "reject" => Reply::Reject,
        _ => anyhow::bail!("Invalid permission reply"),
    };
    process
        .reply_permission(
            directory.to_path_buf(),
            ReplyInput {
                request_id: id.into(),
                reply,
                message: None,
            },
        )
        .await?;
    Ok(())
}
