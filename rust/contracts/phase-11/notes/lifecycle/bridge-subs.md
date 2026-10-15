# Bridge Subscriptions Registry

## Inventory

| Baseline (file:line) | Rust Item | Test | Contract | Blockers |
|---|---|---|---|---|
| `packages/desktop/src/main/updater-subscriptions.ts:1-20` | `SubscriptionRegistry::subscribe`, `teardown_sender` | `test_replaces_previous_subscription`, `test_sender_teardown` | Re-subscribing replaces previous; window destruction disposes all sender subscriptions. | None |
| `packages/desktop/src/main/ipc.ts:83-94` | `SubscriptionHandle::dispose`, `Drop` | `test_idempotent_dispose` | Disposal is idempotent; dropping handle cleans up registry state. | None |
| `packages/desktop/src/preload/index.ts:40-55` | `SubscriptionRegistry::send` with `OverflowBehavior` | `test_overflow_drop_newest`, `test_overflow_drop_oldest`, `test_overflow_block` | Bounded per-sender delivery channels with explicit overflow behavior. | `DropOldest` behaves as `DropNewest` at sender level due to `tokio::sync::mpsc::Sender` lacking `try_recv`. Documented limitation. |

## Notes

- The registry is generic over `K: Eq + Hash + Send + Sync + 'static` (topic) and `V: Send + 'static` (value), avoiding any dependency on `bridge/catalog.rs` types.
- `SubscriptionHandle` provides explicit `dispose()` and implements `Drop` for automatic cleanup.
- `teardown_sender` efficiently removes all topics for a given sender, simulating window destruction.
- Overflow behavior is explicitly documented: `tokio::sync::mpsc` does not support sender-side `try_recv`, making true `DropOldest` impossible without receiver coordination or a custom channel primitive.
