//! Purpose: contract tests for theme resolution against a fixture captured from the TypeScript
//! baseline (`resolveTheme`, `selectedForeground`, `generateSystem`, `terminalMode`) plus the
//! registry and custom-theme discovery rules from packages/tui/test/theme.test.ts.
//! Scope: fixture-based (tests/fixtures/theme_resolved.json, captured with bun from packages/tui).
//! Uses:
//! rust/crates/
//! `-- ui-terminal/src/theme/mod.rs
//! External:
//! |-- serde_json
//! `-- tempfile
//! Used by:
//! `-- none, entrypoint
//! Invariants: every bundled theme in both modes matches the baseline hex for every token.
//! Contract tests: ui-terminal/tests/theme.rs

use opencode_ui_terminal::theme::{
    BUNDLED_THEMES, Mode, Rgba, THEME_KEYS, TerminalColors, ThemeError, ThemeRegistry,
    discover_themes, generate_system, resolve_theme, selected_foreground, terminal_mode,
    theme_directories,
};
use serde_json::{Value, json};

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/theme_resolved.json")).expect("fixture json")
}

fn assert_row(
    label: &str,
    resolved: &opencode_ui_terminal::theme::ResolvedTheme,
    expected: &Value,
) {
    let actual = resolved_hex(resolved);
    for key in THEME_KEYS {
        assert_eq!(
            actual.get(*key).map(String::as_str),
            expected[*key].as_str(),
            "{label} {key}"
        );
    }
    assert_eq!(
        resolved.has_selected_list_item_text,
        expected["_has"].as_bool().unwrap(),
        "{label} _has"
    );
    assert!(
        (resolved.thinking_opacity - expected["thinkingOpacity"].as_f64().unwrap()).abs() < 1e-6,
        "{label} opacity"
    );
    assert_eq!(
        selected_foreground(resolved, None).to_hex(),
        expected["selectedFg"].as_str().unwrap(),
        "{label} selectedFg"
    );
}

fn resolved_hex(
    theme: &opencode_ui_terminal::theme::ResolvedTheme,
) -> std::collections::HashMap<String, String> {
    THEME_KEYS
        .iter()
        .map(|key| ((*key).to_string(), token(theme, key).to_hex()))
        .collect()
}

fn token(theme: &opencode_ui_terminal::theme::ResolvedTheme, key: &str) -> Rgba {
    theme
        .get(key)
        .unwrap_or_else(|| panic!("unknown key {key}"))
}

#[test]
fn bundled_order_matches_default_themes() {
    let expected: Vec<String> = serde_json::from_value(fixture()["order"].clone()).unwrap();
    let actual: Vec<String> = BUNDLED_THEMES
        .iter()
        .map(|(name, _)| (*name).to_string())
        .collect();
    assert_eq!(actual, expected);
    let registry = ThemeRegistry::with_defaults();
    assert_eq!(registry.all().keys().cloned().collect::<Vec<_>>(), expected);
}

#[test]
fn every_bundled_theme_resolves_like_baseline_in_both_modes() {
    let fixture = fixture();
    let registry = ThemeRegistry::with_defaults();
    for (name, _) in BUNDLED_THEMES {
        for mode in [Mode::Dark, Mode::Light] {
            let label = format!("{name}:{}", mode.as_str());
            let resolved = resolve_theme(&registry.get(name).unwrap(), mode).unwrap();
            assert_row(&label, &resolved, &fixture["themes"][&label]);
        }
    }
}

#[test]
fn spot_check_known_colors() {
    let registry = ThemeRegistry::with_defaults();
    let opencode = resolve_theme(&registry.get("opencode").unwrap(), Mode::Dark).unwrap();
    assert_eq!(opencode.primary.to_hex(), "#fab283");
    assert_eq!(opencode.background.to_hex(), "#0a0a0a");
    let light = resolve_theme(&registry.get("opencode").unwrap(), Mode::Light).unwrap();
    assert_eq!(light.background.to_hex(), "#ffffff");
}

