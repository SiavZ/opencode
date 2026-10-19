//! Purpose: reject array and object insertions that would introduce a circular data value.
//! Scope: identity-based traversal shared by member assignments and collection mutations.
//! Uses:
//! rust/crates/
//! |-- codemode/src/ast.rs
//! |-- codemode/src/error.rs
//! `-- codemode/src/value/mod.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! `-- codemode/src/interp/members.rs
//! Invariants: runtime references remain opaque; traversal rejects a path back to the target container and preserves diagnostic kind, label and source location.
//! Contract tests: codemode/tests/parity.rs exercises circular insertion through the real interpreter.

use crate::ast::Loc;
use crate::error::{DiagnosticKind, Eval, fail_kind};
use crate::value::{Value, lock};

/// Rejects inserting a value that (transitively) contains `container`.
pub(crate) fn reject_circular_insertion(
    container: &Value,
    value: &Value,
    label: &str,
    loc: Loc,
) -> Eval<()> {
    fn walk(container: usize, value: &Value, seen: &mut Vec<usize>) -> bool {
        let Some(id) = value.identity() else {
            return false;
        };
        if id == container {
            return true;
        }
        if value.is_runtime_reference() || seen.contains(&id) {
            return false;
        }
        seen.push(id);
        let items: Vec<Value> = match value {
            Value::Array(array) => lock(array).items.clone(),
            Value::Object(object) => lock(object).props.values().cloned().collect(),
            _ => Vec::new(),
        };
        let found = items.iter().any(|item| walk(container, item, seen));
        seen.pop();
        found
    }
    let Some(container) = container.identity() else {
        return Ok(());
    };
    if walk(container, value, &mut Vec::new()) {
        return Err(fail_kind(
            format!("{label} contains a circular value."),
            loc,
            DiagnosticKind::InvalidDataValue,
        ));
    }
    Ok(())
}
