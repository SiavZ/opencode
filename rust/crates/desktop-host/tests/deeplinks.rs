//! Purpose: fixture tests for deep link validation, pending queue, argv extraction and routing.
//! Scope: all tests are in-process, no real windows or IPC.
//! Uses:
//! rust/crates/
//! `-- desktop-host/src/deeplinks.rs
//! External:
//! `-- parking_lot::Mutex
//! Used by:
//! `-- none, entrypoint
//! Invariants: tests exercise the real public API through fixture sinks.
//! Contract tests: this file.

use opencode_desktop_host::deeplinks::{
    DeepLinkError, DeepLinkManager, DeepLinkSink, DeepLinkTarget, parse_deep_link,
};
use parking_lot::Mutex;
use std::collections::HashMap;

#[derive(Default)]
struct FixtureSink {
    focused: Mutex<Option<String>>,
    recent: Mutex<Option<String>>,
    delivered: Mutex<HashMap<String, Vec<Vec<String>>>>,
}

impl FixtureSink {
    fn new() -> Self {
        Self::default()
    }

    fn with_focused(self, id: &str) -> Self {
        *self.focused.lock() = Some(id.to_string());
        self
    }

    fn with_recent(self, id: &str) -> Self {
        *self.recent.lock() = Some(id.to_string());
        self
    }

    #[allow(dead_code)]
    fn deliveries(&self, window_id: &str) -> Vec<Vec<String>> {
        self.delivered
            .lock()
            .get(window_id)
            .cloned()
            .unwrap_or_default()
    }
}

impl DeepLinkSink for FixtureSink {
    fn send_deep_links(&self, window_id: &str, urls: &[String]) {
        self.delivered
            .lock()
            .entry(window_id.to_string())
            .or_default()
            .push(urls.to_vec());
    }

    fn get_focused_window(&self) -> Option<String> {
        self.focused.lock().clone()
    }

    fn get_most_recent_window(&self) -> Option<String> {
        self.recent.lock().clone()
    }
}

fn url(s: &str) -> String {
    s.to_string()
}

mod parse {
    use super::*;

    #[test]
    fn parses_open_project_deep_link() {
        assert_eq!(
            parse_deep_link("opencode://open-project?directory=/tmp/demo").unwrap(),
            DeepLinkTarget::OpenProject {
                directory: "/tmp/demo".to_string(),
            }
        );
    }

    #[test]
    fn rejects_non_opencode_scheme() {
        assert!(matches!(
            parse_deep_link("https://example.com"),
            Err(DeepLinkError::InvalidScheme)
        ));
    }

    #[test]
    fn rejects_unknown_hostname() {
        assert!(matches!(
            parse_deep_link("opencode://other?directory=/tmp/demo"),
            Err(DeepLinkError::UnsupportedHostname(_))
        ));
    }

    #[test]
    fn rejects_malformed_url_without_panic() {
        let result = parse_deep_link("opencode://open-project/%E0%A4%A%");
        assert!(result.is_err());
    }

    #[test]
    fn rejects_open_project_without_directory() {
        assert!(matches!(
            parse_deep_link("opencode://open-project"),
            Err(DeepLinkError::MissingDirectory)
        ));
    }

    #[test]
    fn rejects_open_project_with_empty_directory() {
        assert!(matches!(
            parse_deep_link("opencode://open-project?directory="),
            Err(DeepLinkError::MissingDirectory)
        ));
    }

    #[test]
    fn parses_new_session_with_prompt() {
        assert_eq!(
            parse_deep_link("opencode://new-session?directory=/tmp/demo&prompt=hello%20world")
                .unwrap(),
            DeepLinkTarget::NewSession {
                directory: "/tmp/demo".to_string(),
                prompt: Some("hello world".to_string()),
            }
        );
    }

    #[test]
    fn parses_new_session_without_prompt() {
        assert_eq!(
            parse_deep_link("opencode://new-session?directory=/tmp/demo").unwrap(),
            DeepLinkTarget::NewSession {
                directory: "/tmp/demo".to_string(),
                prompt: None,
            }
        );
    }

