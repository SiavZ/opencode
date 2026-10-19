//! Purpose: call and construction dispatch: tool calls (eager promises), user functions,
//! intrinsic methods, global statics, coercions, URI helpers, error constructors, `new`.
//! Scope: `runtime.ts` evaluateCallExpression/invokeFunction/invokeIntrinsic/evaluateNewExpression
//! (lines 1472-1510, 1856-1930, 2210-2290).
//! Uses:
//! rust/crates/
//! |-- codemode/src/ast.rs
//! |-- codemode/src/error.rs
//! |-- codemode/src/interp/mod.rs
//! |-- codemode/src/stdlib/mod.rs
//! |-- codemode/src/value/convert.rs
//! `-- codemode/src/value/mod.rs
//! External:
//! |-- futures::future::poll_fn
//! |-- stdlib::collections::construct_map
//! |-- stdlib::collections::construct_set
//! |-- stdlib::date::construct
//! |-- stdlib::date::invoke_method
//! |-- stdlib::invoke_global
//! |-- stdlib::number::invoke_method
//! |-- stdlib::regexp::construct
//! |-- stdlib::regexp::invoke_method
//! |-- stdlib::string::invoke_method
//! |-- stdlib::url::construct_params
//! |-- stdlib::url::construct_url
//! |-- stdlib::url::invoke_uri
//! |-- stdlib::url::invoke_url_method
//! `-- stdlib::value::invoke_coercion
//! Used by:
//! rust/crates/
//! |-- codemode/src/interp/collections.rs
//! `-- codemode/src/interp/mod.rs
//! Invariants: only tools, functions and allowlisted builtins are callable; tool calls start at
//! the call site, before any await.
//! Contract tests: codemode/tests/codemode.rs, codemode/tests/promise.rs, codemode/tests/enumeration.rs

use super::{Fut, Interp};
use crate::ast::{Arg, Expr, FunctionBody, Loc};
use crate::error::{
    DiagnosticKind, Eval, fail, fail_kind, unsupported_feature, unsupported_syntax,
};
use crate::stdlib;
use crate::value::convert::{bounded, to_string};
use crate::value::{FunctionValue, ObjectData, PromiseData, PromiseState, Value, lock, shared};
use std::sync::Arc;

impl Interp {
    pub(super) async fn call(
        &mut self,
        callee: &Expr,
        args: &[Arg],
        optional: bool,
        loc: Loc,
    ) -> Eval<Value> {
        let callable = self.expr(callee).await?;
        if matches!(callable, Value::ShortCircuit) {
            return Ok(Value::ShortCircuit);
        }
        if callable.is_nullish() && optional {
            return Ok(Value::ShortCircuit);
        }
        let args = self.eval_args(args).await?;
        self.apply(callable, args, loc, callee.loc()).await
    }

