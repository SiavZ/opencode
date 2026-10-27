//! Purpose: executable model catalog refresh and restart acceptance.
//! Scope: disposable HOME and one loopback HTTP fixture.
//! Uses: opencode/tests/support/mod.rs
//! Used by: opencode/tests/admin/mod.rs
//! Invariants: refresh never contacts a public catalog or user credentials.
//! Contract tests: self

use super::super::support::{Home, assert_success, output};
use std::io::{Read, Write};
use std::net::TcpListener;

#[test]
fn forced_refresh_fetches_loopback_catalog_and_restart_reads_cache() {
    let home = Home::new();
    std::fs::write(
        home.0.path().join("config/opencode/opencode.json"),
        r#"{"provider":{"fixture":{"options":{"apiKey":"controlled-fixture-only"}}}}"#,
    )
    .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let mut request = [0; 4096];
        let count = socket.read(&mut request).unwrap();
        assert!(String::from_utf8_lossy(&request[..count]).starts_with("GET /api.json "));
        let body = r#"{"fixture":{"id":"fixture","name":"Fixture","env":[],"npm":"@ai-sdk/openai","models":{"catalog-model":{"id":"catalog-model","name":"Catalog model","limit":{"context":4096,"output":128}}}}}"#;
        write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
    });
    let cache_dir = home.0.path().join("cache/opencode");
    let mut command = home.command(&["models", "--refresh"]);
    command.env("OPENCODE_MODELS_URL", &url);
    let refreshed = output(command, None);
    assert_success(&refreshed);
    server.join().unwrap();
    assert!(String::from_utf8_lossy(&refreshed.stdout).contains("Models cache refreshed"));
    assert!(String::from_utf8_lossy(&refreshed.stdout).contains("fixture/catalog-model"));
    let cache = std::fs::read_dir(cache_dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("models-")
                && path
                    .extension()
                    .is_some_and(|extension| extension == "json")
        })
        .expect("URL-specific models cache");
    assert!(
        std::fs::read_to_string(cache)
            .unwrap()
            .contains("catalog-model")
    );
    let mut command = home.command(&["models", "fixture"]);
    command.env("OPENCODE_MODELS_URL", &url);
    let restarted = output(command, None);
    assert_success(&restarted);
    assert_eq!(
        String::from_utf8_lossy(&restarted.stdout),
        "fixture/catalog-model\n"
    );
}
