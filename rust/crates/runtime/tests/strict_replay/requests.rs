//! Purpose: resolve captured request environment tokens without weakening response matching.
//! Scope: strict production replay request preparation only.
//! Uses:
//! rust/crates/
//! |-- server/tests/support/replay/matcher.rs
//! `-- server/tests/support/replay/mod.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! `-- runtime/tests/strict_replay.rs
//! Invariants: Git response wildcards and original fixture files remain unchanged.
//! Contract tests: this file

use super::replay::{Case, matcher::Bindings};

pub(super) fn bind(case: &Case, bindings: &Bindings, project: &str) -> Case {
    // Capture normalization collapses every Git hash to {SHA}. Only request hashes identify
    // the isolated root commit. Response hashes can also be blob hashes and must stay wildcards.
    let request = Bindings {
        ids: bindings.ids.clone(),
        literals: vec![("{SHA}".into(), project.into())],
    };
    let mut execution = case.clone();
    execution.request.path = request.fill(&case.request.path);
    execution.request.query = case
        .request
        .query
        .iter()
        .map(|(key, value)| (key.clone(), request.fill(value)))
        .collect();
    execution.request.headers = case
        .request
        .headers
        .iter()
        .map(|(key, value)| (key.clone(), request.fill(value)))
        .collect();
    execution.request.body = case
        .request
        .body
        .as_ref()
        .map(|body| request.fill_value(body));
    execution.request.raw_body = case
        .request
        .raw_body
        .as_ref()
        .map(|body| request.fill(body));
    execution
}

#[test]
fn git_request_binding_never_rewrites_expected_response_hashes() {
    let cases = super::replay::load();
    let case = cases
        .iter()
        .find(|case| case.id == "project.update#ok")
        .unwrap();
    let actual = bind(
        case,
        &Bindings::default(),
        "0123456789abcdef0123456789abcdef01234567",
    );
    assert_eq!(
        actual.request.path,
        "/project/0123456789abcdef0123456789abcdef01234567"
    );
    assert_eq!(actual.response.body, case.response.body);
    assert_eq!(actual.response.headers, case.response.headers);
    assert_eq!(actual.response.status, case.response.status);
    assert_eq!(case.request.path, "/project/{SHA}");
}

#[test]
fn dependent_ids_are_bound_or_explicitly_reported_unbound() {
    let cases = super::replay::load();
    let case = cases
        .iter()
        .find(|case| case.id == "session.message#ok")
        .unwrap();
    let mut bindings = Bindings::default();
    bindings.ids.insert("{ses_1}".into(), "ses_real".into());
    let unresolved = bind(case, &bindings, "unused");
    assert!(
        unresolved
            .request
            .path
            .starts_with("/session/ses_real/message/msg_unbound")
    );
    bindings.ids.insert("{msg_1}".into(), "msg_real".into());
    let resolved = bind(case, &bindings, "unused");
    assert_eq!(resolved.request.path, "/session/ses_real/message/msg_real");
    assert_eq!(resolved.response.body, case.response.body);
}
