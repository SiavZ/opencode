//! Purpose: exercise desktop-host public local spawn/health/stop and remote connection
//! against LOCAL temp fixtures. Captures identity and process cleanup proof.
//! Scope: spike only. Uses tempdirs, bounded teardown, and dummy CLI fixtures.
//! Uses:
//! rust/crates/
//! `-- desktop-host/src/connections.rs
//! External:
//! |-- tokio
//! |-- tempfile
//! `-- opencode-desktop-host
//! Used by:
//! `-- none, spike only
//! Invariants: actual packaged CLI unavailable (honest blocker), no production data.
//! Contract tests: this file.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

use opencode_desktop_host::connections::{
    ChildServer, ChildSpec, ServerConnection, ServerReadyData, SidecarSlot, check_health,
};

/// Helper to get a free loopback port and URL
async fn get_free_port() -> (u16, String) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    (port, format!("http://127.0.0.1:{}", port))
}

#[tokio::test]
async fn test_local_spawn_health_stop_with_temp_fixture() {
    // BLOCKER: Actual packaged CLI (`opencode serve`) is unavailable in this spike environment.
    // We use `python3 -m http.server` as a dummy fixture to prove the spawn/health/stop mechanism.
    // A real integration test would use the actual packaged CLI.
    
    let temp_dir = tempfile::tempdir().unwrap();
    let (port, url) = get_free_port().await;
    
    // Create a simple Python HTTP server script that exits quickly if we want, 
    // but for health check we just need it to stay alive.
    // Actually, python3 -m http.server doesn't have /api/health.
    // Let's use a bash script that starts a simple Python server with /api/health,
    // or just use a bash script that sleeps and we mock the health check, 
    // OR we can just test the ChildServer with a simple `sleep` command and verify it stops.
    
    // Better: use a python script that serves /api/health
    let script_path = temp_dir.path().join("server.py");
    std::fs::write(&script_path, format!(r#"
import http.server
import socketserver

class HealthHandler(http.server.SimpleHTTPRequestHandler):
    def do_GET(self):
        if self.path in ['/api/health', '/global/health']:
            self.send_response(200)
            self.end_headers()
            self.wfile.write(b'OK')
        else:
            self.send_response(404)
            self.end_headers()

with socketserver.TCPServer(("", {port}), HealthHandler) as httpd:
    httpd.serve_forever()
"#, port = port)).unwrap();

    let spec = ChildSpec {
        program: PathBuf::from("python3"),
        args: vec![script_path.to_string_lossy().to_string()],
        env: BTreeMap::new(),
        cwd: None,
        stdin: None,
        url: url.clone(),
        auth: None,
        stop_grace: Duration::from_secs(2),
        on_line: None,
    };

    let child = ChildServer::spawn(spec).await.expect("Failed to spawn child");
    let pid = child.pid().expect("Child should have a PID");
    
    // Verify process is running
    let client = reqwest::Client::new();
    let cancel = CancellationToken::new();
    
    // Wait for healthy
    let health_result = child.wait_healthy(&client, Duration::from_secs(5), &cancel).await;
    assert!(health_result.is_ok(), "Child should become healthy");
    
    // Verify identity
    assert_eq!(child.url(), &url);
    assert!(child.program().contains("python3"));
    
    // Stop the child
    let exit_info = child.stop().await;
    assert!(exit_info.is_some(), "Should have exit info after stop");
    
    // Verify process cleanup (process should be gone)
    // On Unix, we can check if the process is still running
    #[cfg(unix)]
    {
        let is_running = unsafe { libc::kill(pid as i32, 0) == 0 };
        assert!(!is_running, "Process {} should be dead after stop", pid);
    }
}

#[tokio::test]
async fn test_sidecar_slot_kill_sidecar_only_stops_owned() {
    let slot = SidecarSlot::new();
    
    // Create a dummy owned sidecar
    let temp_dir = tempfile::tempdir().unwrap();
    let (port, url) = get_free_port().await;
    
    let script_path = temp_dir.path().join("server.py");
    std::fs::write(&script_path, format!(r#"
import http.server, socketserver
class H(http.server.SimpleHTTPRequestHandler):
    def do_GET(self):
        if self.path in ['/api/health', '/global/health']:
            self.send_response(200); self.end_headers(); self.wfile.write(b'OK')
        else:
            self.send_response(404); self.end_headers()
with socketserver.TCPServer(("", {port}), H) as httpd: httpd.serve_forever()
"#, port = port)).unwrap();

    let spec = ChildSpec {
        program: PathBuf::from("python3"),
        args: vec![script_path.to_string_lossy().to_string()],
        env: BTreeMap::new(),
        cwd: None,
        stdin: None,
        url: url.clone(),
        auth: None,
        stop_grace: Duration::from_secs(2),
        on_line: None,
    };

    let child = ChildServer::spawn(spec).await.unwrap();
    let pid = child.pid().unwrap();
    
    let ready = ServerReadyData {
        url: url.clone(),
        username: Some("opencode".to_string()),
        password: Some("testpass".to_string()),
    };
    
    let conn = ServerConnection::OwnedSidecar(std::sync::Arc::new(child), ready);
    slot.set(conn).await;
    
    // Verify it's in the slot
    let current = slot.current().await;
    assert!(current.is_some());
    assert!(current.unwrap().owned_by_host());
    
    // Kill sidecar
    slot.kill_sidecar().await;
    
    // Verify slot is cleared
    assert!(slot.current().await.is_none());
    
    // Verify process is dead
    #[cfg(unix)]
    {
        let is_running = unsafe { libc::kill(pid as i32, 0) == 0 };
        assert!(!is_running, "Owned sidecar process {} should be dead after kill_sidecar", pid);
    }
}

#[tokio::test]
async fn test_remote_connection_not_killed_by_slot() {
    let slot = SidecarSlot::new();
    
    // Start a temp server to act as "remote"
    let (url, handle) = p11_connections_spike::fixtures::spawn_temp_server().await;
    
    let ready = ServerReadyData {
        url: url.clone(),
        username: Some("opencode".to_string()),
        password: Some("remotepass".to_string()),
    };
    
    let conn = ServerConnection::Remote(ready);
    slot.set(conn).await;
    
    // Verify it's in the slot and NOT owned by host
    let current = slot.current().await.unwrap();
    assert!(!current.owned_by_host());
    assert_eq!(current.key(), url);
    
    // Kill sidecar should clear slot but NOT kill the remote server
    slot.kill_sidecar().await;
    
    assert!(slot.current().await.is_none());
    
    // Verify remote server is still running (health check should still pass)
    let client = reqwest::Client::new();
    let is_healthy = check_health(&client, &url, None).await;
    assert!(is_healthy, "Remote server should still be healthy after kill_sidecar");
    
    // Cleanup
    handle.abort();
}
