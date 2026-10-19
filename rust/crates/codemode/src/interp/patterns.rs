//! Purpose: binding patterns (identifiers, defaults, object/array destructuring with rest).
//! Scope: `runtime.ts` declarePattern (lines 1362-1440).
//! Uses:
//! rust/crates/
//! |-- codemode/src/ast.rs
//! |-- codemode/src/error.rs
//! |-- codemode/src/interp/mod.rs
//! |-- codemode/src/value/convert.rs
//! `-- codemode/src/value/mod.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! `-- codemode/src/interp/mod.rs
//! Invariants: blocked keys (`__proto__`, `constructor`, `prototype`) are never bound.
//! Contract tests: codemode/tests/parity.rs

use super::{Fut, Interp};
use crate::ast::{Loc, Pattern};
use crate::error::{DiagnosticKind, fail, fail_kind};
use crate::value::convert::is_blocked_member;
use crate::value::{ObjectData, Value, lock};

impl Interp {
    pub(super) fn bind_pattern<'a>(
        &'a mut self,
        pattern: &'a Pattern,
        value: Value,
        mutable: bool,
        loc: Loc,
    ) -> Fut<'a, ()> {
        Box::pin(async move {
            match pattern {
                Pattern::Ident(name, _) => self.declare(name, value, mutable, loc),
                Pattern::Default { left, right, .. } => {
                    let resolved = if matches!(value, Value::Undefined) {
                        self.expr(right).await?
                    } else {
                        value
                    };
                    self.bind_pattern(left, resolved, mutable, loc).await
                }
                Pattern::Object {
                    properties,
                    rest,
                    loc: pattern_loc,
                } => {
                    let Value::Object(object) = &value else {
                        return Err(fail_kind(
                            "Object destructuring requires a data object value.",
                            *pattern_loc,
                            DiagnosticKind::InvalidDataValue,
                        ));
                    };
                    let mut consumed = Vec::new();
                    for property in properties {
                        let Some(key) = &property.key else {
                            return Err(fail(
                                "Only named object destructuring properties are supported.",
                                property.loc,
                            ));
                        };
                        if is_blocked_member(key) {
                            return Err(fail(
                                format!("Property '{key}' is not available in CodeMode."),
                                property.key_loc,
                            ));
                        }
                        consumed.push(key.clone());
                        let item = lock(object).get(key).cloned().unwrap_or(Value::Undefined);
                        self.bind_pattern(&property.value, item, mutable, property.loc)
                            .await?;
                    }
                    if let Some(rest) = rest {
                        let remaining: Vec<(String, Value)> = lock(object)
                            .entries()
                            .into_iter()
                            .filter(|(key, _)| !consumed.contains(key) && !is_blocked_member(key))
                            .collect();
                        self.bind_pattern(
                            rest,
                            Value::object(ObjectData::from_pairs(remaining)),
                            mutable,
                            rest.loc(),
                        )
                        .await?;
                    }
                    Ok(())
                }
                Pattern::Array {
                    elements,
                    rest,
                    loc: pattern_loc,
                } => {
                    let Value::Array(array) = &value else {
                        return Err(fail(
                            "Array destructuring requires an array value.",
                            *pattern_loc,
                        ));
                    };
                    let items = lock(array).items.clone();
                    for (index, element) in elements.iter().enumerate() {
                        let Some(element) = element else { continue };
                        let item = items.get(index).cloned().unwrap_or(Value::Undefined);
                        self.bind_pattern(element, item, mutable, *pattern_loc)
                            .await?;
                    }
                    if let Some(rest) = rest {
                        let tail = items
                            .get(elements.len()..)
                            .map(<[Value]>::to_vec)
                            .unwrap_or_default();
                        self.bind_pattern(rest, Value::array(tail), mutable, rest.loc())
                            .await?;
                    }
                    Ok(())
                }
                Pattern::Unsupported { kind, loc } => {
                    Err(fail(format!("Unsupported binding pattern '{kind}'."), *loc))
                }
            }
        })
    }
}
