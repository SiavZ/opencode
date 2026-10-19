//! Purpose: real native PTY service matching core pty.ts lifecycle, retained output and attachment semantics.
//! Scope: Location-owned terminal processes and joined reader/wait tasks.
//! Uses:
//! rust/crates/
//! |-- platform-api/src/pty.rs
//! |-- platform-native/src/process/kill.rs
//! |-- platform-native/src/pty/attachment.rs
//! `-- platform-native/src/pty/state.rs
//! External:
//! |-- async_trait::async_trait
//! |-- portable_pty::native_pty_system
//! `-- tokio::task::JoinHandle
//! Used by:
//! rust/crates/
//! `-- platform-native/src/lib.rs
//! Invariants: no pipe masquerades as a PTY; shutdown kills and joins every owned process and reader.
//! Contract tests: platform-native/tests/pty.rs

mod attachment;
mod state;

use std::collections::{BTreeMap, VecDeque};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use opencode_platform_api::pty::{
    EXITED_LIMIT, PtyAttachment, PtyCreateInput, PtyError, PtyEvent, PtyInfo, PtyService,
    PtyStatus, PtyUpdateInput, ReplayFrom,
};
use opencode_platform_api::{PlatformError, PlatformErrorKind};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use tokio::task::JoinHandle;

use state::{State, Subscriber, lock};

#[async_trait]
pub trait PtyEvents: Send + Sync {
    async fn publish(&self, event: PtyEvent) -> Result<(), PtyError>;
}

struct Session {
    state: Arc<Mutex<State>>,
    task: tokio::sync::Mutex<Option<JoinHandle<()>>>,
}

pub struct NativePty {
    directory: PathBuf,
    shell: String,
    environment: Option<BTreeMap<String, String>>,
    events: Arc<dyn PtyEvents>,
    sessions: Arc<Mutex<Vec<Arc<Session>>>>,
    exits: Arc<Mutex<VecDeque<String>>>,
    retired: Arc<Mutex<Vec<Arc<Session>>>>,
    lifecycle: tokio::sync::Mutex<()>,
    closed: AtomicBool,
}

impl std::fmt::Debug for NativePty {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativePty")
            .field("directory", &self.directory)
            .finish_non_exhaustive()
    }
}

impl NativePty {
    pub fn new(directory: PathBuf, shell: String, events: Arc<dyn PtyEvents>) -> Self {
        Self {
            directory,
            shell,
            environment: None,
            events,
            sessions: Arc::new(Mutex::new(Vec::new())),
            exits: Arc::new(Mutex::new(VecDeque::new())),
            retired: Arc::new(Mutex::new(Vec::new())),
            lifecycle: tokio::sync::Mutex::new(()),
            closed: AtomicBool::new(false),
        }
    }

    pub fn with_environment(mut self, environment: BTreeMap<String, String>) -> Self {
        self.environment = Some(environment);
        self
    }

    fn session(&self, id: &str) -> Result<Arc<Mutex<State>>, PtyError> {
        lock(&self.sessions)
            .iter()
            .find(|session| lock(&session.state).info.id == id)
            .map(|session| session.state.clone())
            .ok_or_else(|| PtyError::NotFound(id.to_owned()))
    }

    async fn remove_owned(&self, id: &str) -> Result<(), PtyError> {
        let session = lock(&self.sessions)
            .iter()
            .find(|session| lock(&session.state).info.id == id)
            .cloned()
            .ok_or_else(|| PtyError::NotFound(id.to_owned()))?;
        {
            let mut state = lock(&session.state);
            state.kill();
            state.end(None);
        }
        join(&session).await?;
        lock(&self.sessions).retain(|item| !Arc::ptr_eq(item, &session));
        lock(&self.exits).retain(|item| item != id);
        self.events
            .publish(PtyEvent::Deleted { id: id.to_owned() })
            .await
    }
}

