//! Purpose: contract tests for the process-driving support library: command parsing, HOME/XDG
//! isolation and OPENCODE_* scrubbing observed from a real child process, percentile math, and
//! SSE parsing against a local loopback SSE server.
//! Scope: fixture tests (no opencode binary needed).
//! Uses:
//! rust/benchmarks/
//! |-- src/lib.rs
//! `-- src/sse.rs
//! External:
//! `-- none
//! Used by:
//! `-- none, entrypoint
//! Invariants: tests never read or write the real user HOME.
//! Contract tests: this file

use opencode_benchmarks::{Home, Target, percentile, sse};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::Duration;

#[test]
fn parse_rejects_blank_and_splits_quotes() {
    assert!(Target::parse("   ", None).is_none());
    let t = Target::parse("bun run 'src/index.ts'", None).expect("target");
    assert_eq!(t.argv, ["bun", "run", "src/index.ts"]);
}

#[test]
fn child_sees_isolated_home() {
    let t = Target::parse("/usr/bin/env", None).expect("target");
    let home = Home::new().expect("home");
    let (out, _) = t.run(&home, &[]).expect("run env");
    let env = String::from_utf8(out.stdout).expect("utf8");
    let root = home.path().display().to_string();
    assert!(env.lines().any(|l| l == format!("HOME={root}")), "{env}");
    assert!(
        env.lines()
            .any(|l| l == format!("XDG_DATA_HOME={root}/data"))
    );
    assert!(
        env.lines()
            .any(|l| l == format!("XDG_STATE_HOME={root}/state"))
    );
    assert!(!env.lines().any(|l| l.starts_with("OPENCODE_")), "{env}");
}

#[test]
fn percentile_interpolates() {
    assert_eq!(percentile(&[], 50.0), None);
    assert_eq!(percentile(&[3.0, 1.0, 2.0], 50.0), Some(2.0));
    assert_eq!(percentile(&[0.0, 10.0], 95.0), Some(9.5));
}

#[test]
fn sse_reads_chunked_data_lines_in_order() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let server = std::thread::spawn(move || {
        let (mut s, _) = listener.accept().expect("accept");
        let mut req = [0u8; 1024];
        let _ = s.read(&mut req).expect("read req");
        let body = "data: {\"type\":\"server.connected\",\"properties\":{}}\n\n\
                    data: {\"payload\":{\"type\":\"session.created\"}}\n\n";
        write!(
            s,
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n{:x}\r\n{body}\r\n",
            body.len()
        )
        .expect("write");
        std::thread::sleep(Duration::from_millis(200));
    });
    let stream = sse::SseStream::connect(port, "/event").expect("connect");
    let hit = stream.wait_for(0, Duration::from_secs(5), |v| {
        sse::event_type(v) == Some("session.created")
    });
    assert!(hit.is_some());
    let types: Vec<_> = stream
        .snapshot()
        .iter()
        .map(|v| sse::event_type(v).map(str::to_owned))
        .collect();
    assert_eq!(
        types,
        [
            Some("server.connected".into()),
            Some("session.created".into())
        ]
    );
    server.join().expect("server");
}
