//! Purpose: drive a generated operation across real loopback TCP and decode its response.
//! Scope: controlled localhost HTTP fixture, no external network or user data.
//! Uses:
//! |-- rust/crates/client/src/generated/current/health/get.rs
//! `-- rust/crates/client/src/operation.rs
//! External:
//! `-- serde_json
//! Used by:
//! `-- cargo test -p opencode-client
//! Invariants: fixture adapter is test-only; it does not claim native HTTPS transport parity.
//! Contract tests: this file.
use opencode_client::{
    ClientError, ClientErrorReason, current,
    operation::{Response, Transport},
    transport::RequestDescriptor,
};
use std::io::{Read, Write};

struct Loopback(std::net::SocketAddr);
impl Transport for Loopback {
    fn execute(&self, request: RequestDescriptor) -> Result<Response, ClientError> {
        let failure = |error| ClientError::with_cause(ClientErrorReason::Transport, error);
        let mut socket = std::net::TcpStream::connect(self.0).map_err(failure)?;
        socket
            .set_read_timeout(Some(std::time::Duration::from_secs(3)))
            .map_err(failure)?;
        write!(
            socket,
            "{} {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
            request.method, request.path, self.0
        )
        .map_err(failure)?;
        let mut bytes = Vec::new();
        socket.read_to_end(&mut bytes).map_err(failure)?;
        let split = bytes
            .windows(4)
            .position(|value| value == b"\r\n\r\n")
            .expect("controlled HTTP fixture has headers");
        let head = std::str::from_utf8(&bytes[..split]).unwrap();
        Ok(Response {
            status: head.split_whitespace().nth(1).unwrap().parse().unwrap(),
            content_type: head
                .lines()
                .find_map(|line| line.strip_prefix("Content-Type: "))
                .map(str::to_owned),
            body: bytes[split + 4..].to_vec(),
        })
    }
}
#[test]
fn generated_health_execute_crosses_loopback_and_preserves_typed_result() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(std::time::Duration::from_secs(3)))
            .unwrap();
        let mut bytes = vec![];
        while !bytes.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            socket.read_exact(&mut byte).unwrap();
            bytes.push(byte[0]);
        }
        assert!(bytes.starts_with(b"GET /api/health HTTP/1.1\r\n"));
        let body = r#"{"healthy":true}"#;
        write!(socket,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
    });
    let input =
        serde_json::from_value::<current::health::get::Input>(serde_json::json!({})).unwrap();
    let result = current::health::get::execute(&Loopback(address), &input).unwrap();
    assert_eq!(
        serde_json::to_value(result).unwrap(),
        serde_json::json!({"healthy":true})
    );
    server.join().unwrap();
}
