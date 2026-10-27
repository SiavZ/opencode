//! Purpose: exercise strict CLI startup validation of both supported config generations.
//! Scope: disposable global/project documents and actual executable startup.
//! Uses: opencode/tests/support/mod.rs
//! Used by: opencode/tests/admin/mod.rs
//! Invariants: invalid known fields and malformed JSON fail rather than being silently skipped.
//! Contract tests: self

use super::super::support::{Home, output};

#[test]
fn malformed_and_invalid_v1_v2_documents_fail_in_global_and_project_locations() {
    for relative in ["config/opencode/opencode.json", "project/opencode.json"] {
        for contents in [
            "{ invalid",
            r#"{"provider":{"broken":false}}"#,
            r#"{"providers":{"broken":false}}"#,
            r#"{"permissions":[{"action":"read","resource":"*","effect":"invalid"}]}"#,
        ] {
            let home = Home::new();
            std::fs::write(home.0.path().join(relative), contents).unwrap();
            let result = output(home.command(&["models"]), None);
            assert_eq!(result.status.code(), Some(1), "{relative}: {contents}");
            assert!(!result.stderr.is_empty());
            assert!(!home.0.path().join("data/opencode/opencode.db").exists());
        }
    }
}