#[async_trait]
impl PtyService for NativePty {
    async fn list(&self) -> Vec<PtyInfo> {
        lock(&self.sessions)
            .iter()
            .map(|session| lock(&session.state).info.clone())
            .collect()
    }
    async fn get(&self, id: &str) -> Result<PtyInfo, PtyError> {
        let state = self.session(id)?;
        Ok(lock(&state).info.clone())
    }
    async fn create(&self, input: PtyCreateInput) -> Result<PtyInfo, PtyError> {
        let _lifecycle = self.lifecycle.lock().await;
        if self.closed.load(Ordering::Acquire) {
            return Err(failure("create", "PTY service closed"));
        }
        let retired = lock(&self.retired).clone();
        for session in retired {
            join(&session).await?;
            lock(&self.retired).retain(|item| !Arc::ptr_eq(item, &session));
        }
        let id = format!("pty_{}", opencode_schema::identifier::ascending());
        let command = input
            .command
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| self.shell.clone());
        let mut args = input.args.unwrap_or_default();
        if matches!(
            std::path::Path::new(&command)
                .file_name()
                .and_then(|name| name.to_str()),
            Some("bash" | "dash" | "zsh" | "fish" | "sh" | "ksh")
        ) {
            args.push("-l".to_owned());
        }
        let cwd = input
            .cwd
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| self.directory.clone());
        let pair = native_pty_system()
            .openpty(PtySize::default())
            .map_err(|error| failure("openpty", error))?;
        let mut cmd = CommandBuilder::new(&command);
        cmd.args(&args);
        cmd.cwd(&cwd);
        if let Some(environment) = &self.environment {
            cmd.env_clear();
            for (key, value) in environment {
                cmd.env(key, value);
            }
        }
        for (key, value) in input.env.unwrap_or_default() {
            cmd.env(key, value);
        }
        cmd.env("TERM", "xterm-256color");
        cmd.env("OPENCODE_TERMINAL", "1");
        #[cfg(windows)]
        for key in ["LC_ALL", "LC_CTYPE", "LANG"] {
            cmd.env(key, "C.UTF-8");
        }
        // Prepare every fallible handle before spawning so setup errors cannot orphan a child.
        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|error| failure("reader", error))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|error| failure("writer", error))?;
        let mut child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|error| failure("spawn", error))?;
        drop(pair.slave);
        let info = PtyInfo {
            id: id.clone(),
            title: input
                .title
                .filter(|title| !title.is_empty())
                .unwrap_or_else(|| format!("Terminal {}", &id[id.len() - 4..])),
            command,
            args,
            cwd: cwd.display().to_string(),
            status: PtyStatus::Running,
            pid: child.process_id().unwrap_or(0),
            exit_code: None,
        };
        let (input, incoming) = std::sync::mpsc::sync_channel::<String>(64);
        let writing = tokio::task::spawn_blocking(move || {
            let mut writer = writer;
            while let Ok(text) = incoming.recv() {
                if writer
                    .write_all(text.as_bytes())
                    .and_then(|()| writer.flush())
                    .is_err()
                {
                    break;
                }
            }
        });
        let state = Arc::new(Mutex::new(State {
            info: info.clone(),
            buffer: VecDeque::new(),
            cursor: 0,
            subscribers: BTreeMap::new(),
            next: 0,
            master: pair.master,
            writer: Some(input),
            killer: child.clone_killer(),
        }));
        let reading = state.clone();
        let read = tokio::task::spawn_blocking(move || read_output(reader, reading));
        let waiting = tokio::task::spawn_blocking(move || child.wait());
        let (created, creation) = tokio::sync::oneshot::channel();
        let mut admission = CreationGuard(Some(state.clone()));
        let done = state.clone();
        let events = self.events.clone();
        let sessions = self.sessions.clone();
        let exits = self.exits.clone();
        let retired = self.retired.clone();
        let task = tokio::spawn(async move {
            let status = waiting
                .await
                .ok()
                .and_then(Result::ok)
                .map(|status| status.exit_code());
            // Reap descendants still holding the PTY before joining its blocking reader.
            lock(&done).kill();
            let _ = read.await;
            let _ = writing.await;
            lock(&done).end(status);
            let published = creation.await.is_ok();
            if let Some(exit_code) = status.filter(|_| published) {
                lock(&exits).push_back(id.clone());
                if let Err(error) = events.publish(PtyEvent::Exited { id, exit_code }).await {
                    tracing::warn!(%error, "PTY exit event failed");
                }
                loop {
                    let oldest = {
                        let mut exits = lock(&exits);
                        if exits.len() > EXITED_LIMIT {
                            exits.pop_front()
                        } else {
                            None
                        }
                    };
                    let Some(oldest) = oldest else {
                        break;
                    };
                    let session = lock(&sessions)
                        .iter()
                        .find(|session| lock(&session.state).info.id == oldest)
                        .cloned();
                    if let Some(session) = session {
                        lock(&session.state).buffer.clear();
                        lock(&retired).push(session.clone());
                        lock(&sessions).retain(|item| !Arc::ptr_eq(item, &session));
                        if let Err(error) = events.publish(PtyEvent::Deleted { id: oldest }).await {
                            tracing::warn!(%error, "PTY eviction event failed");
                        }
                    }
                }
            }
        });
        lock(&self.sessions).push(Arc::new(Session {
            state,
            task: tokio::sync::Mutex::new(Some(task)),
        }));
        if let Err(error) = self.events.publish(PtyEvent::Created(info.clone())).await {
            drop(created);
            self.remove_owned(&info.id).await?;
            return Err(error);
        }
        admission.0.take();
        let _ = created.send(());
        Ok(info)
    }
    async fn update(&self, id: &str, input: PtyUpdateInput) -> Result<PtyInfo, PtyError> {
        let state = self.session(id)?;
        let info = {
            let mut state = lock(&state);
            if let Some(title) = input.title.filter(|title| !title.is_empty()) {
                state.info.title = title;
            }
            if let Some(size) = input
                .size
                .filter(|_| state.info.status == PtyStatus::Running)
            {
                state
                    .master
                    .resize(PtySize {
                        rows: size.rows,
                        cols: size.cols,
                        pixel_width: 0,
                        pixel_height: 0,
                    })
                    .map_err(|error| failure("resize", error))?;
            }
            state.info.clone()
        };
        self.events.publish(PtyEvent::Updated(info.clone())).await?;
        Ok(info)
    }
    async fn remove(&self, id: &str) -> Result<(), PtyError> {
        let _lifecycle = self.lifecycle.lock().await;
        self.remove_owned(id).await
    }
    async fn write(&self, id: &str, data: &str) -> Result<(), PtyError> {
        let state = self.session(id)?;
        write(&mut lock(&state), data)
    }
    async fn attach(&self, id: &str, from: ReplayFrom) -> Result<Box<dyn PtyAttachment>, PtyError> {
        let state = self.session(id)?;
        let mut session = lock(&state);
        if session.info.status != PtyStatus::Running {
            return Err(PtyError::Exited(id.to_owned()));
        }
        let cursor = session.cursor;
        let start = cursor.saturating_sub(session.buffer.len() as u64);
        let from = match from {
            ReplayFrom::Start => start,
            ReplayFrom::End => cursor,
            ReplayFrom::Cursor(value) => value.max(start).min(cursor),
        };
        let units: Vec<u16> = session
            .buffer
            .iter()
            .skip((from - start) as usize)
            .copied()
            .collect();
        let replay = String::from_utf16_lossy(&units);
        let subscriber = Arc::new(Subscriber {
            queue: Mutex::new((VecDeque::new(), 0, false)),
            ready: tokio::sync::Notify::new(),
        });
        let token = session.next;
        session.next += 1;
        session.subscribers.insert(token, subscriber.clone());
        drop(session);
        Ok(Box::new(attachment::Attachment {
            state: Arc::downgrade(&state),
            subscriber,
            id: token,
            replay,
            cursor,
            active: AtomicBool::new(false),
            detached: AtomicBool::new(false),
        }))
    }
    async fn shutdown(&self) {
        let _lifecycle = self.lifecycle.lock().await;
        self.closed.store(true, Ordering::Release);
        let mut sessions = lock(&self.sessions).clone();
        sessions.extend(lock(&self.retired).iter().cloned());
        for session in &sessions {
            let mut state = lock(&session.state);
            state.kill();
            state.end(None);
        }
        for session in &sessions {
            if let Err(error) = join(session).await {
                tracing::warn!(%error, "PTY shutdown join failed");
            }
        }
        lock(&self.sessions).clear();
        lock(&self.retired).clear();
        lock(&self.exits).clear();
    }
}

