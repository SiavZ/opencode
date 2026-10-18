//! Purpose: tests for identity keys, ownership generations and the prompt submit state machine.
//! Ports `packages/app/src/utils/session-route.ts` behavior, `session-ownership.ts` semantics and
//! `components/prompt-input/submit.ts:58-208` rollback. Fixture-based.
//! Scope: integration tests through `identity` and `actions`.
//! Uses:
//! rust/crates/
//! |-- view-model/src/actions.rs
//! `-- view-model/src/identity.rs
//! External:
//! `-- none
//! Used by:
//! `-- none, entrypoint
//! Invariants: expected encodings come from the baseline base64url-no-padding rule.
//! Contract tests: self

use opencode_view_model::actions::{
    DraftSnapshot, Effect, Phase, PromptSubmit, Submission, parse_command,
};
use opencode_view_model::identity::{
    DraftKey, Ownership, ServerConnection, ServerKey, SessionKey, Target, decode_segment,
    encode_segment, legacy_session_href, legacy_session_server, require_server_key, session_href,
};

#[test]
fn server_keys_match_connection_kinds() {
    assert_eq!(
        ServerConnection::Http {
            url: "http://localhost:4096".into()
        }
        .key()
        .0,
        "http://localhost:4096"
    );
    assert_eq!(ServerConnection::Sidecar.key().0, "sidecar");
    assert_eq!(
        ServerConnection::SidecarWsl {
            distro: "Ubuntu".into()
        }
        .key()
        .0,
        "wsl:Ubuntu"
    );
    assert_eq!(
        ServerConnection::Ssh { host: "box".into() }.key().0,
        "ssh:box"
    );
}

#[test]
fn route_segments_round_trip_and_reject_noncanonical() {
    // btoa("http://localhost:4096") with +/ -> -_ and no padding.
    assert_eq!(
        encode_segment("http://localhost:4096"),
        "aHR0cDovL2xvY2FsaG9zdDo0MDk2"
    );
    assert_eq!(encode_segment("/repo/ü?"), "L3JlcG8vw7w_");
    assert_eq!(decode_segment("L3JlcG8vw7w_").as_deref(), Some("/repo/ü?"));
    let key = ServerKey("http://localhost:4096".into());
    assert_eq!(
        session_href(&key, "ses_1"),
        "/server/aHR0cDovL2xvY2FsaG9zdDo0MDk2/session/ses_1"
    );
    assert_eq!(require_server_key("aHR0cDovL2xvY2FsaG9zdDo0MDk2"), Ok(key));
    assert!(
        require_server_key("aHR0cDovL2xvY2FsaG9zdDo0MDk2==").is_err(),
        "padding is not canonical"
    );
    assert!(require_server_key("!!").is_err());
    assert!(require_server_key("").is_err());
    assert_eq!(
        legacy_session_href("/repo", "ses_1"),
        "/L3JlcG8/session/ses_1"
    );
}

#[test]
fn legacy_session_server_prefers_active_then_unique() {
    let a = ServerKey("a".into());
    let b = ServerKey("b".into());
    let tabs = vec![
        (a.clone(), "s1".into()),
        (b.clone(), "s1".into()),
        (b.clone(), "s2".into()),
    ];
    assert_eq!(legacy_session_server(&tabs, "s1", &a), a);
    assert_eq!(legacy_session_server(&tabs, "s2", &a), b);
    assert_eq!(legacy_session_server(&tabs, "s3", &a), a);
}

#[test]
fn ownership_generation_invalidates_on_session_change_and_dispose() {
    let mut own = Ownership::new("ses_1");
    let c = own.capture();
    assert_eq!(c.key, "0:ses_1");
    own.transition("ses_1");
    assert!(own.is_current(&c));
    own.transition("ses_2");
    assert!(!own.is_current(&c));
    assert_eq!(own.run(&c, || 1), None);
    let c2 = own.capture();
    assert_eq!(c2.key, "1:ses_2");
    assert_eq!(own.run(&c2, || 1), Some(1));
    own.dispose();
    assert!(!own.is_current(&c2));
}

#[test]
fn draft_remount_key_ignores_draft_id() {
    let d = DraftKey {
        server: ServerKey("s".into()),
        directory: "/r".into(),
        draft_id: "x".into(),
    };
    assert_eq!(d.remount_key(), "s\0/r");
}

fn target() -> Target {
    Target::Session(SessionKey {
        server: ServerKey("s".into()),
        session_id: "ses_1".into(),
    })
}

fn draft(text: &str) -> DraftSnapshot {
    DraftSnapshot {
        text: text.into(),
        agent: "build".into(),
        model_id: "m".into(),
        provider_id: "p".into(),
        variant: None,
        attachments: vec![],
    }
}

#[test]
fn parses_only_known_commands() {
    assert_eq!(
        parse_command("/review a b", &["review"]),
        Some(("review".into(), "a b".into()))
    );
    assert_eq!(
        parse_command("/review", &["review"]),
        Some(("review".into(), String::new()))
    );
    assert_eq!(parse_command("/nope x", &["review"]), None);
    assert_eq!(parse_command("hello /review", &["review"]), None);
}

#[test]
fn prompt_failure_rolls_back_to_captured_target() {
    let (mut s, start) = PromptSubmit::begin(target(), draft("hi"), &[], "msg_1".into(), true);
    assert_eq!(
        start,
        vec![
            Effect::SetBusy(target()),
            Effect::AddOptimistic {
                target: target(),
                message_id: "msg_1".into()
            },
            Effect::ClearDraft(target()),
        ]
    );
    assert_eq!(
        s.before_resolved(true),
        vec![Effect::Send {
            target: target(),
            submission: Submission::Prompt {
                message_id: "msg_1".into()
            }
        }]
    );
    assert_eq!(
        s.finished(false),
        vec![
            Effect::SetIdle(target()),
            Effect::RemoveOptimistic {
                target: target(),
                message_id: "msg_1".into()
            },
            Effect::RestoreDraft {
                target: target(),
                draft: draft("hi")
            },
        ]
    );
    assert_eq!(s.phase, Phase::Failed);
    assert!(s.finished(false).is_empty(), "terminal phases emit nothing");
}

#[test]
fn cancelled_before_hook_and_command_have_no_optimistic_message() {
    let (mut s, start) = PromptSubmit::begin(
        target(),
        draft("/review x"),
        &["review"],
        "msg_1".into(),
        false,
    );
    assert_eq!(start, vec![Effect::ClearDraft(target())]);
    assert_eq!(
        s.before_resolved(false),
        vec![Effect::RestoreDraft {
            target: target(),
            draft: draft("/review x")
        }]
    );
    assert_eq!(s.phase, Phase::Cancelled);
}
