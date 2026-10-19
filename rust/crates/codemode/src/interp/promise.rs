//! Purpose: promise semantics: await settlement, Promise.all/allSettled/race/resolve/reject, and
//! the end-of-program drain that surfaces unhandled rejections.
//! Scope: `runtime.ts` drainPendingSettlements/observePromise/settlePromise/invokePromiseMethod
//! (lines 702-771, 2103-2210).
//! Uses:
//! rust/crates/
//! |-- codemode/src/ast.rs
//! |-- codemode/src/error.rs
//! |-- codemode/src/interp/collections.rs
//! |-- codemode/src/interp/diagnose.rs
//! |-- codemode/src/interp/mod.rs
//! `-- codemode/src/value/mod.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! `-- codemode/src/interp/mod.rs
//! Invariants: combinators observe settlements and never re-run calls; interruption from teardown
//! is never converted into program-visible data.
//! Contract tests: codemode/tests/promise.rs

use super::Interp;
use super::diagnose::{caught_value, normalize};
use crate::ast::Loc;
use crate::error::{Eval, Fault, ValueBox, fail, runtime};
use crate::value::{ObjectData, PromiseData, PromiseMethod, PromiseState, Value};
use std::sync::Arc;

const RACE_LOST: &str =
    "This tool call was interrupted because another value settled a Promise.race first.";

impl Interp {
    /// Settlement of a promise, marking it observed.
    async fn observe(&mut self, promise: &PromiseData) -> (Eval<Value>, bool) {
        match &promise.state {
            PromiseState::Ready(result) => (result.clone(), false),
            PromiseState::Task(id) => {
                self.pool.unobserved.shift_remove(id);
                self.pool.wait(*id).await
            }
        }
    }

    /// `await promise`: the fulfilled value, or the failure re-raised at the await site.
    pub(super) async fn settle(
        &mut self,
        promise: &Arc<PromiseData>,
        loc: Option<Loc>,
    ) -> Eval<Value> {
        let (result, race_interrupted) = self.observe(promise).await;
        unwrap_settlement(result, race_interrupted, loc)
    }

    /// Awaits abandoned tool calls; a failure nobody observed becomes a diagnostic.
    pub(super) async fn drain_pending(&mut self) -> Eval<()> {
        while let Some(id) = self.pool.unobserved.shift_remove_index(0) {
            let (result, _) = self.pool.wait(id).await;
            match result {
                Ok(_) | Err(Fault::Interrupted) => {}
                Err(fault) => {
                    let failure = normalize(&fault);
                    let mut error = runtime(
                        format!(
                            "Unhandled rejection from an un-awaited tool call: {}",
                            failure.message
                        ),
                        None,
                    );
                    error.kind = failure.kind;
                    error.suggestions = Some(vec![
                        "Await tool calls - `const result = await tools.ns.tool(...)` - so failures can be caught and handled."
                            .into(),
                    ]);
                    return Err(error.into());
                }
            }
        }
        Ok(())
    }

    pub(super) async fn promise_method(
        &mut self,
        method: PromiseMethod,
        args: Vec<Value>,
        loc: Loc,
    ) -> Eval<Value> {
        let first = args.into_iter().next().unwrap_or(Value::Undefined);
        match method {
            PromiseMethod::Resolve => {
                return Ok(match first {
                    Value::Promise(_) => first,
                    value => Value::Promise(Arc::new(PromiseData {
                        state: PromiseState::Ready(Ok(value)),
                    })),
                });
            }
            PromiseMethod::Reject => {
                let state = PromiseState::Ready(Err(Fault::Throw(ValueBox(first))));
                return Ok(Value::Promise(Arc::new(PromiseData { state })));
            }
            _ => {}
        }
        let name = match method {
            PromiseMethod::All => "all",
            PromiseMethod::AllSettled => "allSettled",
            _ => "race",
        };
        let Some(items) = super::collections::spread_items(&first) else {
            return Err(fail(
                format!(
                    "Promise.{name} expects an array of promises or plain values (e.g. Promise.{name}(items.map((item) => tools.ns.tool(item))))."
                ),
                loc,
            ));
        };
        // Every member's settlement counts as observed up front, like JS combinators.
        for item in &items {
            if let Value::Promise(promise) = item
                && let PromiseState::Task(id) = promise.state
            {
                self.pool.unobserved.shift_remove(&id);
            }
        }
        match method {
            PromiseMethod::All => {
                let mut values = Vec::with_capacity(items.len());
                for item in items {
                    values.push(match &item {
                        Value::Promise(promise) => self.settle(promise, Some(loc)).await?,
                        _ => item,
                    });
                }
                Ok(Value::array(values))
            }
            PromiseMethod::AllSettled => {
                let mut outcomes = Vec::with_capacity(items.len());
                for item in items {
                    let (result, race_interrupted) = match &item {
                        Value::Promise(promise) => self.observe(promise).await,
                        _ => (Ok(item.clone()), false),
                    };
                    let outcome = match result {
                        Ok(value) => ObjectData::from_pairs([
                            ("status".into(), Value::str("fulfilled")),
                            ("value".into(), value),
                        ]),
                        Err(Fault::Interrupted) if !race_interrupted => {
                            return Err(Fault::Interrupted);
                        }
                        Err(fault) => {
                            let fault = if matches!(fault, Fault::Interrupted) {
                                fail(RACE_LOST, loc)
                            } else {
                                fault
                            };
                            ObjectData::from_pairs([
                                ("status".into(), Value::str("rejected")),
                                ("reason".into(), caught_value(&fault)),
                            ])
                        }
                    };
                    outcomes.push(Value::object(outcome));
                }
                Ok(Value::array(outcomes))
            }
            _ => self.race(items, loc).await,
        }
    }

    async fn race(&mut self, items: Vec<Value>, loc: Loc) -> Eval<Value> {
        if items.is_empty() {
            return Err(fail(
                "Promise.race([]) would never settle; provide at least one promise or value.",
                loc,
            ));
        }
        let tasks: Vec<Option<usize>> = items
            .iter()
            .map(|item| match item {
                Value::Promise(promise) => match promise.state {
                    PromiseState::Task(id) => Some(id),
                    PromiseState::Ready(_) => None,
                },
                _ => None,
            })
            .collect();
        // Plain values and ready promises settle immediately, in list order.
        let winner = match items.iter().position(|item| !matches!(item, Value::Promise(promise) if matches!(promise.state, PromiseState::Task(_)))) {
            Some(index) if tasks[..index].iter().flatten().all(|id| !self.pool.is_done(*id)) => index,
            _ => {
                let ids: Vec<usize> = tasks.iter().flatten().copied().collect();
                let first_done = match ids.iter().position(|id| self.pool.is_done(*id)) {
                    Some(position) => ids[position],
                    None => self.pool.wait_any(&ids).await,
                };
                tasks.iter().position(|id| *id == Some(first_done)).unwrap_or(0)
            }
        };
        for (index, id) in tasks.iter().enumerate() {
            if index != winner
                && let Some(id) = id
            {
                self.pool.interrupt(*id);
            }
        }
        match &items[winner] {
            Value::Promise(promise) => {
                let (result, race_interrupted) = self.observe(promise).await;
                unwrap_settlement(result, race_interrupted, Some(loc))
            }
            value => Ok(value.clone()),
        }
    }
}

fn unwrap_settlement(result: Eval<Value>, race_interrupted: bool, loc: Option<Loc>) -> Eval<Value> {
    match result {
        Err(Fault::Interrupted) if race_interrupted => Err(runtime(RACE_LOST, loc).into()),
        other => other,
    }
}
