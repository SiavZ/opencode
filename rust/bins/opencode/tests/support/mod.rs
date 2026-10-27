//! Purpose: isolated binary commands and controlled loopback provider for CLI regression tests.
//! Scope: subprocess and TCP fixture, never live credentials or provider traffic.
//! Uses:
//! `-- none
//! External:
//! |-- mpsc::Receiver
//! |-- mpsc::channel
//! |-- serde_json::Value
//! |-- serde_json::from_slice
//! |-- serde_json::json
//! |-- tempfile::TempDir
//! `-- tempfile::tempdir
//! Used by:
//! rust/bins/
//! `-- opencode/tests/commands.rs
//! Invariants: every child is reaped, server joined, all application state lives in temp roots.
//! Contract tests: opencode/tests/commands.rs

use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::{Child, Command, Output, Stdio};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

pub struct Home(pub tempfile::TempDir);

/// Test-only artifact selection, resolved before children change cwd or clear their environment.
/// Invalid overrides fail rather than silently falling back to the Cargo-built executable.
pub fn binary() -> std::path::PathBuf {
    std::env::var_os("OPENCODE_ACCEPTANCE_BIN")
        .map(|path| {
            std::path::PathBuf::from(path)
                .canonicalize()
                .expect("OPENCODE_ACCEPTANCE_BIN must identify an existing executable")
        })
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_opencode").into())
}

impl Home {
    pub fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        for path in [
            "home",
            "config/opencode",
            "data",
            "cache",
            "state",
            "project",
            "tmp",
        ] {
            std::fs::create_dir_all(root.path().join(path)).unwrap();
        }
        Self(root)
    }

    pub fn config(&self, url: &str) {
        std::fs::write(self.0.path().join("config/opencode/opencode.json"), json!({
            "model":"local/local-test",
            "provider": {"local": {
                "npm":"@ai-sdk/openai-compatible",
                "name":"Controlled local provider",
                "options":{"baseURL":url,"apiKey":"local-test-only"},
                "models":{"local-test":{"name":"Local test", "limit":{"context":32768,"output":1024}}}
            }}
        }).to_string()).unwrap();
    }

    pub fn config_v2(&self, url: &str) {
        std::fs::write(
            self.0.path().join("config/opencode/opencode.json"),
            json!({
                "model":"local/local-test",
                "providers":{"local":{
                    "name":"Controlled loopback",
                    "api":{"type":"aisdk","package":"@ai-sdk/openai-compatible","url":url},
                    "request":{"body":{"apiKey":"local-test-only"}},
                    "models":{"local-test":{
                        "name":"Local test",
                        "capabilities":{"tools":true,"input":["text"],"output":["text"]},
                        "limit":{"context":32768,"output":1024}
                    }}
                }}
            })
            .to_string(),
        )
        .unwrap();
    }

    pub fn env(&self) -> Vec<(&'static str, std::path::PathBuf)> {
        [
            ("HOME", "home"),
            ("XDG_CONFIG_HOME", "config"),
            ("XDG_DATA_HOME", "data"),
            ("XDG_CACHE_HOME", "cache"),
            ("XDG_STATE_HOME", "state"),
            ("TMPDIR", "tmp"),
        ]
        .into_iter()
        .map(|(key, path)| (key, self.0.path().join(path)))
        .collect()
    }

    pub fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(binary());
        command
            .args(args)
            .current_dir(self.0.path().join("project"))
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("OPENCODE_DISABLE_MODELS_FETCH", "true")
            .envs(self.env())
            .stdin(Stdio::null());
        command
    }
}

