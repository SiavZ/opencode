//! Purpose: confined CodeMode interpreter state: scopes, globals, cooperative scheduling, and the
//! program entrypoint (hoisting, completion value, implicit await, settlement drain).
//! Scope: `runtime.ts` Interpreter class shell (lines 588-700, 3349-3465 scope helpers).
//! Uses:
//! rust/crates/
//! |-- codemode/src/ast.rs
//! |-- codemode/src/clock.rs
//! |-- codemode/src/error.rs
//! |-- codemode/src/interp/call.rs
//! |-- codemode/src/interp/console.rs
//! |-- codemode/src/interp/diagnose.rs
//! |-- codemode/src/interp/expr.rs
//! |-- codemode/src/interp/members.rs
//! |-- codemode/src/interp/ops.rs
//! |-- codemode/src/interp/patterns.rs
//! |-- codemode/src/interp/pool.rs
//! |-- codemode/src/interp/promise.rs
//! |-- codemode/src/interp/stmt.rs
//! `-- codemode/src/value/mod.rs
//! External:
//! `-- futures::future::BoxFuture
//! Used by:
//! rust/crates/
//! `-- codemode/src/run.rs
//! Invariants: scopes are restored on every exit path; no ambient host access exists in globals.
//! Contract tests: codemode/tests/parity.rs, codemode/tests/promise.rs

mod call;
mod collections;
mod console;
pub(crate) mod diagnose;
mod expr;
mod members;
mod ops;
mod patterns;
pub(crate) mod pool;
mod promise;
mod stmt;

use crate::ast::{Loc, Stmt};
use crate::error::{Eval, Fault, fail_named, runtime};
use crate::value::{
    Binding, Coercion, ERROR_CONSTRUCTORS, GlobalName, ScopeRef, UriFn, Value, lock, shared,
};
use futures::future::BoxFuture;
use indexmap::IndexMap;
use pool::Pool;
use std::sync::Arc;

pub(crate) type Fut<'a, T> = BoxFuture<'a, Eval<T>>;

pub(crate) fn typeof_of(value: &Value) -> &'static str {
    ops::typeof_value(value)
}

pub(crate) fn spread_of(value: &Value) -> Option<Vec<Value>> {
    collections::spread_items(value)
}

/// Statement completion (`StatementResult`).
pub(crate) enum Completion {
    None,
    Value(Value),
    Return(Value),
    Break,
    Continue,
}

pub(crate) struct Interp {
    scopes: Vec<ScopeRef>,
    pub pool: Pool,
    pub logs: Vec<String>,
    last_value: Value,
    steps: u32,
}

impl Interp {
    pub fn new(pool: Pool) -> Self {
        let mut global = IndexMap::new();
        let mut set = |name: &str, value: Value| {
            global.insert(
                name.to_string(),
                Binding {
                    mutable: false,
                    value,
                    initialized: true,
                },
            );
        };
        set("tools", Value::Tool(Arc::new(Vec::new())));
        set("Promise", Value::PromiseNs);
        set("undefined", Value::Undefined);
        for name in [
            GlobalName::Object,
            GlobalName::Math,
            GlobalName::Json,
            GlobalName::Array,
            GlobalName::Console,
            GlobalName::Date,
            GlobalName::RegExp,
            GlobalName::Map,
            GlobalName::Set,
            GlobalName::Url,
            GlobalName::UrlSearchParams,
        ] {
            set(name.as_str(), Value::Global(name));
        }
        for coercion in [
            Coercion::Number,
            Coercion::String,
            Coercion::Boolean,
            Coercion::ParseInt,
            Coercion::ParseFloat,
        ] {
            set(coercion.as_str(), Value::Coercion(coercion));
        }
        for uri in [
            UriFn::EncodeUri,
            UriFn::EncodeUriComponent,
            UriFn::DecodeUri,
            UriFn::DecodeUriComponent,
        ] {
            set(uri.as_str(), Value::Uri(uri));
        }
        for name in ERROR_CONSTRUCTORS {
            set(name, Value::ErrorCtor(name));
        }
        set("NaN", Value::Number(f64::NAN));
        set("Infinity", Value::Number(f64::INFINITY));
        Self {
            scopes: vec![shared(global)],
            pool,
            logs: Vec::new(),
            last_value: Value::Undefined,
            steps: 0,
        }
    }

    /// Runs the program body in a module scope above the builtin global scope.
    pub async fn run(&mut self, program: &[Stmt]) -> Eval<Value> {
        self.push_scope();
        let result = self.run_body(program).await;
        self.pop_scope();
        let mut value = result?;
        if let Value::Promise(promise) = &value {
            value = self.settle(promise, None).await?;
        }
        self.drain_pending().await?;
        Ok(value)
    }

