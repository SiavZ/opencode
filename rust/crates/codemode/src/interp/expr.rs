//! Purpose: expression evaluation (literals, identifiers, operators, assignment, updates,
//! object/array literals, templates, conditionals, await, new).
//! Scope: `runtime.ts` evaluateExpression and its helpers (lines 1442-1595, 1742-1856, 2800-2905).
//! Uses:
//! rust/crates/
//! |-- codemode/src/ast.rs
//! |-- codemode/src/error.rs
//! |-- codemode/src/interp/members.rs
//! |-- codemode/src/interp/ops.rs
//! |-- codemode/src/stdlib/mod.rs
//! `-- codemode/src/value/convert.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! `-- codemode/src/interp/mod.rs
//! Invariants: object literals reject blocked keys; results crossing operators are bounded data.
//! Contract tests: codemode/tests/parity.rs, codemode/tests/stdlib.rs

use super::ops::{binary, compound, instanceof_value, typeof_value, unary};
use super::{Fut, Interp};
use crate::ast::{Arg, AssignTarget, Expr, Literal, ObjectProp, PropKey};
use crate::error::{
    DiagnosticKind, Eval, data_error, fail, fail_kind, unsupported_feature, unsupported_syntax,
};
use crate::value::convert::{bounded, is_blocked_member, to_number, to_string};
use crate::value::{ObjectData, Value, lock};

