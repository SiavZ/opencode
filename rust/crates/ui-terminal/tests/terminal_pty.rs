//! Purpose: verify real PTY terminal acquisition and restoration after lifecycle boundaries.
//! Scope: live subprocess tests with a minimal terminal protocol responder.
//! Uses:
//! rust/crates/
//! `-- ui-terminal/src/terminal.rs
//! External:
//! |-- portable_pty::{CommandBuilder, PtySize, native_pty_system}
//! `-- vt100::Parser
//! Used by:
//! `-- none, entrypoint
//! Invariants: tests never touch the user's terminal, workers are joined, children are reaped.
//! Contract tests: ui-terminal/tests/terminal_pty.rs

#![cfg(unix)]

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{OnceLock, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

fn example() -> &'static PathBuf {
    static EXAMPLE: OnceLock<PathBuf> = OnceLock::new();
    EXAMPLE.get_or_init(|| {
        let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let target = std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| workspace.join("target/p09"));
        let status = std::process::Command::new(env!("CARGO"))
            .current_dir(&workspace)
            .args([
                "build",
                "-p",
                "opencode-ui-terminal",
                "--example",
                "panic_restore",
            ])
            .env("CARGO_BUILD_JOBS", "3")
            .env("CARGO_TARGET_DIR", &target)
            .status()
            .unwrap();
        assert!(status.success(), "panic example build failed");
        target.join("debug/examples/panic_restore")
    })
}

struct Session {
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    writer: Box<dyn Write + Send>,
    reader: Option<JoinHandle<()>>,
    chunks: mpsc::Receiver<Vec<u8>>,
    output: Vec<u8>,
    answered: usize,
    kitty: bool,
    original_termios: String,
}

impl Session {
    fn start(mode: &str, kitty: bool) -> Self {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        let original_termios = format!("{:?}", pair.master.get_termios().unwrap());
        let mut command = CommandBuilder::new(example());
        command.arg(mode);
        command.env("TERM", "xterm-256color");
        let child = pair.slave.spawn_command(command).unwrap();
        drop(pair.slave);
        let writer = pair.master.take_writer().unwrap();
        let mut input = pair.master.try_clone_reader().unwrap();
        let (send, chunks) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            let mut bytes = [0; 4096];
            while let Ok(count) = input.read(&mut bytes) {
                if count == 0 || send.send(bytes[..count].to_vec()).is_err() {
                    break;
                }
            }
        });
        Self {
            master: pair.master,
            child,
            writer,
            reader: Some(reader),
            chunks,
            output: Vec::new(),
            answered: 0,
            kitty,
            original_termios,
        }
    }

    fn wait(&mut self, marker: &str) {
        while !String::from_utf8_lossy(&self.output).contains(marker) {
            let chunk = self
                .chunks
                .recv_timeout(Duration::from_secs(10))
                .unwrap_or_else(|error| {
                    panic!(
                        "waiting for {marker}: {error}; output={:?}",
                        String::from_utf8_lossy(&self.output)
                    )
                });
            self.output.extend(chunk);
            let queries = self
                .output
                .windows(4)
                .filter(|window| *window == b"\x1b[?u")
                .count();
            while self.answered < queries {
                self.writer
                    .write_all(if self.kitty {
                        b"\x1b[?0u\x1b[?1;2c"
                    } else {
                        b"\x1b[?1;2c"
                    })
                    .unwrap();
                self.writer.flush().unwrap();
                self.answered += 1;
            }
        }
    }

    fn acknowledge(&mut self) {
        // Newline also completes the canonical read at restored checkpoints.
        self.writer.write_all(b"\n").unwrap();
        self.writer.flush().unwrap();
    }

    fn screen(&self) -> vt100::Parser {
        let mut parser = vt100::Parser::new(24, 80, 0);
        parser.process(&self.output);
        parser
    }
}

impl Drop for Session {
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

fn exercise(mode: &str, kitty: bool) {
    let mut session = Session::start(mode, kitty);
    if mode == "suspend" {
        session.wait("SUSPENDED");
        assert!(!session.screen().screen().alternate_screen());
        assert!(!session.screen().screen().bracketed_paste());
        session.acknowledge();
    }
    session.wait("READY");
    let raw = session.master.get_termios().expect("PTY exposes termios");
    let active = session.screen();
    assert!(active.screen().alternate_screen());
    assert!(active.screen().bracketed_paste());
    assert!(active.screen().hide_cursor());
    assert_eq!(
        active.screen().mouse_protocol_mode() != vt100::MouseProtocolMode::None,
        mode != "no-mouse"
    );
    assert!(String::from_utf8_lossy(&session.output).contains("\x1b[?1004h"));
    assert_eq!(
        String::from_utf8_lossy(&session.output).contains("\x1b[>7u"),
        kitty
    );
    session.acknowledge();
    session.wait("DONE");
    let cooked = session
        .master
        .get_termios()
        .expect("restored PTY exposes termios");
    assert_eq!(
        format!("{cooked:?}"),
        session.original_termios,
        "restore exact original termios"
    );
    assert_ne!(
        raw.local_flags, cooked.local_flags,
        "raw mode must be disabled"
    );
    let restored = session.screen();
    assert!(!restored.screen().alternate_screen());
    assert!(!restored.screen().bracketed_paste());
    assert!(!restored.screen().hide_cursor());
    assert_eq!(
        restored.screen().mouse_protocol_mode(),
        vt100::MouseProtocolMode::None
    );
    let output = String::from_utf8_lossy(&session.output);
    assert!(output.contains("\x1b[?1004l"));
    assert!(
        output.contains("\x1b]0;\x07"),
        "title cleared before epilogue"
    );
    assert_eq!(
        output.matches("\x1b[<1u").count(),
        if kitty { session.answered } else { 0 }
    );
    assert_eq!(
        output.matches("\x1b[?1049l").count(),
        if mode == "suspend" { 2 } else { 1 },
        "restoration must be idempotent"
    );
    if mode == "panic" {
        assert!(
            output.contains("PREVIOUS_HOOK raw=false"),
            "panic hook chained after restoration"
        );
    }
    session.acknowledge();
    let status = session.child.wait().unwrap();
    assert_eq!(status.success(), mode != "panic");
}

#[test]
fn panic_restores_real_pty_and_chains_previous_hook() {
    exercise("panic", true);
}

#[test]
fn drop_restores_real_pty_without_kitty_support() {
    exercise("drop", false);
}

#[test]
fn suspend_resume_and_repeated_restore_are_idempotent() {
    exercise("suspend", true);
}

#[test]
fn sighup_restores_real_pty_and_prevents_resume() {
    exercise("sighup", true);
}

#[test]
fn sigterm_restores_real_pty_and_prevents_resume() {
    exercise("sigterm", true);
}

#[test]
fn mouse_capture_can_be_disabled() {
    exercise("no-mouse", false);
}
