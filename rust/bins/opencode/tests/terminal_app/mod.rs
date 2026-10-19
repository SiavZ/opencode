//! Purpose: exercise the executable application in a real PTY with isolated durable services.
//! Scope: Unix end-to-end application workflows.
//! Uses:
//! rust/bins/
//! |-- opencode/tests/support/mod.rs
//! `-- opencode/tests/terminal_app/terminal.rs
//! External:
//! `-- none
//! Used by:
//! rust/bins/
//! `-- opencode/tests/commands.rs
//! Invariants: interactive outcomes, not escape sequences alone, prove application behavior.
//! Contract tests: this module

mod provider;
mod questions;
mod terminal;
use super::support::{Home, Provider};
use provider::ToolProvider;
use terminal::Terminal;

#[test]
fn pickers_navigation_and_real_session_lifecycle() {
    let home = Home::new();
    let provider = Provider::start(false);
    home.config(&provider.url);
    let mut terminal = Terminal::start(&home, &[]);
    terminal.wait("Home");
    terminal.send(b"\x10");
    terminal.wait("Commands");
    terminal.send(b"Switch model\r");
    terminal.wait("Select model");
    terminal.send(b"Local test\r");
    terminal.wait("Model: local/local-test");
    terminal.send(b"\x18a");
    terminal.wait("Select agent");
    terminal.send(b"build\r");
    terminal.wait("Agent: build");
    terminal.send(b"hello from full app\r");
    terminal.wait("controlled assistant reply");
    terminal.wait("Enter sends");
    terminal.send(b"/rename\r");
    terminal.wait("Rename session");
    terminal.send(b"PTY retained title\r");
    terminal.wait("Renamed session: PTY retained title");
    terminal.send(b"\x18l");
    terminal.wait("Sessions");
    terminal.send(b"PTY retained title\r");
    terminal.wait("controlled assistant reply");
    terminal.send(b"/fork\r");
    terminal.wait("Fork session");
    terminal.send(b"\r");
    terminal.wait("Session forked");
    terminal.send(b"/delete\r");
    terminal.wait("Delete session permanently?");
    terminal.send(b"\r");
    terminal.wait("Session forked");
    terminal.send(b"/delete\r");
    terminal.wait("Delete session permanently?");
    terminal.send(b"\x1b[B\r");
    terminal.wait("Session deleted");
    terminal.send(b"\x18l");
    terminal.wait("Sessions");
    terminal.send(b"PTY retained title");
    terminal.wait("PTY retained title");
    terminal.send(b"\r");
    terminal.wait("controlled assistant reply");
    terminal.send(b"\x04");
    terminal.restored(true);
    let (_, request) = provider
        .requests
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap();
    assert!(
        request["messages"]
            .to_string()
            .contains("hello from full app")
    );
    assert!(provider.requests.try_recv().is_err());
}

#[test]
fn empty_picker_and_modal_escape_do_not_submit_provider_work() {
    let home = Home::new();
    let provider = Provider::start(false);
    home.config(&provider.url);
    let mut terminal = Terminal::start(&home, &[]);
    terminal.wait("Home");
    terminal.send(b"\x18l");
    terminal.wait("Sessions");
    terminal.send(b"missing-session");
    terminal.wait("No results found");
    terminal.send(b"\x1b");
    terminal.wait("Start a conversation");
    terminal.send(b"\x10");
    terminal.wait("Commands");
    terminal.send(b"Go home\r");
    terminal.wait("Start a conversation");
    terminal.send(b"\x04");
    terminal.restored(true);
    assert!(provider.requests.try_recv().is_err());
}

