//! Purpose: resolve theme JSON (defs, references, dark/light variants, ANSI codes) into concrete
//! colors exactly like `resolveTheme` and `selectedForeground` in packages/tui/src/theme/index.ts.
//! Scope: pure values.
//! Uses:
//! rust/crates/
//! |-- ui-terminal/src/theme/color.rs
//! `-- ui-terminal/src/theme/mod.rs
//! External:
//! |-- serde_json::Value
//! `-- thiserror::Error
//! Used by:
//! rust/crates/
//! `-- ui-terminal/src/theme/mod.rs
//! Invariants: circular and missing references fail with the baseline messages, optional keys fall back
//! (`selectedListItemText` -> background, `backgroundMenu` -> backgroundElement, `thinkingOpacity` -> 0.6).
//! Contract tests: ui-terminal/tests/theme.rs

use serde_json::{Map, Value};

use super::Theme;
use super::color::{Rgba, ansi_to_rgba};

/// Light or dark appearance used to pick a variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mode {
    /// Dark appearance.
    Dark,
    /// Light appearance.
    Light,
}

impl Mode {
    /// Baseline string form (`"dark"` / `"light"`).
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Dark => "dark",
            Mode::Light => "light",
        }
    }

    /// Parse the baseline string form, anything else is `None` (baseline `pick`).
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "dark" => Some(Mode::Dark),
            "light" => Some(Mode::Light),
            _ => None,
        }
    }
}

/// Resolution failures. Messages match the baseline `Error` strings.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ThemeError {
    /// A reference chain loops back on itself.
    #[error("Circular color reference: {0}")]
    Circular(String),
    /// A reference names neither a def nor a theme key.
    #[error("Color reference \"{0}\" not found in defs or theme")]
    NotFound(String),
    /// The value is not a string, number or `{dark, light}` object, or the theme object is missing.
    #[error("Invalid color value for \"{0}\"")]
    Invalid(String),
    /// A required token is absent from the theme (baseline would yield `undefined`).
    #[error("Theme is missing required color \"{0}\"")]
    Missing(&'static str),
}

macro_rules! theme_tokens {
    ($($json:literal => $field:ident),* $(,)?) => {
        /// Resolved theme keeping full RGBA (alpha matters for `selectedForeground`) plus the
        /// baseline `_hasSelectedListItemText` flag. Convert with [`ResolvedTheme::to_theme`].
        #[derive(Debug, Clone, PartialEq)]
        pub struct ResolvedTheme {
            $(
                #[doc = concat!("Baseline key `", $json, "`.")]
                pub $field: Rgba,
            )*
            /// Baseline `_hasSelectedListItemText`.
            pub has_selected_list_item_text: bool,
            /// Baseline `thinkingOpacity`.
            pub thinking_opacity: f64,
        }

        /// Every resolved token key in baseline `Theme` order.
        pub const THEME_KEYS: &[&str] = &[$($json),*];

        impl ResolvedTheme {
            /// Terminal colors for rendering. Transparent maps to the terminal default color.
            pub fn to_theme(&self) -> Theme {
                Theme {
                    $($field: self.$field.to_color(),)*
                    thinking_opacity: self.thinking_opacity as f32,
                }
            }

            /// Token by baseline key (`"textMuted"`), `None` for unknown keys.
            pub fn get(&self, key: &str) -> Option<Rgba> {
                match key {
                    $($json => Some(self.$field),)*
                    _ => None,
                }
            }

            fn from_map(resolved: &std::collections::HashMap<String, Rgba>, thinking_opacity: f64, has_selected: bool) -> Result<Self, ThemeError> {
                let get = |key: &'static str| resolved.get(key).copied().ok_or(ThemeError::Missing(key));
                Ok(Self {
                    $($field: get($json)?,)*
                    has_selected_list_item_text: has_selected,
                    thinking_opacity,
                })
            }
        }
    };
}

theme_tokens! {
    "primary" => primary, "secondary" => secondary, "accent" => accent, "error" => error,
    "warning" => warning, "success" => success, "info" => info, "text" => text,
    "textMuted" => text_muted, "selectedListItemText" => selected_list_item_text,
    "background" => background, "backgroundPanel" => background_panel,
    "backgroundElement" => background_element, "backgroundMenu" => background_menu,
    "border" => border, "borderActive" => border_active, "borderSubtle" => border_subtle,
    "diffAdded" => diff_added, "diffRemoved" => diff_removed, "diffContext" => diff_context,
    "diffHunkHeader" => diff_hunk_header, "diffHighlightAdded" => diff_highlight_added,
    "diffHighlightRemoved" => diff_highlight_removed, "diffAddedBg" => diff_added_bg,
    "diffRemovedBg" => diff_removed_bg, "diffContextBg" => diff_context_bg,
    "diffLineNumber" => diff_line_number, "diffAddedLineNumberBg" => diff_added_line_number_bg,
    "diffRemovedLineNumberBg" => diff_removed_line_number_bg, "markdownText" => markdown_text,
    "markdownHeading" => markdown_heading, "markdownLink" => markdown_link,
    "markdownLinkText" => markdown_link_text, "markdownCode" => markdown_code,
    "markdownBlockQuote" => markdown_block_quote, "markdownEmph" => markdown_emph,
    "markdownStrong" => markdown_strong, "markdownHorizontalRule" => markdown_horizontal_rule,
    "markdownListItem" => markdown_list_item, "markdownListEnumeration" => markdown_list_enumeration,
    "markdownImage" => markdown_image, "markdownImageText" => markdown_image_text,
    "markdownCodeBlock" => markdown_code_block, "syntaxComment" => syntax_comment,
    "syntaxKeyword" => syntax_keyword, "syntaxFunction" => syntax_function,
    "syntaxVariable" => syntax_variable, "syntaxString" => syntax_string,
    "syntaxNumber" => syntax_number, "syntaxType" => syntax_type,
    "syntaxOperator" => syntax_operator, "syntaxPunctuation" => syntax_punctuation,
}

