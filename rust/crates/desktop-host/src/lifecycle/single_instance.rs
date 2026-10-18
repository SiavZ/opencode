//! Purpose: single-instance arbitration replacing Electron `requestSingleInstanceLock` and the
//! `second-instance` event (`main/index.ts:198-216`). The first process becomes primary: it holds
//! a lock file and listens on a local socket (unix domain socket, or a named pipe on Windows).
//! Later processes connect, forward their argv and working directory as one JSON line, wait for
//! the acknowledgement, and are told to exit.
//! Scope: arbitration and argv forwarding only. Deep link extraction and window focus are done
//! by the consumer of `SecondInstance` (deeplinks.rs / shell).
//! Uses:
//! `-- none
//! External:
//! |-- serde::{Deserialize, Serialize}
//! |-- serde_json
//! |-- thiserror::Error
//! |-- tokio::net::{UnixListener, UnixStream}
//! |-- tokio::net::windows::named_pipe
//! |-- tokio::sync::mpsc
//! `-- tokio_util::sync::CancellationToken
//! Used by:
//! rust/crates/
//! |-- desktop-host/src/lifecycle.rs
//! `-- desktop-host/tests/lifecycle_single_instance.rs
//! Invariants: at most one primary per instance directory; a lock without a live listener is
//! stale and is reclaimed after `STALE_RETRIES` failed connects; forwarded messages are bounded
//! to `MAX_MESSAGE_BYTES`; the listener task is owned, cancellable and joined by `shutdown`, which
//! removes the lock and socket; delivery to the primary uses a bounded channel and a full channel
//! makes the primary reply `busy` so the secondary reports `Rejected` instead of hanging.
//! Contract tests: desktop-host/tests/lifecycle_single_instance.rs

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

pub const LOCK_FILE: &str = "instance.lock";
pub const SOCKET_FILE: &str = "instance.sock";
pub const MAX_MESSAGE_BYTES: u64 = 64 * 1024;
pub const SECOND_INSTANCE_QUEUE: usize = 16;
const STALE_RETRIES: u32 = 5;
const RETRY_DELAY: Duration = Duration::from_millis(40);
const IO_TIMEOUT: Duration = Duration::from_secs(5);

/// Payload of Electron's `second-instance` event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecondInstance {
    pub argv: Vec<String>,
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Ack {
    Ok,
    Busy,
}

#[derive(Debug, thiserror::Error)]
pub enum SingleInstanceError {
    #[error("single instance io error: {0}")]
    Io(#[from] io::Error),
    #[error("primary instance rejected the forwarded arguments")]
    Rejected,
    #[error("could not become primary or reach the primary after retries")]
    Contended,
    #[error("forwarded message exceeds {MAX_MESSAGE_BYTES} bytes")]
    TooLarge,
    #[error("malformed single instance message: {0}")]
    Malformed(#[from] serde_json::Error),
    #[error("timed out talking to the primary instance")]
    TimedOut,
}

pub enum Arbitration {
    Primary(PrimaryInstance),
    /// Arguments were delivered to the running primary. The caller must exit (baseline
    /// `app.quit()` at `index.ts:198-201`).
    Secondary,
}

impl std::fmt::Debug for Arbitration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Primary(_) => f.write_str("Primary"),
            Self::Secondary => f.write_str("Secondary"),
        }
    }
}

/// The running primary. Receive forwarded launches from `second_instances`.
pub struct PrimaryInstance {
    dir: PathBuf,
    cancel: CancellationToken,
    task: Option<JoinHandle<()>>,
    rx: mpsc::Receiver<SecondInstance>,
}

impl PrimaryInstance {
    pub async fn recv(&mut self) -> Option<SecondInstance> {
        self.rx.recv().await
    }

    pub fn try_recv(&mut self) -> Option<SecondInstance> {
        self.rx.try_recv().ok()
    }

    /// Stops listening, joins the listener task and removes the lock and socket.
    pub async fn shutdown(mut self) {
        self.cancel.cancel();
        if let Some(task) = self.task.take()
            && let Err(error) = task.await
        {
            tracing::warn!(%error, "single instance listener task failed");
        }
        remove_files(&self.dir);
    }
}

