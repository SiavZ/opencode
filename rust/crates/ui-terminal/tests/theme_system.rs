//! Purpose: verify generated-system theme failures, fallback and cached refresh behavior.
//! Scope: deterministic terminal palette values, no OSC querying.
//! Uses:
//! rust/crates/
//! `-- ui-terminal/src/theme/mod.rs
//! External:
//! `-- none
//! Used by:
//! `-- none, entrypoint
//! Invariants: initial palette failure falls back to opencode, subsequent failure retains the system theme.
//! Contract tests: ui-terminal/tests/theme_system.rs

use opencode_ui_terminal::theme::{
    Mode, Rgba, SystemThemeError, TerminalColors, ThemeRegistry, generate_system, resolve_theme,
};

#[test]
fn missing_snapshots_fail_instead_of_inventing_system_background_or_text() {
    let mut colors = TerminalColors::default();
    assert_eq!(
        generate_system(&colors, Mode::Dark),
        Err(SystemThemeError::MissingBackground)
    );
    colors.palette.push(Some("#010203".into()));
    assert_eq!(
        generate_system(&colors, Mode::Dark),
        Err(SystemThemeError::MissingForeground)
    );
    colors.default_foreground = Some("#aabbcc".into());
    let resolved =
        resolve_theme(&generate_system(&colors, Mode::Dark).unwrap(), Mode::Dark).unwrap();
    assert_eq!(resolved.background, Rgba::new(1, 2, 3, 0));
    assert_eq!(resolved.text, Rgba::rgb(170, 187, 204));
}

#[test]
fn default_snapshots_override_palette_snapshots_and_missing_ansi_colors_fall_back() {
    let colors = TerminalColors {
        palette: vec![Some("#010203".into()); 8],
        default_background: Some("#202122".into()),
        default_foreground: Some("#ededed".into()),
    };
    let resolved =
        resolve_theme(&generate_system(&colors, Mode::Dark).unwrap(), Mode::Dark).unwrap();
    assert_eq!(resolved.background, Rgba::new(32, 33, 34, 0));
    assert_eq!(resolved.text, Rgba::rgb(237, 237, 237));
    assert_eq!(resolved.diff_highlight_added, Rgba::rgb(0, 255, 0));
    assert_eq!(resolved.diff_highlight_removed, Rgba::rgb(255, 0, 0));
}

#[test]
fn failed_initial_refresh_falls_back_and_failed_later_refresh_retains_prior_theme() {
    let mut registry = ThemeRegistry::with_defaults();
    let opencode = registry
        .resolve_selected("opencode", None, Mode::Dark)
        .unwrap();
    assert!(!registry.refresh_system(None, Mode::Dark).unwrap());
    assert!(
        !registry
            .refresh_system(Some(&TerminalColors::default()), Mode::Dark)
            .unwrap()
    );
    assert_eq!(
        registry
            .resolve_selected("system", None, Mode::Dark)
            .unwrap(),
        opencode
    );
    let mut colors = TerminalColors {
        palette: vec![Some("#202020".into())],
        default_background: None,
        default_foreground: Some("#ababab".into()),
    };
    assert!(registry.refresh_system(Some(&colors), Mode::Dark).unwrap());
    let system = registry.get("system").unwrap();
    assert!(!registry.refresh_system(None, Mode::Light).unwrap());
    assert_eq!(registry.get("system"), Some(system.clone()));
    colors.default_foreground = None;
    assert_eq!(
        registry.refresh_system(Some(&colors), Mode::Light),
        Err(SystemThemeError::MissingForeground)
    );
    assert_eq!(registry.get("system"), Some(system));
}

#[test]
fn palette_slot_zero_gate_matches_context_even_when_default_background_exists() {
    let mut registry = ThemeRegistry::with_defaults();
    let mut colors = TerminalColors {
        default_background: Some("#000000".into()),
        default_foreground: Some("#ffffff".into()),
        palette: vec![None],
    };
    assert!(!registry.refresh_system(Some(&colors), Mode::Dark).unwrap());
    assert!(!registry.has("system"));
    colors.palette[0] = Some(String::new());
    assert!(!registry.refresh_system(Some(&colors), Mode::Dark).unwrap());
    assert!(!registry.has("system"));
    colors.palette[0] = Some("#000000".into());
    assert!(registry.refresh_system(Some(&colors), Mode::Dark).unwrap());
    assert!(registry.has("system"));
}
