//! Purpose: keybinding configuration parsing and key matching (leader, chords) ported from packages/tui/src/config/keybind.ts.
//! Scope: pure values, parsing, and default definitions.
//! Uses:
//! rust/crates/
//! `-- ui-terminal/src/lib.rs
//! External:
//! |-- serde::{Deserialize, Deserializer, Serialize}
//! `-- std::collections::HashMap
//! Used by:
//! rust/crates/
//! `-- ui-terminal/src/lib.rs
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
                formatter.write_str("false, \"none\", a key string, a key object, or an array of keys")
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
                    let strokes: Result<Vec<KeyStroke>, _> = v.split(',').map(parse_key_stroke).collect();
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
                            } else if let Ok(stroke) = serde_json::from_value::<KeyStroke>(key_val) {
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

    Ok(KeyStroke {
        name,
        ctrl,
        shift,
        meta,
        super_key,
        hyper,
    })
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

macro_rules! kb {
    ($m:expr, $name:expr, $val:expr) => {
        $m.insert($name, parse_binding_value($val).unwrap());
    };
}

pub static DEFAULT_KEYBINDS: LazyLock<HashMap<&'static str, BindingValue>> = LazyLock::new(|| {
    let mut m = HashMap::new();
    kb!(m, "leader", "ctrl+x");
    kb!(m, "app_exit", "ctrl+c,ctrl+d,<leader>q");
    kb!(m, "app_debug", "none");
    kb!(m, "app_console", "none");
    kb!(m, "app_heap_snapshot", "none");
    kb!(m, "app_toggle_animations", "none");
    kb!(m, "app_toggle_file_context", "none");
    kb!(m, "app_toggle_diffwrap", "none");
    kb!(m, "app_toggle_paste_summary", "none");
    kb!(m, "app_toggle_session_directory_filter", "none");
    kb!(m, "command_list", "ctrl+p");
    kb!(m, "help_show", "none");
    kb!(m, "docs_open", "none");
    kb!(m, "diff_open", "none");
    kb!(m, "diff_close", "escape,q");
    kb!(m, "diff_toggle", "enter,space");
    kb!(m, "diff_expand", "right");
    kb!(m, "diff_expand_all", "E");
    kb!(m, "diff_collapse", "left");
    kb!(m, "diff_switch_focus", "tab");
    kb!(m, "diff_next_hunk", "]");
    kb!(m, "diff_previous_hunk", "[");
    kb!(m, "diff_next_file", "n");
    kb!(m, "diff_previous_file", "p");
    kb!(m, "diff_toggle_file_tree", "b");
    kb!(m, "diff_single_patch", "s");
    kb!(m, "diff_switch_source", "d");
    kb!(m, "diff_toggle_view", "v");
    kb!(m, "diff_help", "?");
    kb!(m, "editor_open", "<leader>e");
    kb!(m, "theme_list", "<leader>t");
    kb!(m, "theme_switch_mode", "none");
    kb!(m, "theme_mode_lock", "none");
    kb!(m, "sidebar_toggle", "<leader>b");
    kb!(m, "scrollbar_toggle", "none");
    kb!(m, "status_view", "<leader>s");
    kb!(m, "debug_view", "none");
    kb!(m, "session_export", "<leader>x");
    kb!(m, "session_copy", "none");
    kb!(m, "session_move", "none");
    kb!(m, "session_new", "<leader>n");
    kb!(m, "session_list", "<leader>l");
    kb!(m, "session_timeline", "<leader>g");
    kb!(m, "session_fork", "none");
    kb!(m, "session_rename", "ctrl+r");
    kb!(m, "session_delete", "ctrl+d");
    kb!(m, "session_share", "none");
    kb!(m, "session_unshare", "none");
    kb!(m, "session_interrupt", "escape");
    kb!(m, "session_background", "ctrl+b");
    kb!(m, "session_compact", "<leader>c");
    kb!(m, "session_toggle_timestamps", "none");
    kb!(m, "session_toggle_generic_tool_output", "none");
    kb!(m, "session_queued_prompts", "<leader>q");
    kb!(m, "session_child_first", "<leader>down");
    kb!(m, "session_child_cycle", "right");
    kb!(m, "session_child_cycle_reverse", "left");
    kb!(m, "session_parent", "up");
    kb!(m, "session_pin_toggle", "ctrl+f");
    kb!(m, "session_quick_switch_1", "<leader>1");
    kb!(m, "session_quick_switch_2", "<leader>2");
    kb!(m, "session_quick_switch_3", "<leader>3");
    kb!(m, "session_quick_switch_4", "<leader>4");
    kb!(m, "session_quick_switch_5", "<leader>5");
    kb!(m, "session_quick_switch_6", "<leader>6");
    kb!(m, "session_quick_switch_7", "<leader>7");
    kb!(m, "session_quick_switch_8", "<leader>8");
    kb!(m, "session_quick_switch_9", "<leader>9");
    kb!(m, "stash_delete", "ctrl+d");
    kb!(m, "model_provider_list", "ctrl+a");
    kb!(m, "model_favorite_toggle", "ctrl+f");
    kb!(m, "model_list", "<leader>m");
    kb!(m, "model_cycle_recent", "f2");
    kb!(m, "model_cycle_recent_reverse", "shift+f2");
    kb!(m, "model_cycle_favorite", "none");
    kb!(m, "model_cycle_favorite_reverse", "none");
    kb!(m, "mcp_list", "none");
    kb!(m, "provider_connect", "none");
    kb!(m, "console_org_switch", "none");
    kb!(m, "agent_list", "<leader>a");
    kb!(m, "agent_cycle", "tab");
    kb!(m, "agent_cycle_reverse", "shift+tab");
    kb!(m, "variant_cycle", "ctrl+t");
    kb!(m, "variant_list", "none");
    kb!(m, "messages_page_up", "pageup,ctrl+alt+b");
    kb!(m, "messages_page_down", "pagedown,ctrl+alt+f");
    kb!(m, "messages_line_up", "ctrl+alt+y");
    kb!(m, "messages_line_down", "ctrl+alt+e");
    kb!(m, "messages_half_page_up", "ctrl+alt+u");
    kb!(m, "messages_half_page_down", "ctrl+alt+d");
    kb!(m, "messages_first", "ctrl+g,home");
    kb!(m, "messages_last", "ctrl+alt+g,end");
    kb!(m, "messages_next", "none");
    kb!(m, "messages_previous", "none");
    kb!(m, "messages_last_user", "none");
    kb!(m, "messages_copy", "<leader>y");
    kb!(m, "messages_undo", "<leader>u");
    kb!(m, "messages_redo", "<leader>r");
    kb!(m, "messages_toggle_conceal", "<leader>h");
    kb!(m, "tool_details", "none");
    kb!(m, "display_thinking", "none");
    kb!(m, "prompt_submit", "none");
    kb!(m, "prompt_editor_context_clear", "none");
    kb!(m, "prompt_skills", "none");
    kb!(m, "prompt_stash", "none");
    kb!(m, "prompt_stash_pop", "none");
    kb!(m, "prompt_stash_list", "none");
    kb!(m, "workspace_set", "none");
    kb!(m, "input_clear", "ctrl+c");
    kb!(m, "input_paste", "ctrl+v");
    kb!(m, "input_submit", "return");
    kb!(m, "input_newline", "shift+return,ctrl+return,alt+return,ctrl+j");
    kb!(m, "input_move_left", "left,ctrl+b");
    kb!(m, "input_move_right", "right,ctrl+f");
    kb!(m, "input_move_up", "up");
    kb!(m, "input_move_down", "down");
    kb!(m, "input_select_left", "shift+left");
    kb!(m, "input_select_right", "shift+right");
    kb!(m, "input_select_up", "shift+up");
    kb!(m, "input_select_down", "shift+down");
    kb!(m, "input_line_home", "ctrl+a");
    kb!(m, "input_line_end", "ctrl+e");
    kb!(m, "input_select_line_home", "ctrl+shift+a");
    kb!(m, "input_select_line_end", "ctrl+shift+e");
    kb!(m, "input_visual_line_home", "alt+a");
    kb!(m, "input_visual_line_end", "alt+e");
    kb!(m, "input_select_visual_line_home", "alt+shift+a");
    kb!(m, "input_select_visual_line_end", "alt+shift+e");
    kb!(m, "input_buffer_home", "home");
    kb!(m, "input_buffer_end", "end");
    kb!(m, "input_select_buffer_home", "shift+home");
    kb!(m, "input_select_buffer_end", "shift+end");
    kb!(m, "input_delete_line", "ctrl+shift+d");
    kb!(m, "input_delete_to_line_end", "ctrl+k");
    kb!(m, "input_delete_to_line_start", "ctrl+u");
    kb!(m, "input_backspace", "backspace,shift+backspace");
    kb!(m, "input_delete", "ctrl+d,delete,shift+delete");
    kb!(m, "input_undo", "ctrl+-,super+z");
    kb!(m, "input_redo", "ctrl+.,super+shift+z");
    kb!(m, "input_word_forward", "alt+f,alt+right,ctrl+right");
    kb!(m, "input_word_backward", "alt+b,alt+left,ctrl+left");
    kb!(m, "input_select_word_forward", "alt+shift+f,alt+shift+right");
    kb!(m, "input_select_word_backward", "alt+shift+b,alt+shift+left");
    kb!(m, "input_delete_word_forward", "alt+d,alt+delete,ctrl+delete");
    kb!(m, "input_delete_word_backward", "ctrl+w,ctrl+backspace,alt+backspace");
    kb!(m, "input_select_all", "super+a");
    kb!(m, "history_previous", "up");
    kb!(m, "history_next", "down");
    kb!(m, "dialog.select.prev", "up,ctrl+p");
    kb!(m, "dialog.select.next", "down,ctrl+n");
    kb!(m, "dialog.select.page_up", "pageup");
    kb!(m, "dialog.select.page_down", "pagedown");
    kb!(m, "dialog.select.home", "home");
    kb!(m, "dialog.select.end", "end");
    kb!(m, "dialog.select.submit", "return");
    kb!(m, "dialog.prompt.submit", "return");
    kb!(m, "dialog.mcp.toggle", "space");
    kb!(m, "dialog.move_session.new", "ctrl+m");
    kb!(m, "dialog.move_session.delete", "ctrl+d");
    kb!(m, "dialog.move_session.refresh", "ctrl+r");
    kb!(m, "prompt.autocomplete.prev", "up,ctrl+p");
    kb!(m, "prompt.autocomplete.next", "down,ctrl+n");
    kb!(m, "prompt.autocomplete.hide", "escape");
    kb!(m, "prompt.autocomplete.select", "return");
    kb!(m, "prompt.autocomplete.complete", "tab");
    kb!(m, "permission.prompt.fullscreen", "ctrl+f");
    kb!(m, "plugins.toggle", "space");
    kb!(m, "dialog.plugins.install", "shift+i");
    kb!(m, "terminal_suspend", "ctrl+z");
    kb!(m, "terminal_title_toggle", "none");
    kb!(m, "tips_toggle", "<leader>h");
    kb!(m, "plugin_manager", "none");
    kb!(m, "plugin_install", "none");
    kb!(m, "which_key_toggle", "ctrl+alt+k");
    kb!(m, "which_key_layout_toggle", "ctrl+alt+shift+k");
    kb!(m, "which_key_pending_toggle", "ctrl+alt+shift+p");
    kb!(m, "which_key_group_previous", "ctrl+alt+left,ctrl+alt+[");
    kb!(m, "which_key_group_next", "ctrl+alt+right,ctrl+alt+]");
    kb!(m, "which_key_scroll_up", "ctrl+alt+up,ctrl+alt+p");
    kb!(m, "which_key_scroll_down", "ctrl+alt+down,ctrl+alt+n");
    kb!(m, "which_key_page_up", "ctrl+alt+pageup");
    kb!(m, "which_key_page_down", "ctrl+alt+pagedown");
    kb!(m, "which_key_home", "ctrl+alt+home");
    kb!(m, "which_key_end", "ctrl+alt+end");
    m
});

pub fn get_default(name: &str) -> Option<BindingValue> {
    DEFAULT_KEYBINDS.get(name).cloned()
}

pub fn parse_overrides(overrides: &HashMap<String, serde_json::Value>) -> Result<HashMap<String, BindingValue>, String> {
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
