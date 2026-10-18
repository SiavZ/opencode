//! Purpose: portable wall clock, stopwatch, timer and cooperative yield for the interpreter.
//! Scope: time sources only. On `wasm32` there is no ambient clock; the host must supply one via
//! `ExecuteOptions::clock` (see contracts/phase-07/codemode.md blockers).
//! Uses:
//! `-- none
//! External:
//! |-- std::time::{Instant, SystemTime}
//! `-- tokio::time::sleep
//! Used by:
//! rust/crates/
//! |-- codemode/src/interp/mod.rs
//! |-- codemode/src/run.rs
//! `-- codemode/src/tools/runtime.rs
//! Invariants: no ambient time access on wasm; native time uses the system clock.
//! Contract tests: codemode/tests/codemode.rs

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

/// Milliseconds since the Unix epoch (`Date.now()`), or NaN when no clock exists.
pub(crate) fn now_ms() -> f64 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(f64::NAN, |duration| duration.as_millis() as f64)
    }
    #[cfg(target_arch = "wasm32")]
    {
        f64::NAN
    }
}

pub(crate) struct Stopwatch {
    #[cfg(not(target_arch = "wasm32"))]
    started: std::time::Instant,
}

impl Stopwatch {
    pub fn start() -> Self {
        Self {
            #[cfg(not(target_arch = "wasm32"))]
            started: std::time::Instant::now(),
        }
    }

    pub fn elapsed_ms(&self) -> f64 {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.started.elapsed().as_secs_f64() * 1000.0
        }
        #[cfg(target_arch = "wasm32")]
        {
            0.0
        }
    }
}

/// Resolves after `ms` milliseconds. `None` on targets without a timer (wasm), where execution
/// timeouts must be enforced by the host through cancellation.
pub(crate) fn sleep(ms: u64) -> Option<Pin<Box<dyn Future<Output = ()> + Send>>> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        Some(Box::pin(tokio::time::sleep(
            std::time::Duration::from_millis(ms),
        )))
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = ms;
        None
    }
}

/// Yields to the executor once so timers and cancellation can interrupt busy programs.
pub(crate) struct YieldNow(bool);

pub(crate) fn yield_now() -> YieldNow {
    YieldNow(false)
}

impl Future for YieldNow {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<()> {
        if self.0 {
            return Poll::Ready(());
        }
        self.0 = true;
        context.waker().wake_by_ref();
        Poll::Pending
    }
}
