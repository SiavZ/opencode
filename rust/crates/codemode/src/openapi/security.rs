//! Purpose: OpenAPI security requirement and scheme parsing for generated operations.
//! Scope: codemode/src/openapi/spec.ts securityRequirements/operationSecurityRequirements/
//! securitySchemes.
//! Uses:
//! rust/crates/
//! |-- codemode/src/openapi/mod.rs
//! |-- codemode/src/openapi/spec.rs
//! `-- codemode/src/tools/render.rs
//! External:
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! `-- codemode/src/openapi/mod.rs
//! Invariants: requirements naming missing or cookie schemes never produce a callable tool.
//! Contract tests: codemode/tests/openapi.rs

use super::spec::{Parsed, non_empty, resolve};
use super::{SecurityRequirement, SecurityScheme};
use crate::tools::render::entries;
use serde_json::Value as Json;

pub(crate) fn security_requirements(value: Option<&Json>) -> Parsed<Vec<SecurityRequirement>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let Some(items) = value.as_array() else {
        return Err("security declaration is not an array".into());
    };
    items
        .iter()
        .map(|item| {
            let Some(map) = item.as_object() else {
                return Err("security requirement is not an object".to_string());
            };
            map.iter()
                .map(|(name, scopes)| {
                    let scopes = scopes
                        .as_array()
                        .ok_or("security requirement scopes are not string arrays")?;
                    let parsed: Vec<String> = scopes
                        .iter()
                        .filter_map(Json::as_str)
                        .map(str::to_string)
                        .collect();
                    if parsed.len() != scopes.len() {
                        return Err("security requirement scopes are not string arrays".to_string());
                    }
                    Ok((name.clone(), parsed))
                })
                .collect::<Result<Vec<_>, String>>()
        })
        .collect()
}

pub(crate) fn operation_security(
    value: Option<&Json>,
    defaults: &Parsed<Vec<SecurityRequirement>>,
    schemes: &[(String, SecurityScheme)],
) -> Parsed<Vec<SecurityRequirement>> {
    let parsed = match value {
        None => defaults.clone()?,
        Some(value) => security_requirements(Some(value))?,
    };
    let scheme = |name: &str| {
        schemes
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, scheme)| scheme)
    };
    let is_cookie = |scheme: Option<&SecurityScheme>| matches!(scheme, Some(SecurityScheme::ApiKey { location, .. }) if location == "cookie");
    let supported: Vec<SecurityRequirement> = parsed
        .iter()
        .filter(|requirement| {
            requirement
                .iter()
                .all(|(name, _)| scheme(name).is_some() && !is_cookie(scheme(name)))
        })
        .cloned()
        .collect();
    if parsed.is_empty() || !supported.is_empty() {
        return Ok(supported);
    }
    let mut names: Vec<String> = Vec::new();
    for (name, _) in parsed.iter().flatten() {
        if !names.contains(name) {
            names.push(name.clone());
        }
    }
    Err(match names.iter().find(|name| is_cookie(scheme(name))) {
        Some(cookie) => format!("cookie authentication '{cookie}' is not supported"),
        None => format!(
            "security requirement references missing or malformed scheme: {}",
            names.join(", ")
        ),
    })
}

pub(crate) fn security_schemes(document: &Json) -> Vec<(String, SecurityScheme)> {
    let declared = document
        .get("components")
        .and_then(|components| components.get("securitySchemes"))
        .and_then(Json::as_object);
    let Some(declared) = declared else {
        return Vec::new();
    };
    entries(declared)
        .into_iter()
        .filter_map(|(name, value)| {
            let resolved = resolve(document, value);
            let map = resolved.as_object()?;
            let scheme = match non_empty(map.get("type"))?.as_str() {
                "apiKey" => {
                    let location = non_empty(map.get("in"))?;
                    let parameter = non_empty(map.get("name"))?;
                    if !["header", "query", "cookie"].contains(&location.as_str()) {
                        return None;
                    }
                    SecurityScheme::ApiKey {
                        name: parameter,
                        location,
                    }
                }
                "http" => SecurityScheme::Http {
                    scheme: non_empty(map.get("scheme"))?.to_lowercase(),
                },
                "oauth2" => SecurityScheme::OAuth2,
                "openIdConnect" => SecurityScheme::OpenIdConnect,
                _ => return None,
            };
            Some((name.clone(), scheme))
        })
        .collect()
}