    async fn run_body(&mut self, program: &[Stmt]) -> Eval<Value> {
        self.hoist(program)?;
        for statement in program {
            match self.stmt(statement).await? {
                Completion::Return(value) => return Ok(value),
                Completion::Break => {
                    return Err(runtime(
                        "Unexpected 'break' outside of a loop.",
                        stmt_loc(statement),
                    )
                    .into());
                }
                Completion::Continue => {
                    return Err(runtime(
                        "Unexpected 'continue' outside of a loop.",
                        stmt_loc(statement),
                    )
                    .into());
                }
                Completion::Value(value) => self.last_value = value,
                Completion::None => {}
            }
        }
        Ok(self.last_value.clone())
    }

    /// Cooperative scheduling point: drives in-flight tool calls and lets timers and
    /// cancellation interrupt busy programs.
    pub async fn tick(&mut self) -> Eval<()> {
        self.steps = self.steps.wrapping_add(1);
        if !self.steps.is_multiple_of(128) {
            return Ok(());
        }
        if self.pool.tools().cancel.is_cancelled() {
            return Err(Fault::Interrupted);
        }
        futures::future::poll_fn(|context| {
            self.pool.poll_all(context);
            std::task::Poll::Ready(())
        })
        .await;
        crate::clock::yield_now().await;
        Ok(())
    }

    pub fn push_scope(&mut self) {
        self.scopes.push(shared(IndexMap::new()));
    }

    pub fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn current_scope(&self) -> ScopeRef {
        self.scopes
            .last()
            .cloned()
            .unwrap_or_else(|| shared(IndexMap::new()))
    }

    pub fn declare(&mut self, name: &str, value: Value, mutable: bool, loc: Loc) -> Eval<()> {
        let scope = self.current_scope();
        let mut scope = lock(&scope);
        if scope.get(name).is_some_and(|binding| binding.initialized) {
            return Err(crate::error::fail(
                format!("Identifier '{name}' has already been declared."),
                loc,
            ));
        }
        scope.insert(
            name.to_string(),
            Binding {
                mutable,
                value,
                initialized: true,
            },
        );
        Ok(())
    }

    pub fn resolve_binding(&self, name: &str) -> Option<ScopeRef> {
        self.scopes
            .iter()
            .rev()
            .find(|scope| lock(scope).contains_key(name))
            .cloned()
    }

    pub fn get_identifier(&self, name: &str, loc: Loc) -> Eval<Value> {
        let Some(scope) = self.resolve_binding(name) else {
            return Err(fail_named(
                format!("Unknown identifier '{name}'."),
                loc,
                "ReferenceError",
            ));
        };
        let scope = lock(&scope);
        let binding = scope.get(name).cloned();
        match binding {
            Some(binding) if !binding.initialized => Err(fail_named(
                format!("Cannot access '{name}' before initialization."),
                loc,
                "ReferenceError",
            )),
            Some(binding) => Ok(binding.value),
            None => Ok(Value::Undefined),
        }
    }

    pub fn set_identifier(&mut self, name: &str, value: Value, loc: Loc) -> Eval<Value> {
        let Some(scope) = self.resolve_binding(name) else {
            return Err(fail_named(
                format!("Unknown identifier '{name}'."),
                loc,
                "ReferenceError",
            ));
        };
        let mut scope = lock(&scope);
        let Some(binding) = scope.get_mut(name) else {
            return Ok(value);
        };
        if !binding.mutable {
            return Err(fail_named(
                format!("Cannot assign to constant '{name}'."),
                loc,
                "TypeError",
            ));
        }
        binding.value = value.clone();
        Ok(value)
    }

    /// Binds function declarations ahead of the statements that use them.
    fn hoist(&mut self, statements: &[Stmt]) -> Eval<()> {
        for statement in statements {
            if let Stmt::Function {
                name,
                function,
                loc,
            } = statement
            {
                let value = self.create_function(function);
                self.declare(name, value, true, *loc)?;
            }
        }
        Ok(())
    }

    fn create_function(&self, function: &Arc<crate::ast::FunctionNode>) -> Value {
        Value::Function(Arc::new(crate::value::FunctionValue {
            node: function.clone(),
            captured: self.scopes.clone(),
        }))
    }
}

fn stmt_loc(statement: &Stmt) -> Option<Loc> {
    match statement {
        Stmt::Break { loc, .. } | Stmt::Continue { loc, .. } => Some(*loc),
        _ => None,
    }
}
