//! Purpose: capture reentrant updater subscription traces from the immutable TypeScript baseline.
//! Scope: isolated Bun process, no filesystem or application side effects.
//! Uses:
//! packages/desktop/
//! `-- src/main/updater-subscriptions.ts
//! External:
//! `-- node:assert::deepStrictEqual
//! Used by:
//! `-- none, entrypoint
//! Invariants: imports the real baseline implementation; never copies its Map implementation.
//! Contract tests: desktop-host/tests/updater_subscriptions.rs

import { deepStrictEqual } from "node:assert"
import { createUpdaterSubscriptions } from "../../../../packages/desktop/src/main/updater-subscriptions"

const traces: Record<string, number[]> = {}
{
  const subscriptions = createUpdaterSubscriptions()
  const events: number[] = []
  subscriptions.set(1, () => {
    events.push(1)
    subscriptions.set(3, () => events.push(3))
  })
  subscriptions.set(2, () => events.push(2))
  subscriptions.clear()
  subscriptions.clear()
  traces.insert = events
}
{
  const subscriptions = createUpdaterSubscriptions()
  const events: number[] = []
  subscriptions.set(1, () => {
    events.push(1)
    subscriptions.delete(2)
    subscriptions.set(2, () => events.push(20))
  })
  subscriptions.set(2, () => events.push(2))
  subscriptions.set(3, () => events.push(3))
  subscriptions.clear()
  traces.replace = events
}
{
  const subscriptions = createUpdaterSubscriptions()
  const events: number[] = []
  let entered = false
  subscriptions.set(1, () => {
    events.push(1)
    if (entered) return
    entered = true
    subscriptions.set(1, () => events.push(10))
  })
  subscriptions.delete(1)
  subscriptions.clear()
  traces.nested = events
}
deepStrictEqual(traces, { insert: [1, 2, 3], replace: [1, 2, 3, 20], nested: [1, 1] })
console.log(JSON.stringify(traces))
