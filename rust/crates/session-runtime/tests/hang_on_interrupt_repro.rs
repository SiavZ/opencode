// Reproducer skeleton: on_interrupt hook panic/hang
// TODO: flesh out with real session-runtime test harness and assertions.

use tokio::time::{Duration, sleep};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn on_interrupt_hook_panic_repro() {
    // This test is a starting point for reproducing the case where an on_interrupt
    // hook panics or blocks indefinitely, possibly preventing settle/tail.send and
    // leaving Running/pending state with unresolved done waiters.
    //
    // Steps for whoever implements this test fully:
    // 1. Construct a BackgroundJobs instance (or use the session-runtime test harness).
    // 2. Start a background job with an on_interrupt hook that panics (panic!("boom")).
    // 3. Observe the job lifecycle: verify work future panic is caught and settled as Error.
    // 4. Start another job whose on_interrupt hook blocks (e.g., sleeps forever) and then trigger shutdown.
    // 5. Assert that shutdown returns and that done waiters are woken (with None or appropriate value) instead of hanging.
    // 6. Add variants: explicit cancel, later settlement after panic, restart same-id vs distinct-id behavior.

    // Placeholder sleep to make this test file runnable as a stub while implementation is added.
    sleep(Duration::from_millis(10)).await;
}