impl Drop for NativePty {
    fn drop(&mut self) {
        for session in lock(&self.sessions).iter() {
            let mut state = lock(&session.state);
            state.kill();
            state.end(None);
        }
    }
}

struct CreationGuard(Option<Arc<Mutex<State>>>);
impl Drop for CreationGuard {
    fn drop(&mut self) {
        if let Some(state) = &self.0 {
            let mut state = lock(state);
            state.kill();
            state.end(None);
        }
    }
}

async fn join(session: &Session) -> Result<(), PtyError> {
    let mut task = session.task.lock().await;
    if let Some(handle) = task.as_mut() {
        // Borrow the handle: cancelling remove/shutdown leaves it owned for a later join.
        let result = handle.await.map_err(|error| failure("join", error));
        task.take();
        result?;
    }
    Ok(())
}

fn write(state: &mut State, data: &str) -> Result<(), PtyError> {
    if state.info.status != PtyStatus::Running {
        return Ok(());
    }
    if data.len() > 64 * 1024 {
        return Err(failure("write", "PTY input exceeds 64 KiB per write"));
    }
    if let Some(writer) = &state.writer {
        writer
            .try_send(data.to_owned())
            .map_err(|error| failure("write", error))?;
    }
    Ok(())
}

fn read_output(mut reader: Box<dyn Read + Send>, state: Arc<Mutex<State>>) {
    let mut decoder = encoding_rs::UTF_8.new_decoder();
    let mut bytes = [0u8; 8192];
    loop {
        let count = match reader.read(&mut bytes) {
            Ok(count) => count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => 0,
        };
        let mut text = String::with_capacity(count * 3 + 4);
        let _ = decoder.decode_to_string(&bytes[..count], &mut text, count == 0);
        if !text.is_empty() {
            lock(&state).data(text);
        }
        if count == 0 {
            return;
        }
    }
}

pub(super) fn failure(operation: &str, error: impl std::fmt::Display) -> PtyError {
    PlatformError::new(PlatformErrorKind::Io, operation, "pty", error.to_string()).into()
}