const OPTIONAL_KEYS: [&str; 3] = ["selectedListItemText", "backgroundMenu", "thinkingOpacity"];

/// Port of `resolveTheme(theme, mode)`. `theme` is the raw theme JSON (`{ defs?, theme }`).
pub fn resolve_theme(theme: &Value, mode: Mode) -> Result<ResolvedTheme, ThemeError> {
    let colors = theme
        .get("theme")
        .and_then(Value::as_object)
        .ok_or_else(|| ThemeError::Invalid("theme".into()))?;
    let empty = Map::new();
    let defs = theme
        .get("defs")
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    let resolver = Resolver { defs, colors, mode };

    let mut resolved = std::collections::HashMap::new();
    for (key, value) in colors
        .iter()
        .filter(|(key, _)| !OPTIONAL_KEYS.contains(&key.as_str()))
    {
        resolved.insert(key.clone(), resolver.resolve(value, &mut Vec::new(), key)?);
    }
    let has_selected = colors.contains_key("selectedListItemText");
    let fallback =
        |key: &str, from: &str, resolved: &std::collections::HashMap<String, Rgba>| match colors
            .get(key)
        {
            Some(value) => resolver.resolve(value, &mut Vec::new(), key).map(Some),
            None => Ok(resolved.get(from).copied()),
        };
    if let Some(color) = fallback("selectedListItemText", "background", &resolved)? {
        resolved.insert("selectedListItemText".into(), color);
    }
    if let Some(color) = fallback("backgroundMenu", "backgroundElement", &resolved)? {
        resolved.insert("backgroundMenu".into(), color);
    }
    let thinking_opacity = colors
        .get("thinkingOpacity")
        .and_then(Value::as_f64)
        .unwrap_or(0.6);
    ResolvedTheme::from_map(&resolved, thinking_opacity, has_selected)
}

struct Resolver<'a> {
    defs: &'a Map<String, Value>,
    colors: &'a Map<String, Value>,
    mode: Mode,
}

impl Resolver<'_> {
    fn resolve(
        &self,
        value: &Value,
        chain: &mut Vec<String>,
        key: &str,
    ) -> Result<Rgba, ThemeError> {
        match value {
            Value::String(text) => self.resolve_str(text, chain, key),
            Value::Number(number) => Ok(ansi_to_rgba(number.as_f64().unwrap_or(0.0))),
            Value::Object(variant) => {
                let next = variant
                    .get(self.mode.as_str())
                    .ok_or_else(|| ThemeError::Invalid(key.to_string()))?;
                self.resolve(next, chain, key)
            }
            _ => Err(ThemeError::Invalid(key.to_string())),
        }
    }

    fn resolve_str(
        &self,
        text: &str,
        chain: &mut Vec<String>,
        key: &str,
    ) -> Result<Rgba, ThemeError> {
        if text == "transparent" || text == "none" {
            return Ok(Rgba::transparent());
        }
        if text.starts_with('#') {
            return Ok(Rgba::from_hex(text));
        }
        if chain.iter().any(|item| item == text) {
            let mut path = chain.clone();
            path.push(text.to_string());
            return Err(ThemeError::Circular(path.join(" -> ")));
        }
        let next = self
            .defs
            .get(text)
            .filter(|value| !value.is_null())
            .or_else(|| self.colors.get(text))
            .ok_or_else(|| ThemeError::NotFound(text.to_string()))?;
        chain.push(text.to_string());
        let result = self.resolve(next, chain, key);
        chain.pop();
        result
    }
}

/// Port of `selectedForeground(theme, bg?)`: explicit token, else contrast against the actual
/// background for transparent themes, else the theme background.
pub fn selected_foreground(theme: &ResolvedTheme, bg: Option<Rgba>) -> Rgba {
    if theme.has_selected_list_item_text {
        return theme.selected_list_item_text;
    }
    if theme.background.a == 0 {
        let target = bg.unwrap_or(theme.primary);
        if target.luminance() > 0.5 {
            return Rgba::rgb(0, 0, 0);
        }
        return Rgba::rgb(255, 255, 255);
    }
    theme.background
}