impl Interp {
    pub(super) fn expr<'a>(&'a mut self, expression: &'a Expr) -> Fut<'a, Value> {
        Box::pin(async move {
            match expression {
                Expr::Literal(literal, _) => Ok(match literal {
                    Literal::Null => Value::Null,
                    Literal::Bool(flag) => Value::Bool(*flag),
                    Literal::Number(number) => Value::Number(*number),
                    Literal::Str(text) => Value::str(text),
                }),
                Expr::BigInt(_) => Err(data_error("Literal must contain data only.")),
                Expr::Regex {
                    pattern,
                    flags,
                    loc,
                } => crate::stdlib::regexp::construct(
                    &[Value::str(pattern), Value::str(flags)],
                    *loc,
                ),
                Expr::Ident(name, loc) => self.get_identifier(name, *loc),
                Expr::Binary {
                    op,
                    left,
                    right,
                    loc,
                } => {
                    let left = self.expr(left).await?;
                    let right = self.expr(right).await?;
                    if *op == "instanceof" {
                        return Ok(Value::Bool(instanceof_value(&left, &right, *loc)?));
                    }
                    bounded(
                        &binary(op, &left, &right, *loc)?,
                        "Binary expression result",
                    )
                }
                Expr::Logical {
                    op,
                    left,
                    right,
                    loc,
                } => {
                    let value = self.expr(left).await?;
                    match *op {
                        "&&" => {
                            if value.truthy() {
                                self.expr(right).await
                            } else {
                                Ok(value)
                            }
                        }
                        "||" => {
                            if value.truthy() {
                                Ok(value)
                            } else {
                                self.expr(right).await
                            }
                        }
                        "??" => {
                            if value.is_nullish() {
                                self.expr(right).await
                            } else {
                                Ok(value)
                            }
                        }
                        other => Err(fail(
                            format!("Unsupported logical operator '{other}'."),
                            *loc,
                        )),
                    }
                }
                Expr::Unary { op, argument, loc } => self.unary_expr(op, argument, *loc).await,
                Expr::Assign {
                    op,
                    target,
                    right,
                    loc,
                } => self.assign(op, target, right, *loc).await,
                Expr::Update {
                    increment,
                    prefix,
                    target,
                    ..
                } => {
                    let delta = if *increment { 1.0 } else { -1.0 };
                    let prefix = *prefix;
                    match target {
                        AssignTarget::Ident(name, loc) => {
                            let current = to_number(&self.get_identifier(name, *loc)?);
                            let next = current + delta;
                            self.set_identifier(name, Value::Number(next), *loc)?;
                            Ok(Value::Number(if prefix { next } else { current }))
                        }
                        AssignTarget::Member(member) => {
                            self.modify_member(member, |current| {
                                let value = to_number(&current);
                                let next = value + delta;
                                Ok(Some((
                                    Value::Number(next),
                                    Value::Number(if prefix { next } else { value }),
                                )))
                            })
                            .await
                        }
                        AssignTarget::Invalid(loc) => Err(fail(
                            "Update target must be an Identifier or MemberExpression.",
                            *loc,
                        )),
                    }
                }
                Expr::Call {
                    callee,
                    args,
                    optional,
                    loc,
                } => self.call(callee, args, *optional, *loc).await,
                Expr::New { callee, args, loc } => self.new_expr(callee, args, *loc).await,
                Expr::Function(function) => {
                    if function.generator {
                        return Err(unsupported_feature(
                            "Generator functions are not supported in CodeMode.",
                            function.loc,
                        ));
                    }
                    Ok(self.create_function(function))
                }
                Expr::Member(member) => self.read_member(member).await,
                Expr::Chain(inner) => {
                    let value = self.expr(inner).await?;
                    Ok(if matches!(value, Value::ShortCircuit) {
                        Value::Undefined
                    } else {
                        value
                    })
                }
                Expr::Object(properties, _) => self.object_literal(properties).await,
                Expr::Array(elements, _) => {
                    let mut values = Vec::new();
                    for element in elements {
                        match element {
                            None => values.push(Value::Undefined),
                            Some(Arg::Expr(item)) => values.push(self.expr(item).await?),
                            Some(Arg::Spread(item, loc)) => {
                                let spread = self.expr(item).await?;
                                let Some(items) = super::collections::spread_items(&spread) else {
                                    return Err(fail(
                                        "Array spread requires an array, string, Map, or Set in CodeMode.",
                                        *loc,
                                    ));
                                };
                                values.extend(items);
                            }
                        }
                    }
                    Ok(Value::array(values))
                }
                Expr::Template {
                    quasis,
                    expressions,
                    loc,
                } => {
                    let mut output = String::new();
                    for (index, quasi) in quasis.iter().enumerate() {
                        let Some(cooked) = quasi else {
                            return Err(fail("Invalid template literal quasi.", *loc));
                        };
                        output.push_str(cooked);
                        if let Some(expression) = expressions.get(index) {
                            let raw = self.expr(expression).await?;
                            output.push_str(&to_string(&bounded(&raw, "Template interpolation")?));
                        }
                    }
                    Ok(Value::str(output))
                }
                Expr::Conditional {
                    test,
                    consequent,
                    alternate,
                } => {
                    if self.expr(test).await?.truthy() {
                        self.expr(consequent).await
                    } else {
                        self.expr(alternate).await
                    }
                }
                Expr::Await(argument, loc) => {
                    let value = self.expr(argument).await?;
                    match &value {
                        Value::Promise(promise) => self.settle(promise, Some(*loc)).await,
                        _ => Ok(value),
                    }
                }
                Expr::Unsupported { kind, loc } => Err(unsupported_syntax(kind, *loc)),
            }
        })
    }

    async fn unary_expr(&mut self, op: &str, argument: &Expr, loc: crate::ast::Loc) -> Eval<Value> {
        if op == "typeof"
            && let Expr::Ident(name, _) = argument
            && self.resolve_binding(name).is_none()
        {
            return Ok(Value::str("undefined"));
        }
        if op == "delete" || op == "void" {
            return Err(fail(format!("Unsupported unary operator '{op}'."), loc));
        }
        let value = self.expr(argument).await?;
        match op {
            "typeof" => Ok(Value::str(typeof_value(&value))),
            "!" => Ok(Value::Bool(!value.truthy())),
            _ => bounded(&unary(op, &value, loc)?, "Unary expression result"),
        }
    }

