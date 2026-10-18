//! Purpose: minimal blocking SSE subscriber over a raw loopback TCP socket that timestamps each
//! `data:` JSON payload on arrival, for event latency/throughput and acceptance SSE expectations.
//! Scope: test/bench support only, HTTP/1.1 plain loopback, chunked or identity bodies.
//! Uses:
//! `-- none
//! External:
//! |-- anyhow::Result
//! `-- serde_json::Value
//! Used by:
//! rust/benchmarks/
//! |-- benches/process.rs
//! `-- src/lib.rs
//! rust/tests/
//! `-- acceptance/src/runner.rs
//! Invariants: reader thread is joined on drop after the socket is shut down; events keep arrival order.
//! Contract tests: benchmarks/tests/target.rs

use anyhow::Result;
use serde_json::Value;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub struct SseStream {
    sock: TcpStream,
    events: Arc<Mutex<Vec<(Instant, Value)>>>,
    reader: Option<JoinHandle<()>>,
}

impl SseStream {
    pub fn connect(port: u16, path: &str) -> Result<Self> {
        let mut sock = TcpStream::connect(("127.0.0.1", port))?;
        write!(
            sock,
            "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nAccept: text/event-stream\r\n\r\n"
        )?;
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut read = sock.try_clone()?;
        let sink = events.clone();
        let reader = std::thread::spawn(move || {
            let mut buf = Vec::new();
            let mut chunk = [0u8; 65536];
            while let Ok(n) = read.read(&mut chunk) {
                if n == 0 {
                    return;
                }
                let now = Instant::now();
                buf.extend_from_slice(&chunk[..n]);
                // Chunked framing lines never start with "data: ", so a line scan is sufficient.
                while let Some(pos) = buf.iter().position(|b| *b == b'\n') {
                    let line: Vec<u8> = buf.drain(..=pos).collect();
                    let line = String::from_utf8_lossy(&line);
                    if let Some(json) = line.trim_end().strip_prefix("data: ")
                        && let Ok(v) = serde_json::from_str::<Value>(json)
                    {
                        sink.lock().expect("sse sink").push((now, v));
                    }
                }
            }
        });
        Ok(Self {
            sock,
            events,
            reader: Some(reader),
        })
    }

    /// Wait for the first event (from index `from`) matching `pred`; returns arrival time and payload.
    pub fn wait_for(
        &self,
        from: usize,
        timeout: Duration,
        pred: impl Fn(&Value) -> bool,
    ) -> Option<(Instant, Value)> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if let Some(hit) = self.events.lock().expect("sse events")[from..]
                .iter()
                .find(|(_, v)| pred(v))
            {
                return Some(hit.clone());
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        None
    }

    pub fn len(&self) -> usize {
        self.events.lock().expect("sse events").len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn snapshot(&self) -> Vec<Value> {
        self.events
            .lock()
            .expect("sse events")
            .iter()
            .map(|(_, v)| v.clone())
            .collect()
    }
}

impl Drop for SseStream {
    fn drop(&mut self) {
        let _ = self.sock.shutdown(Shutdown::Both);
        if let Some(r) = self.reader.take() {
            let _ = r.join();
        }
    }
}

/// Event type, accepting both the per-directory shape and the `/global/event` `{payload:{...}}` shape.
pub fn event_type(v: &Value) -> Option<&str> {
    v.get("type")
        .or_else(|| v.get("payload").and_then(|p| p.get("type")))
        .and_then(Value::as_str)
}
