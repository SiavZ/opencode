//! Purpose: reject malformed startup configuration while admitting genuine V1 and V2 documents.
//! Scope: explicit global/project paths and environment snapshot, without mutating source files.
//! Uses: config/src/current.rs, config/src/legacy/parse.rs, config/src/schema/mod.rs
//! Used by: opencode/src/cli/commands/mod.rs
//! Invariants: canonical permission rules are validated with V2, never lowered through V1 first.
//! Contract tests: opencode/tests/admin/permissions.rs, opencode/tests/admin/config.rs

use opencode_config::legacy::parse::{Diagnostics, decode_config, parse_jsonc};
use opencode_config::variable::{Missing, Source, substitute};
use opencode_config::{ConfigError, Environment, fs, migrate, schema};
use std::path::Path;

pub fn validate(env: &Environment, directory: &Path) -> Result<(), ConfigError> {
    let mut paths = Vec::new();
    for directory in [env.global_config.clone(), env.current_global_config()] {
        paths.extend(
            ["config.json", "opencode.json", "opencode.jsonc"].map(|name| directory.join(name)),
        );
    }
    paths.extend(env.opencode_config());
    if !env.disable_project_config() {
        let boundary = directory
            .ancestors()
            .find(|path| path.join(".git").exists())
            .unwrap_or(directory);
        paths.extend(fs::up(
            &[
                "opencode.json",
                "opencode.jsonc",
                ".opencode/opencode.json",
                ".opencode/opencode.jsonc",
            ],
            directory,
            Some(boundary),
        ));
    }
    paths.sort();
    paths.dedup();
    for path in paths {
        let Some(text) = fs::read_safe(&path)?.filter(|text| !text.is_empty()) else {
            continue;
        };
        validate_text(&text, &Source::Path(path), env)?;
    }
    if let Some(text) = env.opencode_config_content() {
        validate_text(
            text,
            &Source::Virtual {
                source: "OPENCODE_CONFIG_CONTENT".into(),
                dir: directory.to_path_buf(),
            },
            env,
        )?;
    }
    Ok(())
}

fn validate_text(text: &str, source: &Source, env: &Environment) -> Result<(), ConfigError> {
    let text = substitute(text, source, env, None, Missing::Error)?;
    let input = parse_jsonc(&text, &source.name())?;
    if migrate::is_v1(&input)
        || input
            .get("mcp")
            .and_then(serde_json::Value::as_object)
            .is_some_and(|servers| servers.values().any(|server| server.get("type").is_some()))
    {
        decode_config(&input, &source.name(), &mut Diagnostics::default())?;
        return Ok(());
    }
    schema::decode(schema::v2::info(), &input).map_err(|issues| ConfigError::Invalid {
        path: source.name(),
        issues,
        message: None,
    })?;
    Ok(())
}