impl Drop for PrimaryInstance {
    fn drop(&mut self) {
        if self.task.is_some() {
            // Not joined: abort and clean up synchronously so a crash-free drop still frees the lock.
            self.cancel.cancel();
            if let Some(task) = self.task.take() {
                task.abort();
            }
            remove_files(&self.dir);
        }
    }
}

fn remove_files(dir: &Path) {
    for name in [SOCKET_FILE, LOCK_FILE] {
        match std::fs::remove_file(dir.join(name)) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => tracing::warn!(%error, name, "failed to remove single instance file"),
        }
    }
}

/// Arbitrates within `dir` (the app's user data directory). `pipe_name` scopes the Windows named
/// pipe and is ignored on unix where the socket lives in `dir`.
pub async fn acquire(
    dir: &Path,
    pipe_name: &str,
    launch: SecondInstance,
) -> Result<Arbitration, SingleInstanceError> {
    std::fs::create_dir_all(dir)?;
    let endpoint = transport::Endpoint::new(dir, pipe_name);
    let payload = encode(&launch)?;
    for attempt in 0..=STALE_RETRIES {
        if let Some(stream) = endpoint.connect().await? {
            forward(stream, &payload).await?;
            return Ok(Arbitration::Secondary);
        }
        match try_lock(dir)? {
            true => {
                let listener = endpoint.bind()?;
                return Ok(Arbitration::Primary(spawn_primary(
                    dir.to_path_buf(),
                    listener,
                )));
            }
            // A lock exists but nobody answers. The primary may be between locking and binding,
            // so retry briefly before treating the lock as stale.
            false if attempt < STALE_RETRIES => tokio::time::sleep(RETRY_DELAY).await,
            false => {
                tracing::warn!(dir = %dir.display(), "reclaiming stale single instance lock");
                remove_files(dir);
                if try_lock(dir)? {
                    let listener = endpoint.bind()?;
                    return Ok(Arbitration::Primary(spawn_primary(
                        dir.to_path_buf(),
                        listener,
                    )));
                }
            }
        }
    }
    Err(SingleInstanceError::Contended)
}

fn encode(launch: &SecondInstance) -> Result<Vec<u8>, SingleInstanceError> {
    let mut payload = serde_json::to_vec(launch)?;
    payload.push(b'\n');
    if payload.len() as u64 > MAX_MESSAGE_BYTES {
        return Err(SingleInstanceError::TooLarge);
    }
    Ok(payload)
}

fn try_lock(dir: &Path) -> io::Result<bool> {
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dir.join(LOCK_FILE))
    {
        Ok(mut file) => {
            use std::io::Write;
            writeln!(file, "{}", std::process::id())?;
            Ok(true)
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(false),
        Err(error) => Err(error),
    }
}

async fn forward<S: AsyncRead + AsyncWrite + Unpin>(
    mut stream: S,
    payload: &[u8],
) -> Result<(), SingleInstanceError> {
    let exchange = async {
        stream.write_all(payload).await?;
        stream.flush().await?;
        let line = read_line(&mut stream).await?;
        let ack: Ack = serde_json::from_str(line.trim_end())?;
        match ack {
            Ack::Ok => Ok(()),
            Ack::Busy => Err(SingleInstanceError::Rejected),
        }
    };
    tokio::time::timeout(IO_TIMEOUT, exchange)
        .await
        .map_err(|_| SingleInstanceError::TimedOut)?
}

async fn read_line<S: AsyncRead + Unpin>(stream: S) -> Result<String, SingleInstanceError> {
    let mut reader = BufReader::new(stream.take(MAX_MESSAGE_BYTES + 1));
    let mut line = String::new();
    reader.read_line(&mut line).await?;
    if line.len() as u64 > MAX_MESSAGE_BYTES {
        return Err(SingleInstanceError::TooLarge);
    }
    if !line.ends_with('\n') {
        return Err(SingleInstanceError::Io(io::ErrorKind::UnexpectedEof.into()));
    }
    Ok(line)
}

