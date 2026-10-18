//! Purpose: exercise terminal cleanup in a real PTY, including deliberate unwinding.
//! Scope: disposable test subprocess.
//! Uses:
//! rust/crates/
//! `-- ui-terminal/src/terminal.rs
//! External:
//! |-- crossterm::terminal
//! `-- signal_hook::{consts, low_level}
//! Used by:
//! rust/crates/
//! `-- ui-terminal/tests/terminal_pty.rs
//! Invariants: only a test-owned PTY is modified, restored modes precede DONE.
//! Contract tests: ui-terminal/tests/terminal_pty.rs

use std::io::{Read, Write};
use std::time::{Duration, Instant};

use opencode_ui_terminal::terminal::{TerminalError, TerminalGuard, TerminalOptions, restore};
use signal_hook::consts::{SIGHUP, SIGTERM};

fn checkpoint(marker: &str) {
    println!("{marker}");
    std::io::stdout().flush().unwrap();
    std::io::stdin().read_exact(&mut [0]).unwrap();
}

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "panic".into());
    std::panic::set_hook(Box::new(|_| {
        eprintln!(
            "PREVIOUS_HOOK raw={}",
            crossterm::terminal::is_raw_mode_enabled().unwrap()
        );
    }));
    let result = std::panic::catch_unwind(|| {
        let guard = TerminalGuard::with_options(TerminalOptions {
            mouse: mode != "no-mouse",
            kitty_keyboard: true,
        })
        .unwrap();
        assert!(matches!(
            TerminalGuard::acquire(),
            Err(TerminalError::Conflict)
        ));
        assert!(guard.is_active());
        if mode == "suspend" {
            guard.suspend().unwrap();
            guard.suspend().unwrap();
            assert!(!guard.is_active());
            checkpoint("SUSPENDED");
            guard.resume().unwrap();
            guard.resume().unwrap();
        }
        checkpoint("READY");
        if mode == "panic" {
            panic!("deliberate terminal guard panic");
        }
        if mode == "sighup" || mode == "sigterm" {
            signal_hook::low_level::raise(if mode == "sighup" { SIGHUP } else { SIGTERM }).unwrap();
            let deadline = Instant::now() + Duration::from_secs(3);
            while guard.termination_signal().is_none() || guard.is_active() {
                assert!(Instant::now() < deadline, "signal restoration timed out");
                std::thread::sleep(Duration::from_millis(5));
            }
            assert!(matches!(guard.resume(), Err(TerminalError::Terminated(_))));
        }
        if mode != "drop" && mode != "no-mouse" {
            guard.restore().unwrap();
            restore().unwrap();
        }
        drop(guard);
    });
    assert!(!crossterm::terminal::is_raw_mode_enabled().unwrap());
    restore().unwrap();
    checkpoint("DONE");
    std::process::exit(if result.is_err() { 101 } else { 0 });
}