    async fn assign(
        &mut self,
        op: &str,
        target: &AssignTarget,
        right: &Expr,
        loc: crate::ast::Loc,
    ) -> Eval<Value> {
        if matches!(op, "??=" | "||=" | "&&=") {
            let should = |current: &Value| match op {
                "??=" => current.is_nullish(),
                "||=" => !current.truthy(),
                _ => current.truthy(),
            };
            return match target {
                AssignTarget::Ident(name, name_loc) => {
                    let current = self.get_identifier(name, *name_loc)?;
                    if !should(&current) {
                        return Ok(current);
                    }
                    let value = self.expr(right).await?;
                    self.set_identifier(name, value, *name_loc)
                }
                AssignTarget::Member(member) => {
                    let reference = self.member_reference(member).await?;
                    let current = self.slot_current(&reference, member)?;
                    if !should(&current) {
                        return Ok(current);
                    }
                    let value = self.expr(right).await?;
                    self.slot_write(reference, member, value.clone())?;
                    Ok(value)
                }
                AssignTarget::Invalid(loc) => Err(fail(
                    "Assignment target must be an Identifier or MemberExpression.",
                    *loc,
                )),
            };
        }
        let value = self.expr(right).await?;
        match target {
            AssignTarget::Ident(name, name_loc) => {
                if op == "=" {
                    return self.set_identifier(name, value, *name_loc);
                }
                let current = self.get_identifier(name, *name_loc)?;
                let next = bounded(&compound(op, &current, &value, loc)?, "Assignment result")?;
                self.set_identifier(name, next, *name_loc)
            }
            AssignTarget::Member(member) => {
                if op == "=" {
                    return self
                        .modify_member(member, |_| Ok(Some((value.clone(), value.clone()))))
                        .await;
                }
                self.modify_member(member, |current| {
                    let next = bounded(&compound(op, &current, &value, loc)?, "Assignment result")?;
                    Ok(Some((next.clone(), next)))
                })
                .await
            }
            AssignTarget::Invalid(loc) => Err(fail(
                "Assignment target must be an Identifier or MemberExpression.",
                *loc,
            )),
        }
    }

    async fn object_literal(&mut self, properties: &[ObjectProp]) -> Eval<Value> {
        let mut object = ObjectData::new();
        for property in properties {
            match property {
                ObjectProp::Spread(argument, loc) => {
                    let spread = self.expr(argument).await?;
                    match &spread {
                        Value::Undefined | Value::Null => {}
                        value if value.is_sandbox() => {}
                        Value::Object(source) => {
                            for (key, item) in lock(source).entries() {
                                if is_blocked_member(&key) {
                                    return Err(fail(
                                        format!("Property '{key}' is not available in CodeMode."),
                                        *loc,
                                    ));
                                }
                                object.set(key, item);
                            }
                        }
                        _ => {
                            return Err(fail_kind(
                                "Object spread requires a data object in CodeMode.",
                                *loc,
                                DiagnosticKind::InvalidDataValue,
                            ));
                        }
                    }
                }
                ObjectProp::Accessor(loc) => {
                    return Err(fail("Only init object properties are supported.", *loc));
                }
                ObjectProp::Init {
                    key,
                    key_loc,
                    value,
                } => {
                    let key = match key {
                        PropKey::Ident(name) => name.clone(),
                        PropKey::Literal(Literal::Str(text)) => text.clone(),
                        PropKey::Literal(Literal::Number(number)) => {
                            crate::value::number::format(*number)
                        }
                        PropKey::Literal(_) => {
                            return Err(fail("Property key must be a string or number.", *key_loc));
                        }
                        PropKey::Computed(expression) => {
                            let computed = self.expr(expression).await?;
                            super::members::property_key(&computed, *key_loc)?.into_string()
                        }
                    };
                    if is_blocked_member(&key) {
                        return Err(fail(
                            format!("Property '{key}' is not available in CodeMode."),
                            *key_loc,
                        ));
                    }
                    let item = self.expr(value).await?;
                    object.set(key, item);
                }
            }
        }
        Ok(Value::object(object))
    }

    pub(super) async fn eval_args(&mut self, args: &[Arg]) -> Eval<Vec<Value>> {
        let mut values = Vec::with_capacity(args.len());
        for arg in args {
            match arg {
                Arg::Expr(expression) => values.push(self.expr(expression).await?),
                Arg::Spread(expression, loc) => {
                    let spread = self.expr(expression).await?;
                    let Some(items) = super::collections::spread_items(&spread) else {
                        return Err(fail(
                            "Spread arguments require an array, string, Map, or Set in CodeMode.",
                            *loc,
                        ));
                    };
                    values.extend(items);
                }
            }
        }
        Ok(values)
    }
}
