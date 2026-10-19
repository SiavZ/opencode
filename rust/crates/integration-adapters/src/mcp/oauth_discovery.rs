//! Purpose: OAuth metadata discovery for MCP (SDK auth.js port): RFC 9728 protected-resource
//! metadata with path-aware then root fallback, RFC 8414 / OIDC authorization-server metadata,
//! resource URL selection and scope determination.
//! Scope: stateless helpers used by oauth.rs.
//! Uses:
//! rust/crates/
//! |-- integration-adapters/src/mcp/oauth.rs
//! `-- integration-adapters/src/mcp/oauth_provider.rs
//! External:
//! |-- reqwest::Response
//! |-- serde_json::Value
//! |-- str::to_string
//! `-- url::Url
//! Used by:
//! rust/crates/
//! |-- integration-adapters/src/mcp/mod.rs
//! |-- integration-adapters/src/mcp/oauth.rs
//! `-- integration-adapters/src/mcp/oauth_token.rs
//! Invariants: 4xx on a discovery candidate moves to the next candidate, 5xx fails; resource
//! metadata must name a resource on the same origin and a path prefix of the server URL.
//! Contract tests: integration-adapters/tests/mcp_oauth.rs

use serde_json::Value;
use url::Url;

use super::oauth::{AuthError, AuthRequest, PROTOCOL_VERSION};
use super::oauth_provider::OAuthProvider;

pub(super) fn parse_url(s: &str) -> Result<Url, AuthError> {
    Url::parse(s).map_err(|_| AuthError::plain(format!("Invalid URL: {s}")))
}

fn well_known(kind: &str, pathname: &str) -> String {
    format!(
        "/.well-known/{kind}{}",
        pathname.strip_suffix('/').unwrap_or(pathname)
    )
}

async fn get(req: &AuthRequest<'_>, url: &Url, accept_json: bool) -> Option<reqwest::Response> {
    let mut b = req
        .http
        .get(url.clone())
        .header("MCP-Protocol-Version", PROTOCOL_VERSION);
    if accept_json {
        b = b.header("Accept", "application/json");
    }
    for (k, v) in req.headers {
        b = b.header(k, v);
    }
    b.send().await.ok()
}

pub(super) async fn discover_protected_resource(
    req: &AuthRequest<'_>,
    server: &Url,
) -> Result<Value, AuthError> {
    let response = match req.resource_metadata_url {
        Some(u) => get(req, &parse_url(u)?, false).await,
        None => {
            let mut url = server
                .join(&well_known("oauth-protected-resource", server.path()))
                .map_err(|e| AuthError::plain(e.to_string()))?;
            url.set_query(server.query());
            let first = get(req, &url, false).await;
            let fallback =
                first.as_ref().is_none_or(|r| r.status().is_client_error()) && server.path() != "/";
            if fallback {
                let root = server
                    .join("/.well-known/oauth-protected-resource")
                    .map_err(|e| AuthError::plain(e.to_string()))?;
                get(req, &root, false).await
            } else {
                first
            }
        }
    };
    let Some(r) = response.filter(|r| r.status() != 404) else {
        return Err(AuthError::plain(
            "Resource server does not implement OAuth 2.0 Protected Resource Metadata.",
        ));
    };
    if !r.status().is_success() {
        return Err(AuthError::plain(format!(
            "HTTP {} trying to load well-known OAuth protected resource metadata.",
            r.status().as_u16()
        )));
    }
    let v: Value = r
        .json()
        .await
        .map_err(|e| AuthError::plain(e.to_string()))?;
    if !v.get("resource").is_some_and(Value::is_string) {
        return Err(AuthError::plain(
            "Invalid protected resource metadata: resource",
        ));
    }
    Ok(v)
}

pub(super) async fn discover_as_metadata(
    req: &AuthRequest<'_>,
    as_url: &Url,
) -> Result<Option<Value>, AuthError> {
    let path = as_url
        .path()
        .strip_suffix('/')
        .unwrap_or(as_url.path())
        .to_string();
    let candidates: Vec<(String, &str)> = if as_url.path() == "/" {
        vec![
            ("/.well-known/oauth-authorization-server".into(), "oauth"),
            ("/.well-known/openid-configuration".into(), "oidc"),
        ]
    } else {
        vec![
            (
                format!("/.well-known/oauth-authorization-server{path}"),
                "oauth",
            ),
            (format!("/.well-known/openid-configuration{path}"), "oidc"),
            (format!("{path}/.well-known/openid-configuration"), "oidc"),
        ]
    };
    for (p, kind) in candidates {
        let url = as_url
            .join(&p)
            .map_err(|e| AuthError::plain(e.to_string()))?;
        let Some(r) = get(req, &url, true).await else {
            continue;
        };
        if !r.status().is_success() {
            if r.status().is_client_error() {
                continue;
            }
            let label = if kind == "oauth" {
                "OAuth"
            } else {
                "OpenID provider"
            };
            return Err(AuthError::plain(format!(
                "HTTP {} trying to load {label} metadata from {url}",
                r.status().as_u16()
            )));
        }
        let v: Value = r
            .json()
            .await
            .map_err(|e| AuthError::plain(e.to_string()))?;
        for key in ["issuer", "authorization_endpoint", "token_endpoint"] {
            if !v.get(key).is_some_and(Value::is_string) {
                return Err(AuthError::plain(format!(
                    "Invalid OAuth metadata: missing {key}"
                )));
            }
        }
        return Ok(Some(v));
    }
    Ok(None)
}

pub(super) fn select_resource(
    server: &Url,
    meta: Option<&Value>,
) -> Result<Option<Url>, AuthError> {
    let Some(meta) = meta else { return Ok(None) };
    let mut requested = server.clone();
    requested.set_fragment(None);
    let configured = parse_url(meta["resource"].as_str().unwrap_or_default())?;
    let with_slash = |p: &str| {
        if p.ends_with('/') {
            p.to_string()
        } else {
            format!("{p}/")
        }
    };
    let allowed = requested.origin() == configured.origin()
        && requested.path().len() >= configured.path().len()
        && with_slash(requested.path()).starts_with(&with_slash(configured.path()));
    if !allowed {
        return Err(AuthError::plain(format!(
            "Protected resource {} does not match expected {requested} (or origin)",
            meta["resource"].as_str().unwrap_or_default()
        )));
    }
    Ok(Some(configured))
}

pub(super) fn determine_scope(
    requested: Option<&str>,
    rm: Option<&Value>,
    md: Option<&Value>,
    p: &OAuthProvider,
) -> Option<String> {
    let from_rm = rm
        .and_then(|m| m.get("scopes_supported"))
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(" ")
        })
        .filter(|s| !s.is_empty());
    let scope = requested
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or(from_rm)
        .or_else(|| p.config.scope.clone())?;
    let offline = md
        .and_then(|m| m.get("scopes_supported"))
        .and_then(Value::as_array)
        .is_some_and(|a| a.iter().any(|s| s == "offline_access"));
    if offline && !scope.split(' ').any(|s| s == "offline_access") {
        return Some(format!("{scope} offline_access"));
    }
    Some(scope)
}
