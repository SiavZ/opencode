//! Purpose: resolved terminal theme tokens, bundled theme assets and the public theme API
//! (resolution, system theme, registry, custom theme discovery).
//! Scope: pure values plus custom theme file reads in registry.rs.
//! Uses:
//! rust/crates/
//! |-- ui-terminal/src/theme/color.rs
//! |-- ui-terminal/src/theme/registry.rs
//! |-- ui-terminal/src/theme/resolve.rs
//! `-- ui-terminal/src/theme/system.rs
//! External:
//! `-- ratatui::style::Color
//! Used by:
//! rust/crates/
//! `-- ui-terminal/src/lib.rs
//! Invariants: token set matches packages/tui/src/theme/index.ts Theme, bundled list and order match DEFAULT_THEMES.
//! Contract tests: ui-terminal/tests/theme.rs

pub mod color;
pub mod registry;
pub mod resolve;
pub mod system;

pub use color::{Rgba, ansi_to_rgba, tint};
pub use registry::{
    DiscoverError, ThemeMap, ThemeRegistry, discover_themes, is_theme, theme_directories,
};
pub use resolve::{
    Mode, ResolvedTheme, THEME_KEYS, ThemeError, resolve_theme, selected_foreground,
};
pub use system::{SystemThemeError, TerminalColors, generate_system, terminal_mode};

/// Bundled theme JSON sources in baseline `DEFAULT_THEMES` insertion order.
pub const BUNDLED_THEMES: &[(&str, &str)] = &[
    ("aura", include_str!("../../assets/themes/aura.json")),
    ("ayu", include_str!("../../assets/themes/ayu.json")),
    (
        "catppuccin",
        include_str!("../../assets/themes/catppuccin.json"),
    ),
    (
        "catppuccin-frappe",
        include_str!("../../assets/themes/catppuccin-frappe.json"),
    ),
    (
        "catppuccin-macchiato",
        include_str!("../../assets/themes/catppuccin-macchiato.json"),
    ),
    ("cobalt2", include_str!("../../assets/themes/cobalt2.json")),
    ("cursor", include_str!("../../assets/themes/cursor.json")),
    ("dracula", include_str!("../../assets/themes/dracula.json")),
    (
        "everforest",
        include_str!("../../assets/themes/everforest.json"),
    ),
    ("flexoki", include_str!("../../assets/themes/flexoki.json")),
    ("github", include_str!("../../assets/themes/github.json")),
    ("gruvbox", include_str!("../../assets/themes/gruvbox.json")),
    (
        "kanagawa",
        include_str!("../../assets/themes/kanagawa.json"),
    ),
    (
        "material",
        include_str!("../../assets/themes/material.json"),
    ),
    ("matrix", include_str!("../../assets/themes/matrix.json")),
    ("mercury", include_str!("../../assets/themes/mercury.json")),
    ("monokai", include_str!("../../assets/themes/monokai.json")),
    (
        "nightowl",
        include_str!("../../assets/themes/nightowl.json"),
    ),
    ("nord", include_str!("../../assets/themes/nord.json")),
    (
        "one-dark",
        include_str!("../../assets/themes/one-dark.json"),
    ),
    (
        "osaka-jade",
        include_str!("../../assets/themes/osaka-jade.json"),
    ),
    (
        "opencode",
        include_str!("../../assets/themes/opencode.json"),
    ),
    ("orng", include_str!("../../assets/themes/orng.json")),
    (
        "lucent-orng",
        include_str!("../../assets/themes/lucent-orng.json"),
    ),
    (
        "palenight",
        include_str!("../../assets/themes/palenight.json"),
    ),
    (
        "rosepine",
        include_str!("../../assets/themes/rosepine.json"),
    ),
    (
        "solarized",
        include_str!("../../assets/themes/solarized.json"),
    ),
    (
        "synthwave84",
        include_str!("../../assets/themes/synthwave84.json"),
    ),
    (
        "tokyonight",
        include_str!("../../assets/themes/tokyonight.json"),
    ),
    ("vesper", include_str!("../../assets/themes/vesper.json")),
    ("vercel", include_str!("../../assets/themes/vercel.json")),
    ("zenburn", include_str!("../../assets/themes/zenburn.json")),
    (
        "carbonfox",
        include_str!("../../assets/themes/carbonfox.json"),
    ),
];

use ratatui::style::Color;