#[test]
fn system_theme_matches_baseline() {
    let fixture = fixture();
    let palette =
        |items: &[Option<&str>]| items.iter().map(|item| item.map(str::to_string)).collect();
    let dark = TerminalColors {
        palette: palette(&[
            Some("#1d1f21"),
            Some("#cc6666"),
            Some("#b5bd68"),
            Some("#f0c674"),
            Some("#81a2be"),
            Some("#b294bb"),
            Some("#8abeb7"),
            Some("#c5c8c6"),
            None,
            Some("#d54e53"),
            Some("#b9ca4a"),
        ]),
        default_foreground: Some("#c5c8c6".into()),
        default_background: Some("#1d1f21".into()),
    };
    let light = TerminalColors {
        palette: palette(&[
            Some("#fdf6e3"),
            Some("#dc322f"),
            Some("#859900"),
            Some("#b58900"),
            Some("#268bd2"),
            Some("#d33682"),
            Some("#2aa198"),
            Some("#073642"),
        ]),
        default_foreground: None,
        default_background: Some("#fdf6e3".into()),
    };
    for (label, colors) in [("dark", dark), ("light", light)] {
        let expected = &fixture["system"][label];
        let mode = terminal_mode(&colors).unwrap_or(Mode::Dark);
        assert_eq!(mode.as_str(), expected["mode"].as_str().unwrap());
        let resolved = resolve_theme(&generate_system(&colors, mode).unwrap(), mode).unwrap();
        assert_eq!(resolved.background.a, 0, "system background is transparent");
        assert_row(&format!("system:{label}"), &resolved, expected);
    }
}

#[test]
fn terminal_mode_rules() {
    let bg = |hex: Option<&str>, slot0: Option<&str>| TerminalColors {
        palette: vec![slot0.map(str::to_string)],
        default_foreground: None,
        default_background: hex.map(str::to_string),
    };
    assert_eq!(terminal_mode(&bg(Some("#fbf1c7"), None)), Some(Mode::Light));
    assert_eq!(terminal_mode(&bg(Some("#1a1b26"), None)), Some(Mode::Dark));
    assert_eq!(terminal_mode(&bg(None, Some("#000000"))), None);
}

#[test]
fn circular_and_missing_references_fail_like_baseline() {
    let registry = ThemeRegistry::with_defaults();
    let mut item = registry.get("opencode").unwrap();
    item["defs"]["one"] = json!("two");
    item["defs"]["two"] = json!("one");
    item["theme"]["primary"] = json!("one");
    let error = resolve_theme(&item, Mode::Dark).unwrap_err();
    assert_eq!(error, ThemeError::Circular("one -> two -> one".into()));
    assert!(error.to_string().starts_with("Circular color reference"));

    item["theme"]["primary"] = json!("nope");
    let error = resolve_theme(&item, Mode::Dark).unwrap_err();
    assert_eq!(
        error.to_string(),
        "Color reference \"nope\" not found in defs or theme"
    );
}

#[test]
fn ansi_codes_and_transparent_values() {
    let registry = ThemeRegistry::with_defaults();
    let mut item = registry.get("opencode").unwrap();
    item["theme"]["primary"] = json!(9);
    item["theme"]["secondary"] = json!(196);
    item["theme"]["accent"] = json!(244);
    item["theme"]["background"] = json!("none");
    let resolved = resolve_theme(&item, Mode::Dark).unwrap();
    assert_eq!(resolved.primary.to_hex(), "#ff0000");
    assert_eq!(resolved.secondary.to_hex(), "#ff0000");
    assert_eq!(resolved.accent.to_hex(), "#808080");
    assert_eq!(resolved.background, Rgba::transparent());
    // Transparent background without explicit selectedListItemText contrasts against primary.
    assert_eq!(selected_foreground(&resolved, None).to_hex(), "#ffffff");
    assert_eq!(
        selected_foreground(&resolved, Some(Rgba::rgb(250, 250, 250))).to_hex(),
        "#000000"
    );
}

