//! Purpose: regression coverage for theme reference precedence, fallback and registry edge cases.
//! Scope: pure theme resolution.
//! Uses:
//! rust/crates/
//! `-- ui-terminal/src/theme/mod.rs
//! External:
//! |-- serde_json::Map
//! |-- serde_json::Value
//! `-- serde_json::json
//! Used by:
//! `-- none, entrypoint
//! Invariants: optional tokens and null defs follow TypeScript fallback rules.
//! Contract tests: ui-terminal/tests/theme_edges.rs

use opencode_ui_terminal::theme::{
    Mode, Rgba, TerminalColors, ThemeError, ThemeRegistry, generate_system, resolve_theme,
    selected_foreground, terminal_mode,
};
use serde_json::{Map, Value, json};

#[test]
fn defs_precede_tokens_but_null_defs_fall_back_to_tokens() {
    let registry = ThemeRegistry::with_defaults();
    let mut raw = registry.get("opencode").unwrap();
    raw["theme"]["primary"] = json!("text");
    raw["defs"]["text"] = json!("#123456");
    assert_eq!(
        resolve_theme(&raw, Mode::Dark).unwrap().primary,
        Rgba::rgb(18, 52, 86)
    );
    raw["defs"]["text"] = Value::Null;
    let resolved = resolve_theme(&raw, Mode::Dark).unwrap();
    assert_eq!(resolved.primary, resolved.text);
}

#[test]
fn variant_references_follow_the_selected_mode() {
    let registry = ThemeRegistry::with_defaults();
    let mut raw = registry.get("opencode").unwrap();
    raw["defs"]["alias"] = json!({"dark": "darkAlias", "light": "lightAlias"});
    raw["defs"]["darkAlias"] = json!("#010203");
    raw["defs"]["lightAlias"] = json!("#fafbfc");
    raw["theme"]["primary"] = json!("alias");
    assert_eq!(
        resolve_theme(&raw, Mode::Dark).unwrap().primary,
        Rgba::rgb(1, 2, 3)
    );
    assert_eq!(
        resolve_theme(&raw, Mode::Light).unwrap().primary,
        Rgba::rgb(250, 251, 252)
    );
}

#[test]
fn optional_defaults_and_explicit_selected_foreground_are_distinct() {
    let registry = ThemeRegistry::with_defaults();
    let mut raw = registry.get("opencode").unwrap();
    let colors = raw["theme"].as_object_mut().unwrap();
    colors.remove("selectedListItemText");
    colors.remove("backgroundMenu");
    colors.remove("thinkingOpacity");
    let resolved = resolve_theme(&raw, Mode::Dark).unwrap();
    assert!(!resolved.has_selected_list_item_text);
    assert_eq!(resolved.selected_list_item_text, resolved.background);
    assert_eq!(resolved.background_menu, resolved.background_element);
    assert_eq!(resolved.thinking_opacity, 0.6);
    assert_eq!(
        selected_foreground(&resolved, Some(Rgba::rgb(255, 255, 255))),
        resolved.background
    );
    raw["theme"]["background"] = json!("transparent");
    raw["theme"]["selectedListItemText"] = json!("#12345678");
    raw["theme"]["backgroundMenu"] = json!("#abcdef");
    raw["theme"]["thinkingOpacity"] = json!(0.0);
    let resolved = resolve_theme(&raw, Mode::Dark).unwrap();
    assert!(resolved.has_selected_list_item_text);
    assert_eq!(
        selected_foreground(&resolved, None),
        Rgba::new(18, 52, 86, 120)
    );
    assert_eq!(resolved.background_menu, Rgba::rgb(171, 205, 239));
    assert_eq!(resolved.thinking_opacity, 0.0);
}

