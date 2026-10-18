//! Purpose: 8-bit RGBA color values with the exact parsing and arithmetic semantics of
//! `@opentui/core` RGBA (`fromHex`, `fromInts`) and `packages/tui/src/theme/index.ts`
//! (`ansiToRgba`, `tint`, luminance checks).
//! Scope: pure values.
//! Uses:
//! `-- none
//! External:
//! `-- ratatui::style::Color
//! Used by:
//! rust/crates/
//! |-- ui-terminal/src/theme/mod.rs
//! |-- ui-terminal/src/theme/resolve.rs
//! `-- ui-terminal/src/theme/system.rs
//! Invariants: channels are stored as bytes like opentui's packed buffer, float accessors divide by 255.
//! Contract tests: ui-terminal/tests/theme.rs

use ratatui::style::Color;

/// RGBA color with byte channels. `a == 0` means fully transparent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rgba {
    /// Red channel.
    pub r: u8,
    /// Green channel.
    pub g: u8,
    /// Blue channel.
    pub b: u8,
    /// Alpha channel.
    pub a: u8,
}

/// Magenta fallback used by opentui for invalid hex strings.
pub const INVALID_HEX: Rgba = Rgba::new(255, 0, 255, 255);

impl Rgba {
    /// Build from byte channels.
    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// Opaque color from byte channels (`RGBA.fromInts(r, g, b)`).
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self::new(r, g, b, 255)
    }

    /// Fully transparent black (`transparent` / `none` in theme JSON).
    pub const fn transparent() -> Self {
        Self::new(0, 0, 0, 0)
    }

    /// `RGBA.fromInts` clamps and rounds each channel to a byte.
    pub fn from_ints(r: f64, g: f64, b: f64, a: f64) -> Self {
        Self::new(to_byte(r), to_byte(g), to_byte(b), to_byte(a))
    }

    /// Parse `#rgb`, `#rgba`, `#rrggbb` or `#rrggbbaa` (leading `#` optional). Invalid input
    /// yields opaque magenta like opentui's `hexToRgb`.
    pub fn from_hex(hex: &str) -> Self {
        Self::parse_hex(hex).unwrap_or(INVALID_HEX)
    }

    /// Strict variant of [`Rgba::from_hex`] returning `None` for invalid input.
    pub fn parse_hex(hex: &str) -> Option<Self> {
        let body = hex.strip_prefix('#').unwrap_or(hex);
        let expanded: String = match body.len() {
            3 | 4 => body.chars().flat_map(|c| [c, c]).collect(),
            _ => body.to_string(),
        };
        if !(expanded.len() == 6 || expanded.len() == 8)
            || !expanded.chars().all(|c| c.is_ascii_hexdigit())
        {
            return None;
        }
        let byte = |i: usize| u8::from_str_radix(&expanded[i..i + 2], 16).ok();
        let a = if expanded.len() == 8 { byte(6)? } else { 255 };
        Some(Self::new(byte(0)?, byte(2)?, byte(4)?, a))
    }

    /// Lowercase hex like opentui `rgbToHex`: alpha is omitted when opaque.
    pub fn to_hex(self) -> String {
        if self.a == 255 {
            return format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b);
        }
        format!("#{:02x}{:02x}{:02x}{:02x}", self.r, self.g, self.b, self.a)
    }

    /// Red as a 0..1 float.
    pub fn rf(self) -> f64 {
        f64::from(self.r) / 255.0
    }

    /// Green as a 0..1 float.
    pub fn gf(self) -> f64 {
        f64::from(self.g) / 255.0
    }

    /// Blue as a 0..1 float.
    pub fn bf(self) -> f64 {
        f64::from(self.b) / 255.0
    }

    /// Perceived luminance on the 0..1 scale (`0.299 r + 0.587 g + 0.114 b`).
    pub fn luminance(self) -> f64 {
        0.299 * self.rf() + 0.587 * self.gf() + 0.114 * self.bf()
    }

    /// Convert to a ratatui color. Fully transparent maps to `Color::Reset` (the terminal
    /// default), any other alpha is dropped because terminal cells cannot blend.
    pub fn to_color(self) -> Color {
        if self.a == 0 {
            return Color::Reset;
        }
        Color::Rgb(self.r, self.g, self.b)
    }
}

impl From<Rgba> for Color {
    fn from(value: Rgba) -> Self {
        value.to_color()
    }
}

fn to_byte(value: f64) -> u8 {
    if !value.is_finite() {
        return 0;
    }
    // Clamped to 0..=255 first, so the cast cannot truncate.
    value.clamp(0.0, 255.0).round() as u8
}

const ANSI_16: [&str; 16] = [
    "#000000", "#800000", "#008000", "#808000", "#000080", "#800080", "#008080", "#c0c0c0",
    "#808080", "#ff0000", "#00ff00", "#ffff00", "#0000ff", "#ff00ff", "#00ffff", "#ffffff",
];

/// Port of `ansiToRgba`: 16 standard colors, 6x6x6 cube, 24-step gray ramp, black otherwise.
pub fn ansi_to_rgba(code: f64) -> Rgba {
    if code < 16.0 {
        let hit = (code >= 0.0 && code.fract() == 0.0)
            .then(|| ANSI_16.get(code as usize))
            .flatten();
        return Rgba::from_hex(hit.copied().unwrap_or("#000000"));
    }
    if code < 232.0 {
        let index = code - 16.0;
        let b = index % 6.0;
        let g = (index / 6.0).floor() % 6.0;
        let r = (index / 36.0).floor();
        let val = |x: f64| if x == 0.0 { 0.0 } else { x * 40.0 + 55.0 };
        return Rgba::from_ints(val(r), val(g), val(b), 255.0);
    }
    if code < 256.0 {
        let gray = (code - 232.0) * 10.0 + 8.0;
        return Rgba::from_ints(gray, gray, gray, 255.0);
    }
    Rgba::rgb(0, 0, 0)
}

/// Port of `tint`: linear blend of `base` toward `overlay` by `alpha`, opaque result.
pub fn tint(base: Rgba, overlay: Rgba, alpha: f64) -> Rgba {
    let mix = |b: f64, o: f64| (b + (o - b) * alpha) * 255.0;
    Rgba::from_ints(
        mix(base.rf(), overlay.rf()).round(),
        mix(base.gf(), overlay.gf()).round(),
        mix(base.bf(), overlay.bf()).round(),
        255.0,
    )
}