#[test]
fn registry_add_upsert_and_has() {
    let mut registry = ThemeRegistry::with_defaults();
    let base = registry.get("opencode").unwrap();
    let mut one = base.clone();
    one["theme"]["primary"] = json!("#101010");
    let mut two = base.clone();
    two["theme"]["primary"] = json!("#fefefe");
    assert!(!registry.has("plugin-theme"));
    assert!(registry.add("plugin-theme", one));
    assert!(!registry.add("plugin-theme", two.clone()));
    assert_eq!(
        registry.all()["plugin-theme"]["theme"]["primary"],
        json!("#101010")
    );
    assert!(!registry.add("invalid", json!({ "defs": { "a": "#ffffff" } })));
    assert!(!registry.has("invalid"));
    assert!(!registry.add("", base.clone()));
    assert!(registry.upsert("plugin-theme", two));
    assert_eq!(
        registry.all()["plugin-theme"]["theme"]["primary"],
        json!("#fefefe")
    );
    assert!(!registry.has("system"));
    registry.set_system(Some(base));
    assert_eq!(
        registry.all().keys().next_back().map(String::as_str),
        Some("system")
    );
}

#[test]
fn selection_falls_back_to_saved_then_opencode() {
    let registry = ThemeRegistry::with_defaults();
    let dracula = resolve_theme(&registry.get("dracula").unwrap(), Mode::Dark).unwrap();
    let opencode = resolve_theme(&registry.get("opencode").unwrap(), Mode::Dark).unwrap();
    assert_eq!(
        registry
            .resolve_selected("missing", Some("dracula"), Mode::Dark)
            .unwrap(),
        dracula
    );
    assert_eq!(
        registry
            .resolve_selected("missing", Some("gone"), Mode::Dark)
            .unwrap(),
        opencode
    );
    assert_eq!(
        registry
            .resolve_selected("system", None, Mode::Dark)
            .unwrap(),
        opencode
    );
}

#[test]
fn custom_theme_precedence_follows_directory_order() {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path().join("global");
    let project = tmp.path().join("project");
    for dir in [&global, &project] {
        std::fs::create_dir_all(dir.join("themes")).unwrap();
    }
    std::fs::write(global.join("themes/custom.json"), r#"{"source":"global"}"#).unwrap();
    std::fs::write(
        project.join("themes/custom.json"),
        r#"{"source":"project"}"#,
    )
    .unwrap();
    std::fs::write(project.join("themes/.hidden.json"), r#"{"source":"dot"}"#).unwrap();
    let found = discover_themes(&[global.clone(), project.clone()]).unwrap();
    assert_eq!(found["custom"], json!({ "source": "project" }));
    assert_eq!(found[".hidden"], json!({ "source": "dot" }));

    std::fs::write(project.join("themes/broken.json"), "{").unwrap();
    assert!(discover_themes(&[global, project]).is_err());
}

#[test]
fn custom_themes_override_bundled_and_filter_non_themes() {
    let mut registry = ThemeRegistry::with_defaults();
    let mut custom = serde_json::Map::new();
    let mut theme = registry.get("opencode").unwrap();
    theme["theme"]["primary"] = json!("#123456");
    custom.insert("dracula".into(), theme);
    custom.insert("junk".into(), json!({ "source": "x" }));
    registry.set_custom(custom);
    assert!(!registry.has("junk"));
    let resolved = registry
        .resolve_selected("dracula", None, Mode::Dark)
        .unwrap();
    assert_eq!(resolved.primary.to_hex(), "#123456");
}

#[test]
fn theme_directories_walk_ancestors() {
    let dirs = theme_directories(std::path::Path::new("/cfg"), std::path::Path::new("/a/b"));
    let dirs: Vec<String> = dirs
        .iter()
        .map(|dir| dir.to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        dirs,
        ["/cfg", "/a/b/.opencode", "/a/.opencode", "/.opencode"]
    );
}

#[test]
fn to_theme_maps_transparent_to_reset() {
    let registry = ThemeRegistry::with_defaults();
    let mut item = registry.get("opencode").unwrap();
    item["theme"]["background"] = json!("transparent");
    let theme = resolve_theme(&item, Mode::Dark).unwrap().to_theme();
    assert_eq!(theme.background, ratatui::style::Color::Reset);
    assert_eq!(theme.primary, ratatui::style::Color::Rgb(0xfa, 0xb2, 0x83));
}
