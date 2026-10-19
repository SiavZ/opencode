//! Purpose: controlled loopback OpenAI-compatible provider for production composition tests.
//! Scope: real TCP HTTP/SSE boundary, never an injected runner or live provider.
//! Uses:
//! `-- none
//! External:
//! |-- serde_json::{Value, json}
//! |-- tokio::{io, net, sync, task}
//! `-- tokio_util::sync::CancellationToken
//! Used by:
//! rust/crates/
//! `-- runtime/tests/production.rs
//! Invariants: listens only on loopback; every server task is cancelled and joined; captured
//! requests pass through the real provider transport, model resolver, runner and projection.
//! Contract tests: runtime/tests/production.rs

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

pub enum Reply {
    Text(&'static str),
    Hold,
    Failure,
    Tool {
        name: &'static str,
        arguments: Value,
    },
}

pub struct Provider {
    pub url: String,
    pub requests: mpsc::Receiver<(String, Value)>,
    cancel: CancellationToken,
    task: JoinHandle<()>,
}

impl Provider {
    pub async fn start(replies: Vec<Reply>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let (sender, requests) = mpsc::channel(8);
        let cancel = CancellationToken::new();
        let token = cancel.clone();
        let task = tokio::spawn(async move {
            for reply in replies {
                let (mut socket, _) = tokio::select! {
                    _ = token.cancelled() => return,
                    result = listener.accept() => result.unwrap(),
                };
                let mut buffer = Vec::new();
                let mut chunk = [0; 4096];
                let (header, body) = loop {
                    let size = tokio::select! {
                        _ = token.cancelled() => return,
                        result = socket.read(&mut chunk) => result.unwrap(),
                    };
                    assert_ne!(size, 0, "provider client closed before sending HTTP body");
                    buffer.extend_from_slice(&chunk[..size]);
                    let Some(end) = buffer.windows(4).position(|bytes| bytes == b"\r\n\r\n") else {
                        continue;
                    };
                    let header = String::from_utf8(buffer[..end].to_vec()).unwrap();
                    let length = header
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().unwrap())
                        })
                        .expect("provider request must have Content-Length");
                    if buffer.len() >= end + 4 + length {
                        break (
                            header,
                            serde_json::from_slice::<Value>(&buffer[end + 4..end + 4 + length])
                                .unwrap(),
                        );
                    }
                };
                sender.send((header, body)).await.unwrap();
                if matches!(reply, Reply::Failure) {
                    let body = r#"{"error":{"message":"controlled rejection"}}"#;
                    socket.write_all(format!("HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
                    continue;
                }
                socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n").await.unwrap();
                let delta = match &reply {
                    Reply::Tool { name, arguments } => {
                        json!({"tool_calls":[{"index":0,"id":"call_read","type":"function","function":{"name":name,"arguments":arguments.to_string()}}]})
                    }
                    Reply::Text(text) => json!({"role":"assistant","content":text}),
                    Reply::Hold => json!({"role":"assistant","content":"partial"}),
                    Reply::Failure => unreachable!(),
                };
                let frame = json!({"id":"chatcmpl-local","created":0,"model":"local-test","choices":[{"index":0,"delta":delta,"finish_reason":null}]});
                socket
                    .write_all(format!("data: {frame}\n\n").as_bytes())
                    .await
                    .unwrap();
                if matches!(reply, Reply::Hold) {
                    tokio::select! {
                        _ = token.cancelled() => return,
                        result = socket.read(&mut chunk) => { let _ = result; },
                    }
                    continue;
                }
                let finish = if matches!(reply, Reply::Tool { .. }) {
                    "tool_calls"
                } else {
                    "stop"
                };
                let frame = json!({"id":"chatcmpl-local","created":0,"model":"local-test","choices":[{"index":0,"delta":{},"finish_reason":finish}],"usage":{"prompt_tokens":7,"completion_tokens":3,"total_tokens":10}});
                socket
                    .write_all(format!("data: {frame}\n\ndata: [DONE]\n\n").as_bytes())
                    .await
                    .unwrap();
            }
        });
        Self {
            url,
            requests,
            cancel,
            task,
        }
    }

    pub async fn stop(self) {
        self.cancel.cancel();
        self.task.await.unwrap();
    }
}
