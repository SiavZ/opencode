//! Purpose: derive the `system` theme from the terminal palette and the dark/light mode from the
//! terminal background, porting `terminalMode`, `generateSystem`, `generateGrayScale` and
//! `generateMutedTextColor` from packages/tui/src/theme/index.ts.
//! Scope: pure values.
//! Uses:
//! rust/crates/
//! |-- ui-terminal/src/theme/color.rs
//! `-- ui-terminal/src/theme/resolve.rs
//! External:
//! |-- f64::from
//! |-- serde_json::Map
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! |-- ui-terminal/src/theme/mod.rs
//! `-- ui-terminal/src/theme/registry.rs
//! Invariants: the generated JSON round-trips through `resolve_theme` with the same colors, the
//! background is the terminal background with alpha 0 so transparency is respected.
//! Contract tests: ui-terminal/tests/theme.rs

use serde_json::{Map, Value};

use super::color::{Rgba, ansi_to_rgba, tint};
use super::resolve::Mode;

/// Terminal colors queried from the terminal (OSC 4/10/11). Mirrors opentui `TerminalColors`
/// fields used by the theme code. Values are hex strings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TerminalColors {
    /// ANSI palette entries 0..N, `None` when the terminal did not answer for that slot.
    pub palette: Vec<Option<String>>,
    /// Default foreground (OSC 10).
    pub default_foreground: Option<String>,
    /// Default background (OSC 11).
    pub default_background: Option<String>,
}

/// The baseline generator requires background and foreground snapshots, either
/// from the defaults or palette slots 0 and 7. The app catches this failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SystemThemeError {
    #[error("system theme has no default background or palette slot 0")]
    MissingBackground,
    #[error("system theme has no default foreground or palette slot 7")]
    MissingForeground,
}

/// Port of `terminalMode`: light when the default background luminance is above 0.5. Never
/// derives a mode from palette slot zero.
pub fn terminal_mode(colors: &TerminalColors) -> Option<Mode> {
    let bg = Rgba::from_hex(
        colors
            .default_background
            .as_deref()
            .filter(|value| !value.is_empty())?,
    );
    Some(if bg.luminance() > 0.5 {
        Mode::Light
    } else {
        Mode::Dark
    })
}