#[test]
fn custom_system_name_survives_without_a_generated_palette() {
    let mut registry = ThemeRegistry::with_defaults();
    let base = registry.get("opencode").unwrap();
    assert!(registry.add("system", base.clone()));
    assert!(registry.has("system"));
    assert_eq!(registry.get("system"), Some(base.clone()));
    let mut custom = base.clone();
    custom["theme"]["primary"] = json!("#010203");
    let mut layer = Map::new();
    layer.insert("system".into(), custom.clone());
    registry.set_custom(layer);
    assert_eq!(registry.get("system"), Some(custom.clone()));
    registry.set_system(Some(base.clone()));
    assert_eq!(registry.get("system"), Some(base));
    registry.set_system(None);
    assert_eq!(registry.get("system"), Some(custom));
    assert_eq!(
        registry.all().get("system"),
        registry.get("system").as_ref()
    );
}

#[test]
fn upserting_a_custom_theme_preserves_precedence_and_insertion_order() {
    let mut registry = ThemeRegistry::with_defaults();
    let mut base = registry.get("opencode").unwrap();
    let mut custom = Map::new();
    custom.insert("custom".into(), base.clone());
    registry.set_custom(custom);
    let before = registry.all().keys().cloned().collect::<Vec<_>>();
    base["theme"]["primary"] = json!("#012345");
    assert!(registry.upsert("custom", base.clone()));
    assert_eq!(registry.get("custom"), Some(base));
    assert_eq!(registry.all().keys().cloned().collect::<Vec<_>>(), before);
}

#[test]
fn empty_palette_colors_use_ansi_and_empty_background_has_no_mode() {
    let colors = TerminalColors {
        palette: vec![Some("#000000".into()), Some(String::new())],
        default_foreground: Some("#ffffff".into()),
        default_background: Some(String::new()),
    };
    assert_eq!(terminal_mode(&colors), None);
    let theme = resolve_theme(&generate_system(&colors, Mode::Dark).unwrap(), Mode::Dark).unwrap();
    assert_eq!(theme.error, Rgba::rgb(128, 0, 0));
    assert_eq!(theme.info, Rgba::rgb(0, 128, 128));
}

#[test]
fn malformed_theme_values_report_typed_errors() {
    assert!(matches!(
        resolve_theme(&json!({"theme": []}), Mode::Dark),
        Err(ThemeError::Invalid(_))
    ));
    let mut raw = ThemeRegistry::with_defaults().get("opencode").unwrap();
    raw["theme"]["primary"] = json!(false);
    assert_eq!(
        resolve_theme(&raw, Mode::Dark).unwrap_err(),
        ThemeError::Invalid("primary".into())
    );
    raw["theme"]["primary"] = json!({"light": "#ffffff"});
    assert_eq!(
        resolve_theme(&raw, Mode::Dark).unwrap_err(),
        ThemeError::Invalid("primary".into())
    );
}

#[test]
fn hex_parsing_matches_opentui_and_terminal_alpha_boundary() {
    assert_eq!(Rgba::from_hex("#abc"), Rgba::rgb(170, 187, 204));
    assert_eq!(Rgba::from_hex("#abcd"), Rgba::new(170, 187, 204, 221));
    assert_eq!(Rgba::from_hex("bad-value"), Rgba::rgb(255, 0, 255));
    assert_eq!(
        Rgba::from_ints(-1.0, 10.5, 999.0, f64::NAN),
        Rgba::new(0, 11, 255, 0)
    );
    assert_eq!(
        Rgba::new(1, 2, 3, 0).to_color(),
        ratatui::style::Color::Reset
    );
    assert_eq!(
        Rgba::new(1, 2, 3, 1).to_color(),
        ratatui::style::Color::Rgb(1, 2, 3)
    );
}

#[test]
fn opacity_preserves_json_precision_until_rendering_conversion() {
    let mut raw = ThemeRegistry::with_defaults().get("opencode").unwrap();
    let opacity = 0.3333333333333333;
    raw["theme"]["thinkingOpacity"] = json!(opacity);
    let resolved = resolve_theme(&raw, Mode::Dark).unwrap();
    assert_eq!(resolved.thinking_opacity, opacity);
    assert_eq!(resolved.to_theme().thinking_opacity, opacity as f32);
}