/// Resolved theme tokens, one field per `Theme` key in `packages/tui/src/theme/index.ts`.
/// Field order follows the baseline type. Convert full RGBA resolved values using
/// [`ResolvedTheme::to_theme`] before rendering.
#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    /// Baseline key `primary`.
    pub primary: Color,
    /// Baseline key `secondary`.
    pub secondary: Color,
    /// Baseline key `accent`.
    pub accent: Color,
    /// Baseline key `error`.
    pub error: Color,
    /// Baseline key `warning`.
    pub warning: Color,
    /// Baseline key `success`.
    pub success: Color,
    /// Baseline key `info`.
    pub info: Color,
    /// Baseline key `text`.
    pub text: Color,
    /// Baseline key `textMuted`.
    pub text_muted: Color,
    /// Baseline key `selectedListItemText`.
    pub selected_list_item_text: Color,
    /// Baseline key `background`.
    pub background: Color,
    /// Baseline key `backgroundPanel`.
    pub background_panel: Color,
    /// Baseline key `backgroundElement`.
    pub background_element: Color,
    /// Baseline key `backgroundMenu`.
    pub background_menu: Color,
    /// Baseline key `border`.
    pub border: Color,
    /// Baseline key `borderActive`.
    pub border_active: Color,
    /// Baseline key `borderSubtle`.
    pub border_subtle: Color,
    /// Baseline key `diffAdded`.
    pub diff_added: Color,
    /// Baseline key `diffRemoved`.
    pub diff_removed: Color,
    /// Baseline key `diffContext`.
    pub diff_context: Color,
    /// Baseline key `diffHunkHeader`.
    pub diff_hunk_header: Color,
    /// Baseline key `diffHighlightAdded`.
    pub diff_highlight_added: Color,
    /// Baseline key `diffHighlightRemoved`.
    pub diff_highlight_removed: Color,
    /// Baseline key `diffAddedBg`.
    pub diff_added_bg: Color,
    /// Baseline key `diffRemovedBg`.
    pub diff_removed_bg: Color,
    /// Baseline key `diffContextBg`.
    pub diff_context_bg: Color,
    /// Baseline key `diffLineNumber`.
    pub diff_line_number: Color,
    /// Baseline key `diffAddedLineNumberBg`.
    pub diff_added_line_number_bg: Color,
    /// Baseline key `diffRemovedLineNumberBg`.
    pub diff_removed_line_number_bg: Color,
    /// Baseline key `markdownText`.
    pub markdown_text: Color,
    /// Baseline key `markdownHeading`.
    pub markdown_heading: Color,
    /// Baseline key `markdownLink`.
    pub markdown_link: Color,
    /// Baseline key `markdownLinkText`.
    pub markdown_link_text: Color,
    /// Baseline key `markdownCode`.
    pub markdown_code: Color,
    /// Baseline key `markdownBlockQuote`.
    pub markdown_block_quote: Color,
    /// Baseline key `markdownEmph`.
    pub markdown_emph: Color,
    /// Baseline key `markdownStrong`.
    pub markdown_strong: Color,
    /// Baseline key `markdownHorizontalRule`.
    pub markdown_horizontal_rule: Color,
    /// Baseline key `markdownListItem`.
    pub markdown_list_item: Color,
    /// Baseline key `markdownListEnumeration`.
    pub markdown_list_enumeration: Color,
    /// Baseline key `markdownImage`.
    pub markdown_image: Color,
    /// Baseline key `markdownImageText`.
    pub markdown_image_text: Color,
    /// Baseline key `markdownCodeBlock`.
    pub markdown_code_block: Color,
    /// Baseline key `syntaxComment`.
    pub syntax_comment: Color,
    /// Baseline key `syntaxKeyword`.
    pub syntax_keyword: Color,
    /// Baseline key `syntaxFunction`.
    pub syntax_function: Color,
    /// Baseline key `syntaxVariable`.
    pub syntax_variable: Color,
    /// Baseline key `syntaxString`.
    pub syntax_string: Color,
    /// Baseline key `syntaxNumber`.
    pub syntax_number: Color,
    /// Baseline key `syntaxType`.
    pub syntax_type: Color,
    /// Baseline key `syntaxOperator`.
    pub syntax_operator: Color,
    /// Baseline key `syntaxPunctuation`.
    pub syntax_punctuation: Color,
    /// Baseline `thinkingOpacity`.
    pub thinking_opacity: f32,
}