fn spawn_primary(dir: PathBuf, listener: transport::Listener) -> PrimaryInstance {
    let cancel = CancellationToken::new();
    let (tx, rx) = mpsc::channel(SECOND_INSTANCE_QUEUE);
    let token = cancel.clone();
    let task = tokio::spawn(async move {
        let mut listener = listener;
        loop {
            let stream = tokio::select! {
                () = token.cancelled() => return,
                accepted = listener.accept() => accepted,
            };
            match stream {
                // Handled inline: messages are tiny and bounded by IO_TIMEOUT, which keeps every
                // connection owned by this joined task.
                Ok(stream) => {
                    if let Err(error) = handle(stream, &tx).await {
                        tracing::warn!(%error, "rejected second instance message");
                    }
                }
                Err(error) => {
                    tracing::warn!(%error, "single instance accept failed");
                    tokio::time::sleep(RETRY_DELAY).await;
                }
            }
        }
    });
    PrimaryInstance {
        dir,
        cancel,
        task: Some(task),
        rx,
    }
}

async fn handle<S: AsyncRead + AsyncWrite + Unpin>(
    mut stream: S,
    tx: &mpsc::Sender<SecondInstance>,
) -> Result<(), SingleInstanceError> {
    let exchange = async {
        let line = read_line(&mut stream).await?;
        let launch: SecondInstance = serde_json::from_str(line.trim_end())?;
        let ack = match tx.try_send(launch) {
            Ok(()) => Ack::Ok,
            Err(_) => Ack::Busy,
        };
        let mut reply = serde_json::to_vec(&ack)?;
        reply.push(b'\n');
        stream.write_all(&reply).await?;
        stream.flush().await?;
        Ok(())
    };
    tokio::time::timeout(IO_TIMEOUT, exchange)
        .await
        .map_err(|_| SingleInstanceError::TimedOut)?
}

#[cfg(unix)]
mod transport {
    use std::io;
    use std::path::{Path, PathBuf};

    use tokio::net::{UnixListener, UnixStream};

    pub struct Endpoint {
        path: PathBuf,
    }

    pub struct Listener(UnixListener);

    impl Endpoint {
        pub fn new(dir: &Path, _pipe_name: &str) -> Self {
            Self {
                path: dir.join(super::SOCKET_FILE),
            }
        }

        /// `Ok(None)` when nothing is listening (missing or stale socket).
        pub async fn connect(&self) -> io::Result<Option<UnixStream>> {
            match UnixStream::connect(&self.path).await {
                Ok(stream) => Ok(Some(stream)),
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
                    ) =>
                {
                    Ok(None)
                }
                Err(error) => Err(error),
            }
        }

        /// Called only while holding the lock, so a leftover socket file is stale.
        pub fn bind(&self) -> io::Result<Listener> {
            match std::fs::remove_file(&self.path) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
            UnixListener::bind(&self.path).map(Listener)
        }
    }

    impl Listener {
        pub async fn accept(&mut self) -> io::Result<UnixStream> {
            self.0.accept().await.map(|(stream, _)| stream)
        }
    }
}

#[cfg(windows)]
mod transport {
    use std::io;
    use std::path::Path;

    use tokio::net::windows::named_pipe::{
        ClientOptions, NamedPipeClient, NamedPipeServer, ServerOptions,
    };

    /// Windows: `ERROR_PIPE_BUSY`.
    const ERROR_PIPE_BUSY: i32 = 231;

    pub struct Endpoint {
        name: String,
    }

    pub struct Listener {
        name: String,
        next: NamedPipeServer,
    }

    impl Endpoint {
        pub fn new(_dir: &Path, pipe_name: &str) -> Self {
            let sanitized: String = pipe_name
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                        c
                    } else {
                        '_'
                    }
                })
                .collect();
            Self {
                name: format!(r"\\.\pipe\{sanitized}-single-instance"),
            }
        }

        pub async fn connect(&self) -> io::Result<Option<NamedPipeClient>> {
            for _ in 0..20 {
                match ClientOptions::new().open(&self.name) {
                    Ok(client) => return Ok(Some(client)),
                    Err(error) if error.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
                        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                    }
                    Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
                    Err(error) => return Err(error),
                }
            }
            Err(io::ErrorKind::TimedOut.into())
        }

        pub fn bind(&self) -> io::Result<Listener> {
            let next = ServerOptions::new()
                .first_pipe_instance(true)
                .create(&self.name)?;
            Ok(Listener {
                name: self.name.clone(),
                next,
            })
        }
    }

    impl Listener {
        pub async fn accept(&mut self) -> io::Result<NamedPipeServer> {
            self.next.connect().await?;
            let fresh = ServerOptions::new().create(&self.name)?;
            Ok(std::mem::replace(&mut self.next, fresh))
        }
    }
}