/// Port of `generateSystem(colors, mode)`. Returns theme JSON (`{ "theme": {...} }`) with hex
/// strings. Missing required snapshots fail so the caller can retain a prior system
/// theme or fall back to opencode, as the baseline context does on palette failure.
pub fn generate_system(colors: &TerminalColors, mode: Mode) -> Result<Value, SystemThemeError> {
    let palette = |i: usize| colors.palette.get(i).cloned().flatten();
    let bg = Rgba::from_hex(
        &colors
            .default_background
            .clone()
            .or_else(|| palette(0))
            .ok_or(SystemThemeError::MissingBackground)?,
    );
    let fg = Rgba::from_hex(
        &colors
            .default_foreground
            .clone()
            .or_else(|| palette(7))
            .ok_or(SystemThemeError::MissingForeground)?,
    );
    let transparent = Rgba::new(bg.r, bg.g, bg.b, 0);
    let is_dark = mode == Mode::Dark;
    let col = |i: usize| {
        palette(i)
            .filter(|hex| !hex.is_empty())
            .map(|hex| Rgba::from_hex(&hex))
            .unwrap_or_else(|| ansi_to_rgba(i as f64))
    };

    let grays = generate_gray_scale(bg, is_dark);
    let gray = |i: usize| grays[i - 1];
    let text_muted = generate_muted_text_color(bg, is_dark);
    let (red, green, yellow, blue, magenta, cyan) =
        (col(1), col(2), col(3), col(4), col(5), col(6));
    let (red_bright, green_bright) = (col(9), col(10));

    let diff_alpha = if is_dark { 0.22 } else { 0.14 };
    let diff_context_bg = gray(2);
    let entries: [(&str, Rgba); 52] = [
        ("primary", cyan),
        ("secondary", magenta),
        ("accent", cyan),
        ("error", red),
        ("warning", yellow),
        ("success", green),
        ("info", cyan),
        ("text", fg),
        ("textMuted", text_muted),
        ("selectedListItemText", bg),
        ("background", transparent),
        ("backgroundPanel", gray(2)),
        ("backgroundElement", gray(3)),
        ("backgroundMenu", gray(3)),
        ("borderSubtle", gray(6)),
        ("border", gray(7)),
        ("borderActive", gray(8)),
        ("diffAdded", green),
        ("diffRemoved", red),
        ("diffContext", gray(7)),
        ("diffHunkHeader", gray(7)),
        ("diffHighlightAdded", green_bright),
        ("diffHighlightRemoved", red_bright),
        ("diffAddedBg", tint(bg, green, diff_alpha)),
        ("diffRemovedBg", tint(bg, red, diff_alpha)),
        ("diffContextBg", diff_context_bg),
        ("diffLineNumber", text_muted),
        (
            "diffAddedLineNumberBg",
            tint(diff_context_bg, green, diff_alpha),
        ),
        (
            "diffRemovedLineNumberBg",
            tint(diff_context_bg, red, diff_alpha),
        ),
        ("markdownText", fg),
        ("markdownHeading", fg),
        ("markdownLink", blue),
        ("markdownLinkText", cyan),
        ("markdownCode", green),
        ("markdownBlockQuote", yellow),
        ("markdownEmph", yellow),
        ("markdownStrong", fg),
        ("markdownHorizontalRule", gray(7)),
        ("markdownListItem", blue),
        ("markdownListEnumeration", cyan),
        ("markdownImage", blue),
        ("markdownImageText", cyan),
        ("markdownCodeBlock", fg),
        ("syntaxComment", text_muted),
        ("syntaxKeyword", magenta),
        ("syntaxFunction", blue),
        ("syntaxVariable", fg),
        ("syntaxString", green),
        ("syntaxNumber", yellow),
        ("syntaxType", cyan),
        ("syntaxOperator", cyan),
        ("syntaxPunctuation", fg),
    ];
    let theme: Map<String, Value> = entries
        .iter()
        .map(|(key, color)| ((*key).to_string(), Value::String(color.to_hex())))
        .collect();
    let mut root = Map::new();
    root.insert("theme".into(), Value::Object(theme));
    Ok(Value::Object(root))
}

/// Port of `generateGrayScale`: 12 steps (index 0 is baseline step 1).
pub fn generate_gray_scale(bg: Rgba, is_dark: bool) -> [Rgba; 12] {
    let (r, g, b) = (f64::from(bg.r), f64::from(bg.g), f64::from(bg.b));
    let luminance = 0.299 * r + 0.587 * g + 0.114 * b;
    std::array::from_fn(|index| {
        let factor = (index + 1) as f64 / 12.0;
        let (nr, ng, nb) = match (
            is_dark,
            is_dark && luminance < 10.0,
            !is_dark && luminance > 245.0,
        ) {
            (true, true, _) => {
                let v = (factor * 0.4 * 255.0).floor();
                (v, v, v)
            }
            (true, false, _) => {
                let ratio = (luminance + (255.0 - luminance) * factor * 0.4) / luminance;
                (
                    (r * ratio).min(255.0),
                    (g * ratio).min(255.0),
                    (b * ratio).min(255.0),
                )
            }
            (false, _, true) => {
                let v = (255.0 - factor * 0.4 * 255.0).floor();
                (v, v, v)
            }
            (false, _, false) => {
                let ratio = (luminance * (1.0 - factor * 0.4)) / luminance;
                (
                    (r * ratio).max(0.0),
                    (g * ratio).max(0.0),
                    (b * ratio).max(0.0),
                )
            }
        };
        Rgba::from_ints(nr.floor(), ng.floor(), nb.floor(), 255.0)
    })
}

/// Port of `generateMutedTextColor`.
pub fn generate_muted_text_color(bg: Rgba, is_dark: bool) -> Rgba {
    let lum = 0.299 * f64::from(bg.r) + 0.587 * f64::from(bg.g) + 0.114 * f64::from(bg.b);
    let value = match (is_dark, lum) {
        (true, lum) if lum < 10.0 => 180.0,
        (true, lum) => (160.0 + lum * 0.3).floor().min(200.0),
        (false, lum) if lum > 245.0 => 75.0,
        (false, lum) => (100.0 - (255.0 - lum) * 0.2).floor().max(60.0),
    };
    Rgba::from_ints(value, value, value, 255.0)
}