    #[test]
    fn rejects_new_session_without_directory() {
        assert!(matches!(
            parse_deep_link("opencode://new-session"),
            Err(DeepLinkError::MissingDirectory)
        ));
    }

    #[test]
    fn rejects_new_session_with_empty_directory() {
        assert!(matches!(
            parse_deep_link("opencode://new-session?directory="),
            Err(DeepLinkError::MissingDirectory)
        ));
    }
}

mod argv {
    use super::*;

    #[test]
    fn extracts_opencode_urls_from_argv() {
        let argv = vec![
            "/usr/bin/opencode".to_string(),
            "--flag".to_string(),
            "opencode://open-project?directory=/a".to_string(),
            "not-a-url".to_string(),
            "opencode://new-session?directory=/b".to_string(),
        ];
        let urls = DeepLinkManager::<FixtureSink>::extract_from_argv(&argv);
        assert_eq!(
            urls,
            vec![
                "opencode://open-project?directory=/a".to_string(),
                "opencode://new-session?directory=/b".to_string(),
            ]
        );
    }

    #[test]
    fn returns_empty_for_no_matches() {
        let argv = vec!["/usr/bin/opencode".to_string(), "--flag".to_string()];
        assert!(DeepLinkManager::<FixtureSink>::extract_from_argv(&argv).is_empty());
    }

    #[test]
    fn preserves_order_and_duplicates() {
        let argv = vec![
            "opencode://a".to_string(),
            "opencode://a".to_string(),
            "opencode://b".to_string(),
        ];
        let urls = DeepLinkManager::<FixtureSink>::extract_from_argv(&argv);
        assert_eq!(urls, vec!["opencode://a", "opencode://a", "opencode://b"]);
    }
}

mod emit {
    use super::*;

    #[test]
    fn empty_urls_are_noop() {
        let sink = FixtureSink::new().with_focused("win1");
        let mgr = DeepLinkManager::new(sink);
        mgr.emit_deep_links(vec![]);
        assert!(mgr.consume_initial().is_empty());
    }

    #[test]
    fn appends_to_pending_and_delivers_to_focused() {
        let sink = FixtureSink::new().with_focused("win1");
        let mgr = DeepLinkManager::new(sink);
        mgr.emit_deep_links(vec![url("opencode://open-project?directory=/a")]);
        assert_eq!(
            mgr.consume_initial(),
            vec!["opencode://open-project?directory=/a"]
        );
    }

    #[test]
    fn falls_back_to_most_recent_when_no_focus() {
        let sink = FixtureSink::new().with_recent("win2");
        let mgr = DeepLinkManager::new(sink);
        mgr.emit_deep_links(vec![url("opencode://open-project?directory=/a")]);
        assert_eq!(
            mgr.consume_initial(),
            vec!["opencode://open-project?directory=/a"]
        );
    }

    #[test]
    fn pending_kept_when_no_window() {
        let sink = FixtureSink::new();
        let mgr = DeepLinkManager::new(sink);
        mgr.emit_deep_links(vec![url("opencode://open-project?directory=/a")]);
        assert_eq!(
            mgr.consume_initial(),
            vec!["opencode://open-project?directory=/a"]
        );
    }

    #[test]
    fn preserves_baseline_duplicate_and_ordering() {
        let sink = FixtureSink::new().with_focused("win1");
        let mgr = DeepLinkManager::new(sink);
        mgr.emit_deep_links(vec![url("opencode://a")]);
        mgr.emit_deep_links(vec![url("opencode://a"), url("opencode://b")]);
        let pending = mgr.consume_initial();
        assert_eq!(
            pending,
            vec!["opencode://a", "opencode://a", "opencode://b"]
        );
    }

    #[test]
    fn consume_initial_is_splice_semantics() {
        let sink = FixtureSink::new();
        let mgr = DeepLinkManager::new(sink);
        mgr.emit_deep_links(vec![url("opencode://a")]);
        let first = mgr.consume_initial();
        let second = mgr.consume_initial();
        assert_eq!(first, vec!["opencode://a"]);
        assert!(second.is_empty());
    }
}
