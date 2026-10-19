//! Purpose: contract tests for IPC dispatcher authorization, payload size, and arg decoding.
//! Scope: repository transaction.
//! Uses:
//! rust/crates/
//! |-- desktop-host/src/bridge/catalog.rs
//! `-- desktop-host/src/bridge/dispatch.rs
//! External:
//! `-- serde_json::json
//! Used by:
//! `-- none, entrypoint
//! Invariants: auth rejections, oversize rejection, args decode for representative channels.
//! Contract tests: this file

use opencode_desktop_host::bridge::catalog::IpcChannel;
use opencode_desktop_host::bridge::dispatch::{DispatchError, Dispatcher, SenderContext};
use serde_json::json;

/// Mock dispatcher for testing dispatch logic.
struct MockDispatcher {
    allow_all: bool,
    max_size: usize,
}

impl MockDispatcher {
    fn new(allow_all: bool, max_size: usize) -> Self {
        Self {
            allow_all,
            max_size,
        }
    }
}

impl Dispatcher for MockDispatcher {
    fn is_authorized(&self, _channel: IpcChannel, _ctx: &SenderContext) -> bool {
        self.allow_all
    }

    fn max_payload_size(&self) -> usize {
        self.max_size
    }
}

fn default_ctx() -> SenderContext {
    SenderContext {
        window_id: Some("window-1".to_string()),
        origin: "app://local".to_string(),
    }
}

#[test]
fn test_decode_success() {
    let dispatcher = MockDispatcher::new(true, 1024);
    let ctx = default_ctx();
    let args = json!({"key": "value"});

    let result = dispatcher.decode("kill-sidecar", &args, &ctx);
    assert!(result.is_ok());
    let (channel, decoded_args) = result.unwrap();
    assert_eq!(channel, IpcChannel::KillSidecar);
    assert_eq!(decoded_args, args);
}

#[test]
fn test_decode_not_found() {
    let dispatcher = MockDispatcher::new(true, 1024);
    let ctx = default_ctx();
    let args = json!({});

    let result = dispatcher.decode("unknown-channel", &args, &ctx);
    assert!(matches!(result, Err(DispatchError::NotFound(_))));
}

#[test]
fn test_decode_unauthorized() {
    let dispatcher = MockDispatcher::new(false, 1024);
    let ctx = default_ctx();
    let args = json!({});

    let result = dispatcher.decode("kill-sidecar", &args, &ctx);
    assert!(matches!(result, Err(DispatchError::Unauthorized(_))));
}

#[test]
fn test_decode_payload_too_large() {
    let dispatcher = MockDispatcher::new(true, 10); // Very small limit
    let ctx = default_ctx();
    let args = json!({"key": "this is a very long string that exceeds the limit"});

    let result = dispatcher.decode("kill-sidecar", &args, &ctx);
    assert!(matches!(result, Err(DispatchError::PayloadTooLarge { .. })));
}

#[test]
fn test_decode_unsupported() {
    let dispatcher = MockDispatcher::new(true, 1024);
    let ctx = default_ctx();
    let args = json!({});

    let result = dispatcher.decode("unsupported", &args, &ctx);
    assert!(matches!(result, Err(DispatchError::Unsupported(_))));
}

#[test]
fn test_decode_representative_channels() {
    let dispatcher = MockDispatcher::new(true, 1024 * 1024);
    let ctx = default_ctx();

    // Test with various argument shapes
    let test_cases = vec![
        ("store-get", json!(["user-settings", "theme"])),
        ("set-display-backend", json!(["wayland"])),
        (
            "open-file-picker",
            json!({"multiple": true, "extensions": ["png", "jpg"]}),
        ),
        ("wsl-servers-install-distro", json!(["Ubuntu"])),
    ];

    for (channel_name, args) in test_cases {
        let result = dispatcher.decode(channel_name, &args, &ctx);
        assert!(result.is_ok(), "Failed to decode channel: {}", channel_name);
        let (channel, decoded_args) = result.unwrap();
        assert_eq!(channel.name(), channel_name);
        assert_eq!(decoded_args, args);
    }
}

#[test]
fn test_max_payload_size_default() {
    let dispatcher = MockDispatcher::new(true, 10 * 1024 * 1024);
    assert_eq!(dispatcher.max_payload_size(), 10 * 1024 * 1024);
}
