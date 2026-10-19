//! Purpose: real CLI command behavior through the built executable and a loopback provider.
//! Scope: isolated binary integration, including a real PTY on Unix.
//! Uses:
//! rust/bins/
//! `-- opencode/tests/support/mod.rs
//! External:
//! |-- mpsc::Receiver
//! |-- mpsc::channel
//! |-- portable_pty::Child
//! |-- portable_pty::CommandBuilder
//! |-- portable_pty::MasterPty
//! |-- portable_pty::PtySize
//! |-- portable_pty::native_pty_system
//! |-- vt100::Parser
//! `-- vt100::Parser::new
//! Used by:
//! `-- none, entrypoint
//! Invariants: no live keys or paid calls; sessions, config and terminal belong to each test.
//! Contract tests: self

mod support;

use std::time::Duration;

use support::{Home, Provider, assert_success, output};

#[test]
fn run_uses_production_provider_and_combines_stdin() {
    let home = Home::new();
    let provider = Provider::start(false);
    home.config(&provider.url);
    drop(
        std::net::TcpStream::connect(
            provider
                .url
                .trim_start_matches("http://")
                .trim_end_matches("/v1"),
        )
        .unwrap(),
    );
    let result = output(
        home.command(&["run", "hello", "--model", "local/local-test"]),
        Some("from stdin"),
    );
    assert_success(&result);
    assert_eq!(
        String::from_utf8(result.stdout).unwrap(),
        "controlled assistant reply\n"
    );
    let (headers, body) = provider
        .requests
        .recv_timeout(Duration::from_secs(2))
        .unwrap();
    assert!(
        headers.starts_with("POST /v1/chat/completions "),
        "{headers}"
    );
    assert!(
        headers
            .to_ascii_lowercase()
            .contains("authorization: bearer local-test-only"),
        "{headers}"
    );
    assert_eq!(body["model"], "local-test");
    assert!(
        body["messages"].to_string().contains("hello\\nfrom stdin"),
        "{body}"
    );
    assert!(
        provider.requests.try_recv().is_err(),
        "one CLI prompt must not make duplicate turns"
    );
}

#[test]
fn run_and_models_use_canonical_v2_configuration() {
    let home = Home::new();
    let provider = Provider::start(false);
    home.config_v2(&provider.url);
    let models = output(home.command(&["models", "local"]), None);
    assert_success(&models);
    assert_eq!(
        String::from_utf8(models.stdout).unwrap(),
        "local/local-test\n"
    );
    let result = output(home.command(&["run", "V2 prompt", "--", "--port"]), None);
    assert_success(&result);
    assert_eq!(
        String::from_utf8(result.stdout).unwrap(),
        "controlled assistant reply\n"
    );
    let (headers, body) = provider
        .requests
        .recv_timeout(Duration::from_secs(2))
        .unwrap();
    assert!(headers.starts_with("POST /v1/chat/completions "));
    assert!(
        headers
            .to_ascii_lowercase()
            .contains("authorization: bearer local-test-only"),
        "{headers}"
    );
    assert!(body["messages"].to_string().contains("V2 prompt --port"));
}

#[test]
fn run_continue_reuses_durable_session_without_reprinting_old_response() {
    let home = Home::new();
    let provider = Provider::start(false);
    home.config(&provider.url);
    assert_success(&output(home.command(&["run", "first"]), None));
    let result = output(home.command(&["run", "second", "--continue"]), None);
    assert_success(&result);
    assert_eq!(
        String::from_utf8(result.stdout).unwrap(),
        "controlled assistant reply\n"
    );
    let _ = provider
        .requests
        .recv_timeout(Duration::from_secs(2))
        .unwrap();
    let (_, second) = provider
        .requests
        .recv_timeout(Duration::from_secs(2))
        .unwrap();
    let messages = second["messages"].to_string();
    assert!(messages.contains("first"), "{messages}");
    assert!(messages.contains("second"), "{messages}");
    assert!(
        messages.contains("controlled assistant reply"),
        "{messages}"
    );
}

