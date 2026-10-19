//! Purpose: keybinding configuration parsing and key matching (leader, chords) ported from packages/tui/src/config/keybind.ts.
//! Scope: pure values, parsing, and default definitions.
//! Uses:
//! rust/crates/
//! `-- ui-terminal/src/input.rs
//! External:
//! |-- de::Error
//! |-- de::Error::custom
//! |-- de::Error::missing_field
//! |-- de::MapAccess
//! |-- de::SeqAccess
//! |-- fmt::Formatter
//! |-- fmt::Result
//! |-- serde::Deserialize
//! |-- serde::Deserializer
//! |-- serde::Serialize
//! |-- serde::de::IgnoredAny
//! |-- serde::de::Visitor
//! |-- serde::de::self
//! |-- serde_json::Value
//! `-- serde_json::from_value
//! Used by:
//! rust/crates/
//! |-- ui-terminal/src/lib.rs
//! `-- ui-terminal/tests/keybind.rs
//! Invariants: defaults match baseline keybind table, parsing handles leader and modifiers.
//! Contract tests: tests/keybind.rs

use serde::{Deserialize, Deserializer, Serialize};
use std::collections::HashMap;
use std::sync::LazyLock;

/// Represents a parsed key stroke.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyStroke {
    pub name: String,
    #[serde(default)]
    pub ctrl: bool,
    #[serde(default)]
    pub shift: bool,
    #[serde(default)]
    pub meta: bool,
    #[serde(default, rename = "super")]
    pub super_key: bool,
    #[serde(default)]
    pub hyper: bool,
}

/// Represents a binding value, which can be disabled, none, a single key, or multiple keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindingValue {
    Disabled,
    None,
    Single(KeyStroke),
    Multiple(Vec<KeyStroke>),
}

impl<'de> Deserialize<'de> for BindingValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        use serde::de::{self, Visitor};
        use std::fmt;

        struct BindingValueVisitor;

        impl<'de> Visitor<'de> for BindingValueVisitor {
            type Value = BindingValue;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter
                    .write_str("false, \"none\", a key string, a key object, or an array of keys")
            }

            fn visit_bool<E>(self, v: bool) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                if !v {
                    Ok(BindingValue::Disabled)
                } else {
                    Err(de::Error::custom("expected false for disabled"))
                }
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                if v == "none" {
                    Ok(BindingValue::None)
                } else {
                    let strokes: Result<Vec<KeyStroke>, _> =
                        v.split(',').map(parse_key_stroke).collect();
                    let strokes = strokes.map_err(de::Error::custom)?;
                    if strokes.len() == 1 {
                        Ok(BindingValue::Single(strokes.into_iter().next().unwrap()))
                    } else {
                        Ok(BindingValue::Multiple(strokes))
                    }
                }
            }

            fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
            where
                M: de::MapAccess<'de>,
            {
                let mut name = None;
                let mut ctrl = false;
                let mut shift = false;
                let mut meta = false;
                let mut super_key = false;
                let mut hyper = false;

                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "name" => name = Some(map.next_value()?),
                        "ctrl" => ctrl = map.next_value()?,
                        "shift" => shift = map.next_value()?,
                        "meta" => meta = map.next_value()?,
                        "super" => super_key = map.next_value()?,
                        "hyper" => hyper = map.next_value()?,
                        "key" => {
                            let key_val: serde_json::Value = map.next_value()?;
                            if let Some(s) = key_val.as_str() {
                                let stroke = parse_key_stroke(s).map_err(de::Error::custom)?;
                                return Ok(BindingValue::Single(stroke));
                            } else if let Ok(stroke) = serde_json::from_value::<KeyStroke>(key_val)
                            {
                                return Ok(BindingValue::Single(stroke));
                            } else {
                                return Err(de::Error::custom("invalid key value"));
                            }
                        }
                        _ => {
                            let _ = map.next_value::<serde::de::IgnoredAny>();
                        }
                    }
                }

                let name = name.ok_or_else(|| de::Error::missing_field("name"))?;
                Ok(BindingValue::Single(KeyStroke {
                    name,
                    ctrl,
                    shift,
                    meta,
                    super_key,
                    hyper,
                }))
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: de::SeqAccess<'de>,
            {
                let mut strokes = Vec::new();
                while let Some(val) = seq.next_element::<serde_json::Value>()? {
                    if let Some(s) = val.as_str() {
                        strokes.push(parse_key_stroke(s).map_err(de::Error::custom)?);
                    } else if let Ok(stroke) = serde_json::from_value::<KeyStroke>(val) {
                        strokes.push(stroke);
                    } else {
                        return Err(de::Error::custom("expected string or key object in array"));
                    }
                }
                if strokes.len() == 1 {
                    Ok(BindingValue::Single(strokes.into_iter().next().unwrap()))
                } else {
                    Ok(BindingValue::Multiple(strokes))
                }
            }
        }

        deserializer.deserialize_any(BindingValueVisitor)
    }
}

