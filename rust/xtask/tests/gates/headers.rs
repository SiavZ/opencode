//! Purpose: cover module-resolution boundaries in the real header generator.
//! Scope: isolated source trees, no real workspace mutations.
//! Uses:
//! rust/xtask/
//! |-- src/headers.rs
//! `-- tests/gates.rs
//! External:
//! `-- none
//! Used by:
//! rust/xtask/
//! `-- tests/gates.rs
//! Invariants: explicit paths and shared integration modules preserve their actual crate roots.
//! Contract tests: this file

use super::{HEADER, tree, write};
use xtask::headers;

#[test]
fn explicit_sibling_paths_inline_super_and_bare_local_imports_resolve() {
    let temp = tree();
    write(
        temp.path(),
        "crates/a/src/lib.rs",
        &format!("{HEADER}mod a; mod b; use a::Item; pub struct Root;"),
    );
    write(
        temp.path(),
        "crates/a/src/a.rs",
        &format!("{HEADER}#[path=\"a_impl.rs\"] mod implementation; pub struct Item;"),
    );
    write(
        temp.path(),
        "crates/a/src/a_impl.rs",
        &format!("{HEADER}use super::super::Root;"),
    );
    write(
        temp.path(),
        "crates/a/src/b.rs",
        &format!("{HEADER}mod nested {{ use super::super::a::Item; }}"),
    );
    assert!(headers::validate(temp.path(), true).unwrap().is_empty());
    assert!(headers::validate(temp.path(), false).unwrap().is_empty());
    let b = std::fs::read_to_string(temp.path().join("crates/a/src/b.rs")).unwrap();
    assert!(b.contains("`-- a/src/a.rs"), "{b}");
    let a = std::fs::read_to_string(temp.path().join("crates/a/src/a.rs")).unwrap();
    assert!(a.contains("`-- a/src/a_impl.rs"), "{a}");
    assert!(!a.contains("External:\n//! `-- a::"));
}

#[test]
fn shared_test_support_has_each_integration_crate_context() {
    let temp = tree();
    for name in ["one", "two"] {
        write(
            temp.path(),
            &format!("crates/a/tests/{name}.rs"),
            &format!("{HEADER}mod support; pub struct Marker;"),
        );
    }
    write(
        temp.path(),
        "crates/a/tests/support/mod.rs",
        &format!("{HEADER}use crate::Marker;"),
    );
    assert!(headers::validate(temp.path(), true).unwrap().is_empty());
    assert!(headers::validate(temp.path(), false).unwrap().is_empty());
    let support =
        std::fs::read_to_string(temp.path().join("crates/a/tests/support/mod.rs")).unwrap();
    assert!(support.contains("a/tests/one.rs"), "{support}");
    assert!(support.contains("a/tests/two.rs"), "{support}");
}
