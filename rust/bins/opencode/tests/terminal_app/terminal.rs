//! Purpose: own a real executable PTY and assert exact restoration of its terminal state.
//! Scope: test child lifetime.
//! Uses:
//! rust/bins/
//! `-- opencode/tests/support/mod.rs
//! External:
//! |-- portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system}
//! `-- vt100::Parser
//! Used by:
//! rust/bins/
//! `-- opencode/tests/terminal_app/mod.rs
//! Invariants: children and reader threads are joined; all paths belong to isolated test roots.
//! Contract tests: terminal_app module

use super::super::support::{Home, binary};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub struct Terminal {
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    writer: Box<dyn Write + Send>,
    chunks: mpsc::Receiver<Vec<u8>>,
    reader: Option<JoinHandle<()>>,
    bytes: Vec<u8>,
    original: String,
}

impl Terminal {
    pub fn start(home: &Home, args: &[&str]) -> Self {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 30,
                cols: 110,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        let original = format!("{:?}", pair.master.get_termios().unwrap());
        let mut command = CommandBuilder::new(binary());
        command.args(args);
        command.env_clear();
        command.env("PATH", "/usr/bin:/bin");
        command.env("TERM", "xterm-256color");
        command.env("OPENCODE_DISABLE_MODELS_FETCH", "true");
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

    pub fn send(&mut self, text: &[u8]) {
        self.writer.write_all(text).unwrap();
        self.writer.flush().unwrap();
    }

    pub fn wait(&mut self, marker: &str) {
        self.wait_screen(marker, true);
    }

    pub fn wait_absent(&mut self, marker: &str) {
        self.wait_screen(marker, false);
    }

    fn wait_screen(&mut self, marker: &str, present: bool) {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            self.drain();
            if self.screen().screen().contents().contains(marker) == present {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "waiting for {marker}; screen={}",
                self.screen().screen().contents()
            );
            if let Ok(bytes) = self.chunks.recv_timeout(Duration::from_millis(20)) {
                self.bytes.extend(bytes);
            }
            if let Some(status) = self.child.try_wait().unwrap() {
                panic!(
                    "TUI exited {status:?} while waiting for {marker}; {}",
                    String::from_utf8_lossy(&self.bytes)
                );
            }
        }
    }

    fn drain(&mut self) {
        while let Ok(bytes) = self.chunks.try_recv() {
            self.bytes.extend(bytes);
        }
    }

    fn screen(&self) -> vt100::Parser {
        let mut parser = vt100::Parser::new(30, 110, 0);
        parser.process(&self.bytes);
        parser
    }

    pub fn restored(&mut self, success: bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        let status = loop {
            self.drain();
            if let Some(status) = self.child.try_wait().unwrap() {
                break status;
            }
            assert!(
                Instant::now() < deadline,
                "TUI failed to exit; {}",
                self.screen().screen().contents()
            );
            if let Ok(bytes) = self.chunks.recv_timeout(Duration::from_millis(20)) {
                self.bytes.extend(bytes);
            }
        };
        assert_eq!(
            status.success(),
            success,
            "{}",
            String::from_utf8_lossy(&self.bytes)
        );
        while let Ok(bytes) = self.chunks.recv_timeout(Duration::from_millis(40)) {
            self.bytes.extend(bytes);
        }
        let parser = self.screen();
        assert!(!parser.screen().alternate_screen());
        assert!(!parser.screen().bracketed_paste());
        assert!(!parser.screen().hide_cursor());
        assert_eq!(
            parser.screen().mouse_protocol_mode(),
            vt100::MouseProtocolMode::None
        );
        assert_eq!(
            format!("{:?}", self.master.get_termios().unwrap()),
            self.original
        );
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
