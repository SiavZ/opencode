//! Purpose: provide a controlled tool-call HTTP/SSE boundary for actual PTY permission tests.
//! Scope: loopback fixture thread.
//! Uses:
//! `-- none
//! External:
//! `-- serde_json::{Value, json}
//! Used by:
//! rust/bins/
//! `-- opencode/tests/terminal_app/mod.rs
//! Invariants: no external traffic; fixture socket and worker shutdown are bounded and joined.
//! Contract tests: terminal_app permission tests

use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub struct ToolProvider {
    pub url: String,
    pub requests: mpsc::Receiver<Value>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl ToolProvider {
    pub fn start() -> Self {
        Self::with_tool("read", json!({"path":"evidence.txt"}))
    }

    pub fn with_tool(name: &str, arguments: Value) -> Self {
        let name = name.to_owned();
        let arguments = arguments.to_string();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let token = stop.clone();
        let (send, requests) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            let mut turn = 0;
            while !token.load(Ordering::Acquire) {
                let (mut socket, _) = match listener.accept() {
                    Ok(socket) => socket,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(error) => panic!("accept: {error}"),
                };
                socket
                    .set_read_timeout(Some(Duration::from_millis(100)))
                    .unwrap();
                socket
                    .set_write_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let Some(request) = read_request(&mut socket, &token) else {
                    continue;
                };
                send.send(request).unwrap();
                let tool = turn % 2 == 0;
                turn += 1;
                let delta = if tool {
                    json!({"tool_calls":[{"index":0,"id":format!("call_read_{turn}"),"type":"function","function":{"name":name,"arguments":arguments}}]})
                } else {
                    json!({"role":"assistant","content":"permission continuation"})
                };
                let frame = json!({"id":"chatcmpl-pty","created":0,"model":"local-test","choices":[{"index":0,"delta":delta,"finish_reason":null}]});
                let finish = json!({"id":"chatcmpl-pty","created":0,"model":"local-test","choices":[{"index":0,"delta":{},"finish_reason":if tool {"tool_calls"} else {"stop"}}],"usage":{"prompt_tokens":7,"completion_tokens":3,"total_tokens":10}});
                let _ = write!(
                    socket,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\ndata: {frame}\n\ndata: {finish}\n\ndata: [DONE]\n\n"
                );
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

fn read_request(socket: &mut TcpStream, stop: &AtomicBool) -> Option<Value> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut bytes = Vec::new();
    let mut chunk = [0; 4096];
    loop {
        if stop.load(Ordering::Acquire) {
            return None;
        }
        assert!(Instant::now() < deadline, "provider request timed out");
        match socket.read(&mut chunk) {
            Ok(0) => return None,
            Ok(count) => bytes.extend_from_slice(&chunk[..count]),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::Interrupted
                ) =>
            {
                continue;
            }
            Err(error) => panic!("read: {error}"),
        }
        let Some(end) = bytes.windows(4).position(|bytes| bytes == b"\r\n\r\n") else {
            continue;
        };
        let headers = String::from_utf8_lossy(&bytes[..end]);
        let length = headers
            .lines()
            .find_map(|line| {
                let (key, value) = line.split_once(':')?;
                key.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().unwrap())
            })
            .unwrap();
        if bytes.len() >= end + 4 + length {
            return Some(serde_json::from_slice(&bytes[end + 4..end + 4 + length]).unwrap());
        }
    }
}

impl Drop for ToolProvider {
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
