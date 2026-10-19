//! Purpose: controlled HTTP fixture for the synchronous legacy remote configuration transport.
//! Scope: integration-test module included by runtime/tests/global_services.rs.
//! Uses:
//! rust/crates/
//! `-- runtime/src/global_services/config.rs
//! External:
//! |-- serde_json::Value
//! `-- tempfile::tempdir
//! Used by:
//! rust/crates/
//! `-- runtime/tests/global_services.rs
//! Invariants: only loopback sockets and isolated environments are used.
//! Contract tests: runtime/tests/global_services.rs

use serde_json::Value;

#[tokio::test]
async fn remote_config_transport_reads_only_controlled_fixture() {
    use opencode_config::legacy::RemoteFetcher;
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/config", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let mut request = [0; 2048];
        let read = socket.read(&mut request).unwrap();
        let request = String::from_utf8_lossy(&request[..read]);
        assert!(request.contains("accept: application/json"));
        assert!(request.contains("x-fixture: controlled"));
        let body = "{\"config\":{\"theme\":\"fixture\"}}";
        write!(socket,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();
    });
    let root = tempfile::tempdir().unwrap();
    let fetcher = opencode_runtime::global_services::RemoteConfiguration::new(
        opencode_config::Environment::isolated(root.path()),
    );
    let response = tokio::task::spawn_blocking(move || {
        fetcher.get(
            &url,
            &std::collections::BTreeMap::from([("x-fixture".into(), "controlled".into())]),
        )
    })
    .await
    .unwrap()
    .unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(
        serde_json::from_str::<Value>(&response.body).unwrap()["config"]["theme"],
        "fixture"
    );
    server.join().unwrap();
}