    pub(super) fn apply<'a>(
        &'a mut self,
        callable: Value,
        args: Vec<Value>,
        loc: Loc,
        callee_loc: Loc,
    ) -> Fut<'a, Value> {
        Box::pin(async move {
            match callable {
                Value::Tool(path) => {
                    if path.is_empty() {
                        return Err(fail("The tools root is not callable.", callee_loc));
                    }
                    Ok(self.start_tool_call(path.as_ref().clone(), args).await)
                }
                Value::PromiseMethod(method) => self.promise_method(method, args, loc).await,
                Value::Function(function) => self.invoke(&function, args).await,
                Value::Intrinsic(reference) => {
                    self.intrinsic(&reference.0, &reference.1, args, loc).await
                }
                Value::GlobalMethod(reference) => {
                    let (namespace, name) = (reference.0.as_str(), reference.1.as_str());
                    if namespace == "console" {
                        return self.console(name, &args, loc);
                    }
                    if namespace == "Object"
                        && let Some(Value::Tool(path)) = args.first()
                    {
                        if name == "keys" {
                            let keys = self.pool.tools().keys(path)?;
                            return Ok(Value::array(keys.into_iter().map(Value::str).collect()));
                        }
                        return Err(fail_kind(
                            format!(
                                "Object.{name}(...) cannot read tool references: they are not plain data. Use Object.keys(tools) for names, or tools.$codemode.search({{ query }}) for signatures."
                            ),
                            loc,
                            DiagnosticKind::InvalidDataValue,
                        ));
                    }
                    let result = stdlib::invoke_global(namespace, name, &args, loc)?;
                    bounded(&result, &format!("{namespace}.{name} result"))
                }
                Value::Coercion(coercion) => {
                    let result = stdlib::value::invoke_coercion(coercion, &args, loc)?;
                    bounded(&result, &format!("{} result", coercion.as_str()))
                }
                Value::Uri(function) => stdlib::url::invoke_uri(function, &args, loc),
                Value::ErrorCtor(name) => Ok(error_value(name, args.first())),
                _ => Err(fail("Only tools are callable in CodeMode.", callee_loc)),
            }
        })
    }

    /// Admits a tool call on the task pool and polls it once so host work begins immediately.
    async fn start_tool_call(&mut self, path: Vec<String>, args: Vec<Value>) -> Value {
        let id = self.pool.spawn(path, args);
        futures::future::poll_fn(|context| {
            self.pool.poll_one(id, context);
            std::task::Poll::Ready(())
        })
        .await;
        Value::Promise(Arc::new(PromiseData {
            state: PromiseState::Task(id),
        }))
    }

    pub(super) fn invoke<'a>(
        &'a mut self,
        function: &'a Arc<FunctionValue>,
        args: Vec<Value>,
    ) -> Fut<'a, Value> {
        Box::pin(async move {
            let saved = std::mem::replace(&mut self.scopes, function.captured.clone());
            self.push_scope();
            let result = self.invoke_body(function, args).await;
            self.scopes = saved;
            result
        })
    }

    async fn invoke_body(&mut self, function: &FunctionValue, args: Vec<Value>) -> Eval<Value> {
        let node = &function.node;
        // Seed parameter TDZ slots so defaults referencing later parameters fail like JS.
        let mut names = Vec::new();
        for parameter in node.params.iter().chain(node.rest.iter()) {
            parameter.names(&mut names);
        }
        {
            let scope = self.current_scope();
            let mut scope = lock(&scope);
            for name in names {
                scope.insert(
                    name,
                    crate::value::Binding {
                        mutable: true,
                        value: Value::Undefined,
                        initialized: false,
                    },
                );
            }
        }
        for (index, parameter) in node.params.iter().enumerate() {
            let value = args.get(index).cloned().unwrap_or(Value::Undefined);
            self.bind_pattern(parameter, value, true, parameter.loc())
                .await?;
        }
        if let Some(rest) = &node.rest {
            let tail = args
                .get(node.params.len()..)
                .map(<[Value]>::to_vec)
                .unwrap_or_default();
            self.bind_pattern(rest, Value::array(tail), true, rest.loc())
                .await?;
        }
        match &node.body {
            FunctionBody::Block(body) => Ok(match self.stmt(body).await? {
                super::Completion::Return(value) | super::Completion::Value(value) => value,
                _ => Value::Undefined,
            }),
            FunctionBody::Expr(expression) => self.expr(expression).await,
        }
    }

    async fn intrinsic(
        &mut self,
        receiver: &Value,
        name: &str,
        args: Vec<Value>,
        loc: Loc,
    ) -> Eval<Value> {
        match receiver {
            Value::Str(text) => {
                if matches!(name, "replace" | "replaceAll") && args.get(1).is_some_and(is_callback)
                {
                    return self.string_replacer(text, name, args, loc).await;
                }
                stdlib::string::invoke_method(text, name, &args, loc)
            }
            Value::Number(number) => stdlib::number::invoke_method(*number, name, &args, loc),
            Value::Array(array) => self.array_method(array.clone(), name, args, loc).await,
            Value::Date(time) => stdlib::date::invoke_method(**time, name, loc),
            Value::RegExp(regex) => stdlib::regexp::invoke_method(regex, name, &args, loc),
            Value::Map(map) => self.map_method(map.clone(), name, args, loc).await,
            Value::Set(set) => self.set_method(set.clone(), name, args, loc).await,
            Value::Url(data) => stdlib::url::invoke_url_method(&lock(data), name, loc),
            Value::Params(params) => self.params_method(params.clone(), name, args, loc).await,
            _ => Err(fail(
                format!("Method '{name}' is not available in CodeMode."),
                loc,
            )),
        }
    }

    pub(super) async fn new_expr(&mut self, callee: &Expr, args: &[Arg], loc: Loc) -> Eval<Value> {
        let Expr::Ident(name, _) = callee else {
            return Err(unsupported_syntax("NewExpression", loc));
        };
        if name == "Promise" {
            return Err(unsupported_feature(
                "new Promise(...) is not supported in CodeMode; tool calls already return promises - call the tool and await the result.",
                loc,
            ));
        }
        if let Some(error) = crate::value::ERROR_CONSTRUCTORS
            .iter()
            .find(|item| **item == name.as_str())
        {
            let first = match args.first() {
                Some(Arg::Expr(expression)) => Some(self.expr(expression).await?),
                _ => None,
            };
            return Ok(error_value(error, first.as_ref()));
        }
        let args = self.eval_args(args).await?;
        match name.as_str() {
            "Date" => Ok(stdlib::date::construct(&args)),
            "RegExp" => stdlib::regexp::construct(&args, loc),
            "Map" => stdlib::collections::construct_map(args.first(), loc),
            "Set" => stdlib::collections::construct_set(args.first(), loc),
            "URL" => stdlib::url::construct_url(&args, loc),
            "URLSearchParams" => stdlib::url::construct_params(args.first(), loc),
            _ => Err(unsupported_syntax("NewExpression", loc)),
        }
    }
}

pub(super) fn is_callback(value: &Value) -> bool {
    matches!(
        value,
        Value::Function(_) | Value::Coercion(_) | Value::Uri(_)
    )
}

fn error_value(name: &'static str, message: Option<&Value>) -> Value {
    let message = match message {
        None | Some(Value::Undefined) => String::new(),
        Some(value) => to_string(value),
    };
    Value::Object(shared(ObjectData::error(name, message)))
}