pub struct ChildGuard(pub Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub fn output(mut command: Command, input: Option<&str>) -> Output {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    if input.is_some() {
        command.stdin(Stdio::piped());
    }
    let mut child = ChildGuard(command.spawn().unwrap());
    if let Some(input) = input {
        child
            .0
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
    }
    let deadline = Instant::now() + Duration::from_secs(20);
    while child.0.try_wait().unwrap().is_none() {
        assert!(Instant::now() < deadline, "binary command timed out");
        std::thread::sleep(Duration::from_millis(20));
    }
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    child
        .0
        .stdout
        .take()
        .unwrap()
        .read_to_end(&mut stdout)
        .unwrap();
    child
        .0
        .stderr
        .take()
        .unwrap()
        .read_to_end(&mut stderr)
        .unwrap();
    Output {
        status: child.0.wait().unwrap(),
        stdout,
        stderr,
    }
}

pub struct Provider {
    pub url: String,
    pub requests: mpsc::Receiver<(String, Value)>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Provider {
    pub fn start(failure: bool) -> Self {
        Self::delayed(failure, Duration::from_millis(60))
    }

    pub fn delayed(failure: bool, delay: Duration) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let token = stop.clone();
        let (send, requests) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            'requests: while !token.load(Ordering::Acquire) {
                let (mut socket, _) = match listener.accept() {
                    Ok(socket) => socket,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(error) => panic!("accept: {error}"),
                };
                socket.set_nonblocking(false).unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_millis(100)))
                    .unwrap();
                socket
                    .set_write_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let deadline = std::time::Instant::now() + Duration::from_secs(10);
                let mut buffer = Vec::new();
                let mut chunk = [0; 4096];
                let (headers, body) = loop {
                    if token.load(Ordering::Acquire) {
                        return;
                    }
                    let count = match socket.read(&mut chunk) {
                        Ok(0) if buffer.is_empty() => continue 'requests,
                        Ok(0) => panic!("provider received a truncated HTTP request"),
                        Ok(count) => count,
                        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(error)
                            if matches!(
                                error.kind(),
                                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                            ) =>
                        {
                            assert!(
                                std::time::Instant::now() < deadline,
                                "provider request read deadline: {error}"
                            );
                            continue;
                        }
                        Err(error) => panic!("provider request read: {error}"),
                    };
                    buffer.extend_from_slice(&chunk[..count]);
                    let Some(end) = buffer.windows(4).position(|bytes| bytes == b"\r\n\r\n") else {
                        continue;
                    };
                    let header = String::from_utf8(buffer[..end].to_vec()).unwrap();
                    let length = header
                        .lines()
                        .find_map(|line| {
                            let (key, value) = line.split_once(':')?;
                            key.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().unwrap())
                        })
                        .unwrap();
                    if buffer.len() >= end + 4 + length {
                        break (
                            header,
                            serde_json::from_slice::<Value>(&buffer[end + 4..end + 4 + length])
                                .unwrap(),
                        );
                    }
                };
                send.send((headers, body)).unwrap();
                if failure {
                    let body = r#"{"error":{"message":"controlled provider failure"}}"#;
                    let _ = write!(
                        socket,
                        "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    continue;
                }
                let _ = socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n");
                for text in ["controlled ", "assistant reply"] {
                    let frame = json!({"id":"chatcmpl-cli","created":0,"model":"local-test","choices":[{"index":0,"delta":{"role":"assistant","content":text},"finish_reason":null}]});
                    let _ = write!(socket, "data: {frame}\n\n");
                    let _ = socket.flush();
                    let deadline = std::time::Instant::now() + delay;
                    while std::time::Instant::now() < deadline {
                        if token.load(Ordering::Acquire) {
                            return;
                        }
                        std::thread::sleep(Duration::from_millis(5));
                    }
                }
                let frame = json!({"id":"chatcmpl-cli","created":0,"model":"local-test","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":7,"completion_tokens":3,"total_tokens":10}});
                let _ = write!(socket, "data: {frame}\n\ndata: [DONE]\n\n");
            }
        });
        Self {
            url,
            requests,
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for Provider {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let result = thread.join();
            if !std::thread::panicking() {
                result.unwrap();
            }
        }
    }
}

pub fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "exit {:?}, stderr {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
}
