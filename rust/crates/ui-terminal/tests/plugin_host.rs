//! Purpose: contract tests for plugin slot registry public boundary (order, panic isolation, disposal).
//! Scope: integration test suite for opencode-ui-terminal.
//! Uses:
//! `-- none
//! External:
//! |-- opencode_ui_terminal::plugin_host::SlotRegistry
//! |-- ratatui::buffer::Buffer
//! |-- ratatui::layout::Rect
//! `-- ratatui::style::Style
//! Used by:
//! `-- none, entrypoint
//! Invariants: tests exercise public API only; no internal state inspection.
//! Contract tests: this file

use opencode_ui_terminal::plugin_host::SlotRegistry;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;

fn buffer_to_string(buffer: &Buffer) -> String {
    buffer
        .content()
        .iter()
        .map(|c| c.symbol().to_string())
        .collect()
}

#[test]
fn test_registration_order_and_rendering() {
    let registry = SlotRegistry::new();
    let area = Rect::new(0, 0, 20, 5);
    let mut buffer = Buffer::empty(area);

    let _reg1 = registry.register("test_slot".to_string(), "plugin_1".to_string(), |buf| {
        buf.set_string(0, 0, "first", Style::default());
    });

    let _reg2 = registry.register("test_slot".to_string(), "plugin_2".to_string(), |buf| {
        buf.set_string(0, 1, "second", Style::default());
    });

    registry.render("test_slot", &mut buffer, area);

    let output = buffer_to_string(&buffer);
    assert!(output.contains("first"));
    assert!(output.contains("second"));
    assert!(output.find("first").unwrap() < output.find("second").unwrap());
}

#[test]
fn test_panic_isolation_and_error_rendering() {
    let registry = SlotRegistry::new();
    let area = Rect::new(0, 0, 40, 5);
    let mut buffer = Buffer::empty(area);

    let _reg1 = registry.register("panic_slot".to_string(), "bad_plugin".to_string(), |_buf| {
        panic!("intentional test panic");
    });

    let _reg2 = registry.register("panic_slot".to_string(), "good_plugin".to_string(), |buf| {
        buf.set_string(0, 2, "survived", Style::default());
    });

    registry.render("panic_slot", &mut buffer, area);

    let output = buffer_to_string(&buffer);
    assert!(output.contains("[Plugin Error: bad_plugin]"));
    assert!(output.contains("intentional test panic"));
    assert!(output.contains("survived"));
}

#[test]
fn test_disposal_exactly_once() {
    let registry = SlotRegistry::new();
    let area = Rect::new(0, 0, 20, 5);
    let mut buffer = Buffer::empty(area);

    let reg = registry.register(
        "dispose_slot".to_string(),
        "temp_plugin".to_string(),
        |buf| {
            buf.set_string(0, 0, "temp", Style::default());
        },
    );

    registry.render("dispose_slot", &mut buffer, area);
    assert!(buffer_to_string(&buffer).contains("temp"));

    drop(reg);

    let mut buffer2 = Buffer::empty(area);
    registry.render("dispose_slot", &mut buffer2, area);
    assert!(!buffer_to_string(&buffer2).contains("temp"));
}