pub fn parse_key_stroke(s: &str) -> Result<KeyStroke, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("empty key stroke".to_string());
    }

    let mut ctrl = false;
    let mut shift = false;
    let mut meta = false;
    let mut super_key = false;
    let mut hyper = false;
    let mut name = String::new();

    let parts: Vec<&str> = s.split('+').map(|p| p.trim()).collect();
    let mut key_part_found = false;

    for part in parts {
        let lower = part.to_lowercase();
        match lower.as_str() {
            "ctrl" | "control" => ctrl = true,
            "shift" => shift = true,
            "meta" | "alt" | "option" => meta = true,
            "super" | "cmd" | "command" | "win" => super_key = true,
            "hyper" => hyper = true,
            _ => {
                if key_part_found {
                    return Err(format!("multiple key parts in '{}'", s));
                }
                name = part.to_string();
                key_part_found = true;
            }
        }
    }

    if !key_part_found {
        return Err(format!("no key name found in '{}'", s));
    }

    // If the name is a single uppercase letter, it implies shift modifier
    if name.len() == 1 && name.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
        shift = true;
        name = name.to_lowercase();
    }

    Ok(KeyStroke {
        name,
        ctrl,
        shift,
        meta,
        super_key,
        hyper,
    })
}

impl KeyStroke {
    /// Check if this key stroke matches a UI key event.
    /// Returns false for non-Press events.
    pub fn matches(&self, event: &crate::input::UiKeyEvent) -> bool {
        use crate::input::{UiKeyCode, UiKeyEventKind};

        // Only match on Press events
        if event.kind != UiKeyEventKind::Press {
            return false;
        }

        // Check modifiers
        if self.ctrl != event.modifiers.control {
            return false;
        }
        if self.shift != event.modifiers.shift {
            return false;
        }
        if self.meta != event.modifiers.alt {
            return false;
        }
        if self.super_key != event.modifiers.super_key {
            return false;
        }
        if self.hyper != event.modifiers.hyper {
            return false;
        }

        // Check key code
        let name_lower = self.name.to_lowercase();
        match name_lower.as_str() {
            "escape" | "esc" => matches!(event.code, UiKeyCode::Esc),
            "return" | "enter" => matches!(event.code, UiKeyCode::Enter),
            "tab" => matches!(event.code, UiKeyCode::Tab),
            "backspace" => matches!(event.code, UiKeyCode::Backspace),
            "delete" | "del" => matches!(event.code, UiKeyCode::Delete),
            "insert" => matches!(event.code, UiKeyCode::Insert),
            "home" => matches!(event.code, UiKeyCode::Home),
            "end" => matches!(event.code, UiKeyCode::End),
            "pageup" => matches!(event.code, UiKeyCode::PageUp),
            "pagedown" => matches!(event.code, UiKeyCode::PageDown),
            "left" => matches!(event.code, UiKeyCode::Left),
            "right" => matches!(event.code, UiKeyCode::Right),
            "up" => matches!(event.code, UiKeyCode::Up),
            "down" => matches!(event.code, UiKeyCode::Down),
            "space" => matches!(event.code, UiKeyCode::Char(' ')),
            _ if name_lower.starts_with("f") && name_lower.len() > 1 => {
                if let Ok(n) = name_lower[1..].parse::<u8>() {
                    matches!(event.code, UiKeyCode::F(f_n) if f_n == n)
                } else {
                    false
                }
            }
            _ => {
                // Single character key
                // Since modifiers (including shift) are already checked above,
                // we can safely compare case-insensitively.
                if self.name.len() == 1 {
                    if let UiKeyCode::Char(c) = event.code {
                        self.name.chars().next().map(|n| n.to_ascii_lowercase())
                            == Some(c.to_ascii_lowercase())
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
        }
    }
}

pub fn parse_binding_value(s: &str) -> Result<BindingValue, String> {
    if s == "false" {
        return Ok(BindingValue::Disabled);
    }
    if s == "none" {
        return Ok(BindingValue::None);
    }
    let strokes: Result<Vec<KeyStroke>, _> = s.split(',').map(parse_key_stroke).collect();
    let strokes = strokes?;
    if strokes.len() == 1 {
        Ok(BindingValue::Single(strokes.into_iter().next().unwrap()))
    } else {
        Ok(BindingValue::Multiple(strokes))
    }
}

const DEFAULTS: &[(&str, &str)] = &[
    ("leader", "ctrl+x"), ("app_exit", "ctrl+c,ctrl+d,<leader>q"), ("app_debug", "none"), ("app_console", "none"),
    ("app_heap_snapshot", "none"), ("app_toggle_animations", "none"), ("app_toggle_file_context", "none"), ("app_toggle_diffwrap", "none"),
    ("app_toggle_paste_summary", "none"), ("app_toggle_session_directory_filter", "none"), ("command_list", "ctrl+p"), ("help_show", "none"),
    ("docs_open", "none"), ("diff_open", "none"), ("diff_close", "escape,q"), ("diff_toggle", "enter,space"),
    ("diff_expand", "right"), ("diff_expand_all", "E"), ("diff_collapse", "left"), ("diff_switch_focus", "tab"),
    ("diff_next_hunk", "]"), ("diff_previous_hunk", "["), ("diff_next_file", "n"), ("diff_previous_file", "p"),
    ("diff_toggle_file_tree", "b"), ("diff_single_patch", "s"), ("diff_switch_source", "d"), ("diff_toggle_view", "v"),
    ("diff_help", "?"), ("editor_open", "<leader>e"), ("theme_list", "<leader>t"), ("theme_switch_mode", "none"),
    ("theme_mode_lock", "none"), ("sidebar_toggle", "<leader>b"), ("scrollbar_toggle", "none"), ("status_view", "<leader>s"),
    ("debug_view", "none"), ("session_export", "<leader>x"), ("session_copy", "none"), ("session_move", "none"),
    ("session_new", "<leader>n"), ("session_list", "<leader>l"), ("session_timeline", "<leader>g"), ("session_fork", "none"),
    ("session_rename", "ctrl+r"), ("session_delete", "ctrl+d"), ("session_share", "none"), ("session_unshare", "none"),
    ("session_interrupt", "escape"), ("session_background", "ctrl+b"), ("session_compact", "<leader>c"), ("session_toggle_timestamps", "none"),
    ("session_toggle_generic_tool_output", "none"), ("session_queued_prompts", "<leader>q"), ("session_child_first", "<leader>down"), ("session_child_cycle", "right"),
    ("session_child_cycle_reverse", "left"), ("session_parent", "up"), ("session_pin_toggle", "ctrl+f"), ("session_quick_switch_1", "<leader>1"),
    ("session_quick_switch_2", "<leader>2"), ("session_quick_switch_3", "<leader>3"), ("session_quick_switch_4", "<leader>4"), ("session_quick_switch_5", "<leader>5"),
    ("session_quick_switch_6", "<leader>6"), ("session_quick_switch_7", "<leader>7"), ("session_quick_switch_8", "<leader>8"), ("session_quick_switch_9", "<leader>9"),
    ("stash_delete", "ctrl+d"), ("model_provider_list", "ctrl+a"), ("model_favorite_toggle", "ctrl+f"), ("model_list", "<leader>m"),
    ("model_cycle_recent", "f2"), ("model_cycle_recent_reverse", "shift+f2"), ("model_cycle_favorite", "none"), ("model_cycle_favorite_reverse", "none"),
    ("mcp_list", "none"), ("provider_connect", "none"), ("console_org_switch", "none"), ("agent_list", "<leader>a"),
    ("agent_cycle", "tab"), ("agent_cycle_reverse", "shift+tab"), ("variant_cycle", "ctrl+t"), ("variant_list", "none"),
    ("messages_page_up", "pageup,ctrl+alt+b"), ("messages_page_down", "pagedown,ctrl+alt+f"), ("messages_line_up", "ctrl+alt+y"), ("messages_line_down", "ctrl+alt+e"),
    ("messages_half_page_up", "ctrl+alt+u"), ("messages_half_page_down", "ctrl+alt+d"), ("messages_first", "ctrl+g,home"), ("messages_last", "ctrl+alt+g,end"),
    ("messages_next", "none"), ("messages_previous", "none"), ("messages_last_user", "none"), ("messages_copy", "<leader>y"),
    ("messages_undo", "<leader>u"), ("messages_redo", "<leader>r"), ("messages_toggle_conceal", "<leader>h"), ("tool_details", "none"),
    ("display_thinking", "none"), ("prompt_submit", "none"), ("prompt_editor_context_clear", "none"), ("prompt_skills", "none"),
    ("prompt_stash", "none"), ("prompt_stash_pop", "none"), ("prompt_stash_list", "none"), ("workspace_set", "none"),
    ("input_clear", "ctrl+c"), ("input_paste", "ctrl+v"), ("input_submit", "return"), ("input_move_left", "left,ctrl+b"),
    ("input_move_right", "right,ctrl+f"), ("input_move_up", "up"), ("input_move_down", "down"), ("input_select_left", "shift+left"),
    ("input_select_right", "shift+right"), ("input_select_up", "shift+up"), ("input_select_down", "shift+down"), ("input_line_home", "ctrl+a"),
    ("input_line_end", "ctrl+e"), ("input_select_line_home", "ctrl+shift+a"), ("input_select_line_end", "ctrl+shift+e"), ("input_visual_line_home", "alt+a"),
    ("input_visual_line_end", "alt+e"), ("input_select_visual_line_home", "alt+shift+a"), ("input_select_visual_line_end", "alt+shift+e"), ("input_buffer_home", "home"),
    ("input_buffer_end", "end"), ("input_select_buffer_home", "shift+home"), ("input_select_buffer_end", "shift+end"), ("input_delete_line", "ctrl+shift+d"),
    ("input_delete_to_line_end", "ctrl+k"), ("input_delete_to_line_start", "ctrl+u"), ("input_backspace", "backspace,shift+backspace"), ("input_delete", "ctrl+d,delete,shift+delete"),
    ("input_undo", "ctrl+-,super+z"), ("input_redo", "ctrl+.,super+shift+z"), ("input_word_forward", "alt+f,alt+right,ctrl+right"), ("input_word_backward", "alt+b,alt+left,ctrl+left"),
    ("input_select_all", "super+a"), ("history_previous", "up"), ("history_next", "down"), ("dialog.select.prev", "up,ctrl+p"),
    ("dialog.select.next", "down,ctrl+n"), ("dialog.select.page_up", "pageup"), ("dialog.select.page_down", "pagedown"), ("dialog.select.home", "home"),
    ("dialog.select.end", "end"), ("dialog.select.submit", "return"), ("dialog.prompt.submit", "return"), ("dialog.mcp.toggle", "space"),
    ("dialog.move_session.new", "ctrl+m"), ("dialog.move_session.delete", "ctrl+d"), ("dialog.move_session.refresh", "ctrl+r"), ("prompt.autocomplete.prev", "up,ctrl+p"),
    ("prompt.autocomplete.next", "down,ctrl+n"), ("prompt.autocomplete.hide", "escape"), ("prompt.autocomplete.select", "return"), ("prompt.autocomplete.complete", "tab"),
    ("permission.prompt.fullscreen", "ctrl+f"), ("plugins.toggle", "space"), ("dialog.plugins.install", "shift+i"), ("terminal_suspend", "ctrl+z"),
    ("terminal_title_toggle", "none"), ("tips_toggle", "<leader>h"), ("plugin_manager", "none"), ("plugin_install", "none"),
    ("which_key_toggle", "ctrl+alt+k"), ("which_key_layout_toggle", "ctrl+alt+shift+k"), ("which_key_pending_toggle", "ctrl+alt+shift+p"), ("which_key_group_previous", "ctrl+alt+left,ctrl+alt+["),
    ("which_key_group_next", "ctrl+alt+right,ctrl+alt+]"), ("which_key_scroll_up", "ctrl+alt+up,ctrl+alt+p"), ("which_key_scroll_down", "ctrl+alt+down,ctrl+alt+n"), ("which_key_page_up", "ctrl+alt+pageup"),
    ("which_key_page_down", "ctrl+alt+pagedown"), ("which_key_home", "ctrl+alt+home"), ("which_key_end", "ctrl+alt+end"),
];

pub static DEFAULT_KEYBINDS: LazyLock<HashMap<&'static str, BindingValue>> = LazyLock::new(|| {
    DEFAULTS.iter().map(|&(k, v)| (k, parse_binding_value(v).unwrap())).collect()
});

pub fn get_default(name: &str) -> Option<BindingValue> {
    DEFAULT_KEYBINDS.get(name).cloned()
}

pub fn parse_overrides(
    overrides: &HashMap<String, serde_json::Value>,
) -> Result<HashMap<String, BindingValue>, String> {
    let mut result = HashMap::new();
    for (k, v) in overrides {
        if !DEFAULT_KEYBINDS.contains_key(k.as_str()) {
            return Err(format!("Unrecognized keybind: {}", k));
        }
        let val: BindingValue = serde_json::from_value(v.clone()).map_err(|e| e.to_string())?;
        result.insert(k.clone(), val);
    }
    Ok(result)
}

pub fn merge_defaults(overrides: &HashMap<String, BindingValue>) -> HashMap<String, BindingValue> {
    let mut result = HashMap::new();
    for (k, v) in DEFAULT_KEYBINDS.iter() {
        result.insert(
            k.to_string(),
            overrides.get(*k).cloned().unwrap_or_else(|| v.clone()),
        );
    }
    result
}
