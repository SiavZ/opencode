//! Purpose: real cross-platform PTY acceptance through public create/write/attach/resize/remove/shutdown.
//! Scope: isolated native terminal processes, no mocked terminal backend.
//! Uses:
//! rust/crates/
//! |-- platform-api/src/pty.rs
//! `-- platform-native/src/pty/mod.rs
//! External:
//! |-- async_trait::async_trait
//! |-- tempfile::tempdir
//! `-- tokio::time::timeout
//! Used by:
//! `-- none, entrypoint
//! Invariants: tests assert terminal output, not merely process spawn; teardown joins native jobs.
//! Contract tests: this file

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use opencode_platform_api::pty::{
    PtyCreateInput, PtyError, PtyEvent, PtyOutput, PtyService, PtySize, PtyStatus, PtyUpdateInput,
    ReplayFrom,
};
use opencode_platform_native::pty::{NativePty, PtyEvents};

#[derive(Default)]
struct Events(Mutex<Vec<PtyEvent>>);

#[async_trait]
impl PtyEvents for Events {
    async fn publish(&self, event: PtyEvent) -> Result<(), PtyError> {
        self.0.lock().unwrap().push(event);
        Ok(())
    }
}

async fn receive(
    attachment: &dyn opencode_platform_api::pty::PtyAttachment,
    needle: &str,
) -> String {
    let mut text = String::new();
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            match attachment.next().await {
                Some(PtyOutput::Data(chunk)) => text.push_str(&chunk),
                output => panic!("terminal ended before {needle}: {output:?}; output={text:?}"),
            }
            if text.contains(needle) {
                return text.clone();
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("PTY output timed out waiting for {needle:?}; collected={text:?}"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_pty_write_read_resize_remove_and_shutdown() {
    let root = tempfile::tempdir().unwrap();
    let events = Arc::new(Events::default());
    let shell = if cfg!(windows) {
        "powershell.exe"
    } else {
        "/bin/sh"
    };
    let mut environment: std::collections::BTreeMap<String, String> = std::env::vars().collect();
    environment.insert("HOME".into(), root.path().display().to_string());
    environment.insert("USERPROFILE".into(), root.path().display().to_string());
    environment.insert(
        "HISTFILE".into(),
        root.path().join("history").display().to_string(),
    );
    let service = NativePty::new(root.path().into(), shell.into(), events.clone())
        .with_environment(environment);
    let info = service
        .create(PtyCreateInput {
            args: cfg!(windows).then(|| vec!["-NoLogo".into(), "-NoProfile".into()]),
            ..PtyCreateInput::default()
        })
        .await
        .unwrap();
    assert_eq!(info.status, PtyStatus::Running);
    assert!(info.pid > 0);
    let attachment = service.attach(&info.id, ReplayFrom::Start).await.unwrap();
    let cursor = attachment.cursor();
    attachment.activate();
    attachment
        .write(if cfg!(windows) {
            "echo ''; echo PTY_NATIVE_READY\r\n"
        } else {
            "printf '\\nPTY_NATIVE_READY\\n'\r\n"
        })
        .unwrap();
    receive(attachment.as_ref(), "\r\nPTY_NATIVE_READY\r\n").await;
    let update = service
        .update(
            &info.id,
            PtyUpdateInput {
                title: Some("Resized terminal".into()),
                size: Some(PtySize { rows: 37, cols: 91 }),
            },
        )
        .await
        .unwrap();
    assert_eq!(update.title, "Resized terminal");
    if cfg!(windows) {
        attachment
            .write("[Console]::IsInputRedirected\r\n")
            .unwrap();
        receive(attachment.as_ref(), "\r\nFalse\r\n").await;
    } else {
        attachment
            .write("stty size; test -t 0 && echo PTY_IS_TTY\r\n")
            .unwrap();
        let output = receive(attachment.as_ref(), "\r\nPTY_IS_TTY\r\n").await;
        assert!(
            output.contains("37 91"),
            "resize was not applied: {output:?}"
        );
    }
    let replay = service
        .attach(&info.id, ReplayFrom::Cursor(cursor))
        .await
        .unwrap();
    assert!(replay.cursor() > cursor);
    assert!(replay.replay().contains("PTY_NATIVE_READY"));
    replay.detach();
    tokio::time::timeout(Duration::from_secs(10), service.remove(&info.id))
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        service.get(&info.id).await,
        Err(PtyError::NotFound(_))
    ));
    let mut ended = false;
    while let Some(output) = attachment.next().await {
        if matches!(output, PtyOutput::End { .. }) {
            ended = true;
        }
    }
    assert!(ended);
    let another = service
        .create(PtyCreateInput {
            args: cfg!(windows).then(|| vec!["-NoLogo".into(), "-NoProfile".into()]),
            ..PtyCreateInput::default()
        })
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), service.shutdown())
        .await
        .unwrap();
    assert!(service.list().await.is_empty());
    assert!(matches!(
        service.get(&another.id).await,
        Err(PtyError::NotFound(_))
    ));
    assert!(service.create(PtyCreateInput::default()).await.is_err());
    let events = events.0.lock().unwrap();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, PtyEvent::Created(value) if value.id == info.id))
    );
    assert!(events.iter().any(
        |event| matches!(event, PtyEvent::Updated(value) if value.title == "Resized terminal")
    ));
    assert!(
        events
            .iter()
            .any(|event| matches!(event, PtyEvent::Deleted { id } if id == &info.id))
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exited_native_pty_is_retained_and_cannot_attach() {
    let root = tempfile::tempdir().unwrap();
    let events = Arc::new(Events::default());
    let service = NativePty::new(root.path().into(), "/bin/sh".into(), events.clone());
    let info = service
        .create(PtyCreateInput {
            command: Some("/bin/sh".into()),
            args: Some(vec!["-c".into(), "printf native_exit; exit 7".into()]),
            env: Some(std::collections::BTreeMap::from([(
                "HOME".into(),
                root.path().display().to_string(),
            )])),
            ..PtyCreateInput::default()
        })
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let retained = service.get(&info.id).await.unwrap();
            if retained.status == PtyStatus::Exited {
                assert_eq!(retained.exit_code, Some(7));
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(matches!(
        service.attach(&info.id, ReplayFrom::Start).await,
        Err(PtyError::Exited(_))
    ));
    assert_eq!(service.list().await.len(), 1);
    service.remove(&info.id).await.unwrap();
    service.shutdown().await;
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_pty_retention_follows_exit_order_and_joins_evictions() {
    let root = tempfile::tempdir().unwrap();
    let events = Arc::new(Events::default());
    let service = NativePty::new(root.path().into(), "/bin/sh".into(), events.clone());
    let mut ids = Vec::new();
    for _ in 0..27 {
        let info = service
            .create(PtyCreateInput {
                args: Some(vec!["-c".into(), "exit 0".into()]),
                ..PtyCreateInput::default()
            })
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if events
                    .0
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|event| matches!(event, PtyEvent::Exited { id, .. } if id == &info.id))
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        ids.push(info.id);
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        while service.list().await.len() != 25 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(matches!(
        service.get(&ids[0]).await,
        Err(PtyError::NotFound(_))
    ));
    assert!(matches!(
        service.get(&ids[1]).await,
        Err(PtyError::NotFound(_))
    ));
    assert_eq!(service.get(&ids[26]).await.unwrap().exit_code, Some(0));
    for id in &ids {
        let events = events.0.lock().unwrap();
        let created = events
            .iter()
            .position(|event| matches!(event, PtyEvent::Created(info) if &info.id == id))
            .unwrap();
        let exited = events
            .iter()
            .position(|event| matches!(event, PtyEvent::Exited { id: found, .. } if found == id))
            .unwrap();
        assert!(created < exited);
    }
    tokio::time::timeout(Duration::from_secs(10), service.shutdown())
        .await
        .unwrap();
}

struct PendingCreation {
    entered: tokio::sync::Notify,
}

#[async_trait]
impl PtyEvents for PendingCreation {
    async fn publish(&self, event: PtyEvent) -> Result<(), PtyError> {
        if matches!(event, PtyEvent::Created(_)) {
            self.entered.notify_one();
            std::future::pending::<()>().await;
        }
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn canceled_native_pty_creation_is_killed_and_joined() {
    let root = tempfile::tempdir().unwrap();
    let events = Arc::new(PendingCreation {
        entered: tokio::sync::Notify::new(),
    });
    let shell = if cfg!(windows) {
        "powershell.exe"
    } else {
        "/bin/sh"
    };
    let service = Arc::new(NativePty::new(
        root.path().into(),
        shell.into(),
        events.clone(),
    ));
    let creation = {
        let service = service.clone();
        tokio::spawn(async move { service.create(PtyCreateInput::default()).await })
    };
    tokio::time::timeout(Duration::from_secs(10), events.entered.notified())
        .await
        .unwrap();
    assert_eq!(service.list().await.len(), 1);
    creation.abort();
    assert!(creation.await.unwrap_err().is_cancelled());
    tokio::time::timeout(Duration::from_secs(10), service.shutdown())
        .await
        .unwrap();
    assert!(service.list().await.is_empty());
}