#[test]
fn unicode_editor_paste_and_history_round_trip_through_real_provider() {
    let home = Home::new();
    let provider = Provider::start(false);
    home.config(&provider.url);
    {
        let mut terminal = Terminal::start(&home, &[]);
        terminal.wait("Home");
        terminal.send("\x1b[200~界e\u{301}👩‍💻\x1b[201~".as_bytes());
        terminal.send(b"\x1b[D\x7f");
        terminal.send("\x1b[200~\nnext\x1b[201~".as_bytes());
        terminal.wait("next");
        terminal.send(b"\r");
        terminal.wait("controlled assistant reply");
        terminal.wait("Enter sends");
        terminal.send(b"\x04");
        terminal.restored(true);
    }
    let expected = "界\nnext👩‍💻";
    let (_, first) = provider
        .requests
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap();
    assert!(
        first["messages"]
            .to_string()
            .contains(&serde_json::to_string(expected).unwrap()),
        "{first}"
    );
    let history =
        std::fs::read_to_string(home.0.path().join("state/opencode/prompt-history.jsonl")).unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(history.trim()).unwrap()["input"],
        expected
    );
    let mut terminal = Terminal::start(&home, &[]);
    terminal.wait("Home");
    terminal.send(b"\x1b[A");
    terminal.wait("next");
    terminal.send(b"\r");
    terminal.wait("controlled assistant reply");
    terminal.wait("Enter sends");
    terminal.send(b"\x04");
    terminal.restored(true);
    let (_, second) = provider
        .requests
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap();
    assert!(
        second["messages"]
            .to_string()
            .contains(&serde_json::to_string(expected).unwrap()),
        "{second}"
    );
}

#[test]
fn navigation_remains_interactive_during_owned_provider_execution() {
    let home = Home::new();
    let provider = Provider::delayed(false, std::time::Duration::from_secs(3));
    home.config(&provider.url);
    let mut terminal = Terminal::start(&home, &[]);
    terminal.wait("Home");
    terminal.send(b"background turn\r");
    provider
        .requests
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    terminal.send(b"\x18n");
    terminal.wait("Home");
    terminal.send(b"\x18l");
    terminal.wait("Sessions");
    terminal.send(b"\r");
    terminal.wait("Running");
    terminal.send(b"\x03");
    terminal.restored(false);
}

#[test]
fn permission_choices_resume_real_tools_and_only_always_survives_restart() {
    for (choice, allowed, persisted) in [
        (b"\x1b".as_slice(), false, false),
        (b"\r".as_slice(), true, false),
        (b"\x1b[C\r".as_slice(), true, true),
    ] {
        let home = Home::new();
        let provider = ToolProvider::start();
        home.config_v2(&provider.url);
        std::fs::write(
            home.0.path().join("project/evidence.txt"),
            "PTY native approved evidence",
        )
        .unwrap();
        let config = home.0.path().join("config/opencode/opencode.json");
        let mut value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&config).unwrap()).unwrap();
        value["permissions"] = serde_json::json!([{"action":"read","resource":"*","effect":"ask"}]);
        std::fs::write(config, value.to_string()).unwrap();
        {
            let mut terminal = Terminal::start(&home, &[]);
            terminal.wait("Home");
            terminal.send(b"read the evidence\r");
            terminal.wait("Permission required: read");
            provider
                .requests
                .recv_timeout(std::time::Duration::from_secs(2))
                .unwrap();
            assert!(
                provider.requests.try_recv().is_err(),
                "provider must wait for a real reply"
            );
            terminal.send(choice);
            if persisted {
                terminal.wait("Always allow this permission?");
                assert!(
                    provider.requests.try_recv().is_err(),
                    "permanent grant requires confirmation"
                );
                terminal.send(b"\r");
            }
            terminal.wait("permission continuation");
            terminal.wait("Enter sends");
            terminal.send(b"\x04");
            terminal.restored(true);
        }
        let continuation = provider
            .requests
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        let result = continuation["messages"]
            .as_array()
            .unwrap()
            .iter()
            .rev()
            .find(|message| message["role"] == "tool")
            .unwrap();
        assert_eq!(
            result.to_string().contains("PTY native approved evidence"),
            allowed,
            "{result}"
        );
        let mut terminal = Terminal::start(&home, &[]);
        terminal.wait("Home");
        terminal.send(b"read after restart\r");
        provider
            .requests
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        if !persisted {
            terminal.wait("Permission required: read");
            terminal.send(b"\r");
        }
        terminal.wait("permission continuation");
        terminal.wait("Enter sends");
        terminal.send(b"\x04");
        terminal.restored(true);
        let continuation = provider
            .requests
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        assert!(
            continuation["messages"]
                .to_string()
                .contains("PTY native approved evidence")
        );
    }
}
