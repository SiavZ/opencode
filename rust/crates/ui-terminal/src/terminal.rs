//! Purpose: acquire terminal modes and restore them on drop, panic, signals and suspension.
//! Scope: process-wide terminal lifecycle.
//! Uses:
//! `-- none
//! External:
//! |-- crossterm::{cursor, event, terminal}
//! |-- signal_hook::{consts, iterator}
//! `-- thiserror::Error
//! Used by:
//! rust/crates/
//! |-- ui-terminal/src/lib.rs
//! `-- ui-terminal/tests/terminal_pty.rs
//! Invariants: one owner, restoration is idempotent, signal worker is closed and joined.
//! Contract tests: ui-terminal/tests/terminal_pty.rs

use std::io;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex, Once, Weak};
use std::thread::JoinHandle;

use crossterm::cursor::{Hide, Show};
use crossterm::event::{
    DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
    EnableFocusChange, EnableMouseCapture, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, SetTitle, disable_raw_mode, enable_raw_mode,
    supports_keyboard_enhancement,
};
use signal_hook::consts::{SIGHUP, SIGTERM};
use signal_hook::iterator::{Handle, Signals};

static OWNER: Mutex<Option<Weak<State>>> = Mutex::new(None);
static PANIC_HOOK: Once = Once::new();

#[derive(Debug, thiserror::Error)]
pub enum TerminalError {
    #[error("terminal already has an owner")]
    Conflict,
    #[error("terminal I/O: {0}")]
    Io(#[from] io::Error),
    #[error("terminal has received shutdown signal {0}")]
    Terminated(i32),
}

#[derive(Debug, Clone, Copy)]
pub struct TerminalOptions {
    pub mouse: bool,
    pub kitty_keyboard: bool,
}

impl Default for TerminalOptions {
    fn default() -> Self {
        Self {
            mouse: true,
            kitty_keyboard: true,
        }
    }
}

#[derive(Default)]
struct Modes {
    active: bool,
    kitty: bool,
}

struct State {
    modes: Mutex<Modes>,
    options: TerminalOptions,
    signal: AtomicI32,
}

impl State {
    fn enter(&self) -> Result<(), TerminalError> {
        let mut modes = self.modes.lock().unwrap_or_else(|error| error.into_inner());
        let signal = self.signal.load(Ordering::Acquire);
        if signal != 0 {
            return Err(TerminalError::Terminated(signal));
        }
        if modes.active {
            return Ok(());
        }
        enable_raw_mode()?;
        // Mark active before writes so partially successful acquisition is rolled back.
        modes.active = true;
        let result = (|| -> io::Result<()> {
            let mut output = io::stdout();
            execute!(
                output,
                EnterAlternateScreen,
                EnableBracketedPaste,
                EnableFocusChange,
                Hide
            )?;
            if self.options.mouse {
                execute!(output, EnableMouseCapture)?;
            }
            if self.options.kitty_keyboard && supports_keyboard_enhancement().unwrap_or(false) {
                // Mark before writing so a flush failure still pops the protocol stack.
                modes.kitty = true;
                execute!(
                    output,
                    PushKeyboardEnhancementFlags(
                        KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                            | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
                            | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
                    )
                )?;
            }
            Ok(())
        })();
        drop(modes);
        if result.is_err() {
            let _ = self.restore();
        }
        result.map_err(TerminalError::Io)
    }

    fn restore(&self) -> io::Result<()> {
        let mut modes = self.modes.lock().unwrap_or_else(|error| error.into_inner());
        if !modes.active {
            return Ok(());
        }
        modes.active = false;
        let mut output = io::stdout();
        // Try every cleanup even if an earlier write fails. Raw mode restoration must
        // not depend on stdout remaining writable (for example after SIGHUP).
        let kitty = if modes.kitty {
            modes.kitty = false;
            execute!(output, PopKeyboardEnhancementFlags)
        } else {
            Ok(())
        };
        let mouse = execute!(output, DisableMouseCapture);
        let paste = execute!(output, DisableBracketedPaste);
        let focus = execute!(output, DisableFocusChange);
        let screen = execute!(output, Show, LeaveAlternateScreen, SetTitle(""));
        let raw = disable_raw_mode();
        kitty.and(mouse).and(paste).and(focus).and(screen).and(raw)
    }
}

/// A unique process-wide terminal owner. Drop never panics, including during unwind.
pub struct TerminalGuard {
    state: Arc<State>,
    signals: Handle,
    worker: Option<JoinHandle<()>>,
}

impl TerminalGuard {
    pub fn acquire() -> Result<Self, TerminalError> {
        Self::with_options(TerminalOptions::default())
    }

    pub fn with_options(options: TerminalOptions) -> Result<Self, TerminalError> {
        let mut owner = OWNER.lock().unwrap_or_else(|error| error.into_inner());
        if owner.as_ref().and_then(Weak::upgrade).is_some() {
            return Err(TerminalError::Conflict);
        }
        PANIC_HOOK.call_once(|| {
            let previous = std::panic::take_hook();
            std::panic::set_hook(Box::new(move |info| {
                let _ = restore();
                previous(info);
            }));
        });
        let mut signals = Signals::new([SIGTERM, SIGHUP])?;
        let state = Arc::new(State {
            modes: Mutex::new(Modes::default()),
            options,
            signal: AtomicI32::new(0),
        });
        // The hook must be able to reach state before acquisition can panic.
        *owner = Some(Arc::downgrade(&state));
        drop(owner);
        state.enter()?;
        let handle = signals.handle();
        let worker_state = Arc::clone(&state);
        let worker = std::thread::Builder::new()
            .name("terminal-signals".into())
            .spawn(move || {
                for signal in signals.forever() {
                    worker_state.signal.store(signal, Ordering::Release);
                    let _ = worker_state.restore();
                }
            });
        match worker {
            Ok(worker) => Ok(Self {
                state,
                signals: handle,
                worker: Some(worker),
            }),
            Err(error) => {
                let _ = state.restore();
                Err(error.into())
            }
        }
    }

    /// Restore before handing control to a shell or job-control suspension.
    /// The caller owns SIGTSTP/SIGCONT delivery and invokes resume on continuation.
    pub fn suspend(&self) -> Result<(), TerminalError> {
        self.state.restore().map_err(TerminalError::Io)
    }

    pub fn resume(&self) -> Result<(), TerminalError> {
        self.state.enter()
    }

    pub fn restore(&self) -> Result<(), TerminalError> {
        self.suspend()
    }

    /// The app must observe this and leave its scoped run, disposing plugins before
    /// printing any epilogue. Signal handlers restore modes but never exit the process.
    pub fn termination_signal(&self) -> Option<i32> {
        match self.state.signal.load(Ordering::Acquire) {
            0 => None,
            signal => Some(signal),
        }
    }

    pub fn is_active(&self) -> bool {
        self.state
            .modes
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .active
    }
}

/// Idempotent emergency restoration shared by panic hooks and external shutdown.
pub fn restore() -> io::Result<()> {
    let state = OWNER
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .as_ref()
        .and_then(Weak::upgrade);
    state.map_or(Ok(()), |state| state.restore())
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        self.signals.close();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        let _ = self.state.restore();
        let mut owner = OWNER.lock().unwrap_or_else(|error| error.into_inner());
        if owner
            .as_ref()
            .and_then(Weak::upgrade)
            .is_some_and(|state| Arc::ptr_eq(&state, &self.state))
        {
            *owner = None;
        }
    }
}