#[test]
fn provider_failure_is_a_failed_command_not_admission_success() {
    let home = Home::new();
    let provider = Provider::start(true);
    home.config(&provider.url);
    let result = output(home.command(&["run", "fail"]), None);
    assert_eq!(result.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("controlled provider failure"),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(result.stdout.is_empty());
}

#[test]
fn models_lists_real_configured_catalog_and_filters_unknown_provider() {
    let home = Home::new();
    let provider = Provider::start(false);
    home.config(&provider.url);
    let result = output(home.command(&["models", "local"]), None);
    assert_success(&result);
    assert_eq!(
        String::from_utf8(result.stdout).unwrap(),
        "local/local-test\n"
    );
    let result = output(home.command(&["models", "local", "--verbose"]), None);
    assert_success(&result);
    assert!(String::from_utf8_lossy(&result.stdout).contains("\"providerID\": \"local\""));
    let result = output(home.command(&["models", "missing-provider"]), None);
    assert_eq!(result.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("Provider not found: missing-provider")
    );
    assert!(
        provider.requests.try_recv().is_err(),
        "catalog listing must not execute a provider call"
    );
}

#[test]
fn input_errors_and_unimplemented_features_are_honest_failures() {
    let home = Home::new();
    for (args, code, diagnostic) in [
        (vec![], 1, "TUI requires a TTY"),
        (vec!["run"], 1, "You must provide a message"),
        (
            vec!["run", "hello", "--model", "invalid"],
            1,
            "provider/model",
        ),
        (
            vec!["run", "hello", "--attach", "http://127.0.0.1:1"],
            2,
            "NotYetPorted: --attach",
        ),
        (vec!["models", "--refresh"], 2, "NotYetPorted: --refresh"),
        (
            vec!["run", "hello", "--format", "json"],
            2,
            "NotYetPorted: legacy JSON event formatting",
        ),
        (
            vec!["run", "hello", "--variant", "high"],
            2,
            "NotYetPorted: --variant without an explicit --model",
        ),
        (
            vec!["run", "hello", "--file", "missing.txt"],
            2,
            "NotYetPorted: --file",
        ),
        (
            vec!["run", "hello", "--port=0"],
            2,
            "NotYetPorted: run --port",
        ),
        (
            vec!["--hostname", "127.0.0.1"],
            2,
            "NotYetPorted: TUI --hostname",
        ),
        (vec!["acp"], 2, "NotYetPorted: command Acp"),
    ] {
        let result = output(home.command(&args), None);
        assert_eq!(
            result.status.code(),
            Some(code),
            "{args:?}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(
            String::from_utf8_lossy(&result.stderr).contains(diagnostic),
            "{args:?}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
}

#[cfg(unix)]
mod pty {
    use std::io::{Read, Write};
    use std::sync::mpsc;
    use std::thread::JoinHandle;
    use std::time::{Duration, Instant};

    use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

    use super::{Home, Provider};

    struct Terminal {
        master: Box<dyn MasterPty + Send>,
        child: Box<dyn Child + Send + Sync>,
        writer: Box<dyn Write + Send>,
        chunks: mpsc::Receiver<Vec<u8>>,
        reader: Option<JoinHandle<()>>,
        bytes: Vec<u8>,
        original: String,
    }

    impl Terminal {
        fn start(home: &Home) -> Self {
            let pair = native_pty_system()
                .openpty(PtySize {
                    rows: 24,
                    cols: 100,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .unwrap();
            let original = format!("{:?}", pair.master.get_termios().unwrap());
            let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_opencode"));
            command.env_clear();
            command.env("PATH", "/usr/bin:/bin");
            command.env("TERM", "xterm-256color");
            for (key, value) in home.env() {
                command.env(key, value);
            }
            command.cwd(home.0.path().join("project"));
            let child = pair.slave.spawn_command(command).unwrap();
            drop(pair.slave);
            let writer = pair.master.take_writer().unwrap();
            let mut reader = pair.master.try_clone_reader().unwrap();
            let (send, chunks) = mpsc::channel();
            let reader = std::thread::spawn(move || {
                let mut bytes = [0; 4096];
                while let Ok(count) = reader.read(&mut bytes) {
                    if count == 0 || send.send(bytes[..count].to_vec()).is_err() {
                        break;
                    }
                }
            });
            Self {
                master: pair.master,
                child,
                writer,
                chunks,
                reader: Some(reader),
                bytes: Vec::new(),
                original,
            }
        }

        fn wait(&mut self, marker: &str) {
            let deadline = Instant::now() + Duration::from_secs(15);
            while !self.screen().screen().contents().contains(marker) {
                assert!(
                    Instant::now() < deadline,
                    "waiting for {marker}: {}",
                    String::from_utf8_lossy(&self.bytes)
                );
                if let Ok(bytes) = self.chunks.recv_timeout(Duration::from_millis(50)) {
                    self.bytes.extend(bytes);
                }
            }
        }

        fn assert_restored_exit(&mut self, success: bool) {
            let deadline = Instant::now() + Duration::from_secs(10);
            let status = loop {
                if let Some(status) = self.child.try_wait().unwrap() {
                    break status;
                }
                assert!(Instant::now() < deadline, "TUI did not exit");
                std::thread::sleep(Duration::from_millis(20));
            };
            assert_eq!(status.success(), success);
            while let Ok(bytes) = self.chunks.recv_timeout(Duration::from_millis(50)) {
                self.bytes.extend(bytes);
            }
            assert!(!self.screen().screen().alternate_screen());
            assert!(!self.screen().screen().bracketed_paste());
            assert_eq!(
                format!("{:?}", self.master.get_termios().unwrap()),
                self.original
            );
        }

        fn screen(&self) -> vt100::Parser {
            let mut screen = vt100::Parser::new(24, 100, 0);
            screen.process(&self.bytes);
            screen
        }
    }

    impl Drop for Terminal {
        fn drop(&mut self) {
            if matches!(self.child.try_wait(), Ok(None)) {
                let _ = self.child.kill();
            }
            let _ = self.child.wait();
            if let Some(reader) = self.reader.take() {
                let _ = reader.join();
            }
        }
    }

    #[test]
    fn default_tui_interrupts_an_active_turn_and_restores_terminal() {
        let home = Home::new();
        let provider = Provider::delayed(false, Duration::from_secs(3));
        home.config(&provider.url);
        let mut terminal = Terminal::start(&home);
        terminal.wait("Enter sends");
        terminal.writer.write_all(b"interrupt this turn\r").unwrap();
        terminal.writer.flush().unwrap();
        provider
            .requests
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        terminal.writer.write_all(b"\x03").unwrap();
        terminal.writer.flush().unwrap();
        terminal.assert_restored_exit(false);
        assert!(String::from_utf8_lossy(&terminal.bytes).contains("Session interrupted"));
    }

    #[test]
    fn default_tui_restores_terminal_after_provider_error() {
        let home = Home::new();
        let provider = Provider::start(true);
        home.config(&provider.url);
        let mut terminal = Terminal::start(&home);
        terminal.wait("Enter sends");
        terminal.writer.write_all(b"fail this turn\r").unwrap();
        terminal.writer.flush().unwrap();
        terminal.assert_restored_exit(false);
        assert!(String::from_utf8_lossy(&terminal.bytes).contains("controlled provider failure"));
    }

    #[test]
    fn default_tui_runs_a_real_turn_and_restores_terminal() {
        let home = Home::new();
        let provider = Provider::start(false);
        home.config(&provider.url);
        let mut terminal = Terminal::start(&home);
        terminal.wait("Enter sends");
        assert!(terminal.screen().screen().alternate_screen());
        assert_ne!(
            format!("{:?}", terminal.master.get_termios().unwrap()),
            terminal.original
        );
        terminal.writer.write_all(b"hello from terminal\r").unwrap();
        terminal.writer.flush().unwrap();
        terminal.wait("controlled assistant reply");
        terminal.wait("Enter sends");
        let (_, body) = provider
            .requests
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        assert!(body["messages"].to_string().contains("hello from terminal"));
        terminal.writer.write_all(b"\x04").unwrap();
        terminal.writer.flush().unwrap();
        terminal.assert_restored_exit(true);
    }
}
