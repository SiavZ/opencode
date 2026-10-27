//! Purpose: prove real question builtin waiters resume with terminal-supplied answers.
//! Scope: executable PTY and loopback provider boundary.
//! Uses: terminal_app provider and terminal fixtures; support Home.
//! Used by: commands integration test.
//! Invariants: no provider continuation until review submits; dismissal settles rather than hangs.
//! Contract tests: this module.

use super::super::support::Home;
use super::provider::ToolProvider;
use super::terminal::Terminal;
use serde_json::json;
use std::time::Duration;

#[test]
fn multiple_choices_custom_unicode_and_review_reach_actual_tool_continuation() {
    let home = Home::new();
    let provider = ToolProvider::with_tool(
        "question",
        json!({"questions":[
            {"header":"Features","question":"Choose features","multiple":true,"options":[{"label":"Audit","description":"Keep evidence"},{"label":"History","description":"Keep prompts"}]},
            {"header":"Name","question":"Choose a name","options":[{"label":"Default","description":"Suggested"}]}
        ]}),
    );
    home.config_v2(&provider.url);
    let mut terminal = Terminal::start(&home, &[]);
    terminal.wait("Home");
    terminal.send(b"ask the questions\r");
    terminal.wait("Choose features");
    provider
        .requests
        .recv_timeout(Duration::from_secs(2))
        .unwrap();
    terminal.send(b"1j\r\t");
    terminal.wait("Choose a name");
    terminal.send(b"2");
    terminal.send("\x1b[200~名前\x1b[201~\r".as_bytes());
    terminal.wait("Review answers");
    terminal.wait("Audit, History");
    terminal.wait("名前");
    terminal.send(b"lh");
    terminal.wait("Review answers");
    terminal.send(b"h2");
    terminal.wait_absent("Review answers");
    terminal.wait("Other: 名前");
    terminal.send("\x1b[200~修正\x1b[201~\r".as_bytes());
    terminal.wait("Review answers");
    terminal.wait("名前修正");
    assert!(
        provider.requests.try_recv().is_err(),
        "review must still own the real waiter"
    );
    terminal.send(b"\r");
    terminal.wait("permission continuation");
    terminal.wait("Enter sends");
    terminal.send(b"\x04");
    terminal.restored(true);
    let continuation = provider
        .requests
        .recv_timeout(Duration::from_secs(2))
        .unwrap();
    let tool = continuation["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["role"] == "tool")
        .unwrap();
    assert!(tool.to_string().contains("Audit, History"), "{tool}");
    assert!(tool.to_string().contains("名前修正"), "{tool}");
}

#[test]
fn dismissing_a_question_settles_the_waiter_and_restores_terminal() {
    let home = Home::new();
    let provider = ToolProvider::with_tool(
        "question",
        json!({"questions":[{"header":"Dismiss","question":"Choose or dismiss","options":[{"label":"Yes","description":"Proceed"}]}]}),
    );
    home.config_v2(&provider.url);
    let mut terminal = Terminal::start(&home, &[]);
    terminal.wait("Home");
    terminal.send(b"ask and dismiss\r");
    terminal.wait("Choose or dismiss");
    provider
        .requests
        .recv_timeout(Duration::from_secs(2))
        .unwrap();
    terminal.send(b"\x04");
    terminal.wait("permission continuation");
    terminal.wait("Enter sends");
    terminal.send(b"\x04");
    terminal.restored(true);
    let continuation = provider
        .requests
        .recv_timeout(Duration::from_secs(2))
        .unwrap();
    assert!(
        continuation["messages"].to_string().contains("dismissed"),
        "{continuation}"
    );
}
