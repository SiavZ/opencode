//! Purpose: keybinding configuration parsing and key matching (leader, chords) ported from packages/tui/src/config/keybind.ts.
//! Scope: pure values, parsing, and default definitions.
//! Uses:
//! rust/crates/
//! |-- ui-terminal/src/input.rs
//! `-- ui-terminal/src/keybind/defaults.rs
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

mod defaults;

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

pub static DEFAULT_KEYBINDS: LazyLock<HashMap<&'static str, BindingValue>> = LazyLock::new(|| {
    defaults::DEFAULTS
        .iter()
        .map(|&(k, v)| (k, parse_binding_value(v).unwrap()))
        .collect()
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
