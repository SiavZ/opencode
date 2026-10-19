//! Purpose: plugin slot registry and rendering boundary for TUI plugins.
//! Scope: app lifetime, rendering to ratatui Buffer.
//! Uses:
//! `-- none
//! External:
//! |-- ratatui::buffer::Buffer
//! |-- ratatui::layout::Rect
//! |-- ratatui::prelude::Widget
//! |-- ratatui::style::Color
//! |-- ratatui::style::Style
//! |-- ratatui::text::Line
//! |-- ratatui::text::Span
//! `-- ratatui::text::Text
//! Used by:
//! rust/crates/
//! |-- ui-terminal/src/lib.rs
//! `-- ui-terminal/tests/plugin_host.rs
//! Invariants: each registration is disposed exactly once; panics in render callbacks are isolated and rendered as errors.
//! Contract tests: ui-terminal/tests/plugin_host.rs

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::prelude::Widget;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span, Text};
use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex};

pub type SlotName = String;
pub type PluginId = String;

struct RegisteredPlugin {
    id: PluginId,
    render_fn: Box<dyn Fn(&mut Buffer) + Send + Sync>,
}

pub struct SlotRegistry {
    slots: Mutex<HashMap<SlotName, Vec<RegisteredPlugin>>>,
}

pub struct Registration {
    id: PluginId,
    slot_name: SlotName,
    registry: Arc<SlotRegistry>,
}

impl Drop for Registration {
    fn drop(&mut self) {
        if let Ok(mut slots) = self.registry.slots.lock()
            && let Some(plugins) = slots.get_mut(&self.slot_name)
        {
            plugins.retain(|p| p.id != self.id);
        }
    }
}

impl SlotRegistry {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            slots: Mutex::new(HashMap::new()),
        })
    }

    pub fn register<F>(
        self: &Arc<Self>,
        slot_name: SlotName,
        plugin_id: PluginId,
        render_fn: F,
    ) -> Registration
    where
        F: Fn(&mut Buffer) + Send + Sync + 'static,
    {
        let mut slots = self.slots.lock().unwrap();
        let plugins = slots.entry(slot_name.clone()).or_default();
        plugins.push(RegisteredPlugin {
            id: plugin_id.clone(),
            render_fn: Box::new(render_fn),
        });

        Registration {
            id: plugin_id,
            slot_name,
            registry: Arc::clone(self),
        }
    }

    pub fn render(&self, slot_name: &str, buffer: &mut Buffer, area: Rect) {
        let slots = self.slots.lock().unwrap();
        if let Some(plugins) = slots.get(slot_name) {
            for plugin in plugins {
                let result = catch_unwind(AssertUnwindSafe(|| {
                    (plugin.render_fn)(buffer);
                }));

                if let Err(err) = result {
                    let panic_msg = if let Some(s) = err.downcast_ref::<&str>() {
                        s.to_string()
                    } else if let Some(s) = err.downcast_ref::<String>() {
                        s.clone()
                    } else {
                        "Unknown panic".to_string()
                    };

                    let error_text = Text::from(vec![
                        Line::from(Span::styled(
                            format!("[Plugin Error: {}]", plugin.id),
                            Style::default().fg(Color::Red),
                        )),
                        Line::from(Span::styled(panic_msg, Style::default().fg(Color::Yellow))),
                    ]);
                    error_text.render(area, buffer);
                }
            }
        }
    }
}
