//! Purpose: OAuth client registration and token endpoint calls for MCP (SDK auth.js port):
//! dynamic client registration, code exchange / refresh with client authentication method
//! selection, OAuth error parsing, PKCE authorization URL and WWW-Authenticate parsing.
//! Scope: stateless helpers used by oauth.rs and the remote transports.
//! Uses:
//! rust/crates/
//! |-- integration-adapters/src/mcp/oauth.rs
//! |-- integration-adapters/src/mcp/oauth_discovery.rs
//! `-- integration-adapters/src/mcp/oauth_provider.rs
//! External:
//! |-- base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD}
//! |-- reqwest::Response
//! |-- sha2::{Digest, Sha256}
//! `-- url::Url
//! Used by:
//! rust/crates/
//! |-- integration-adapters/src/mcp/oauth.rs
//! |-- integration-adapters/src/mcp/remote.rs
//! `-- integration-adapters/src/mcp/streamable.rs
//! Invariants: registration without a registration_endpoint fails with the SDK message that the
//! service classifies as needs_client_registration; PKCE uses S256 with a 43-char verifier.
//! Contract tests: integration-adapters/tests/mcp_oauth.rs

use base64::Engine;
use serde_json::{Map, Value};
use sha2::Digest;
use url::Url;

use super::oauth::{AuthError, AuthRequest};
use super::oauth_discovery::parse_url;
use super::oauth_provider::{ClientInformation, OAuthProvider, OAuthTokens, random_hex};

pub(super) async fn register_client(
    req: &AuthRequest<'_>,
    as_url: &Url,
    metadata: Option<&Value>,
    provider: &OAuthProvider,
    scope: Option<&str>,
) -> Result<ClientInformation, AuthError> {
    let url = match metadata {
        Some(m) => match m.get("registration_endpoint").and_then(Value::as_str) {
            Some(u) => parse_url(u)?,
            None => {
                return Err(AuthError::plain(
                    "Incompatible auth server: does not support dynamic client registration",
                ));
            }
        },
        None => as_url
            .join("/register")
            .map_err(|e| AuthError::plain(e.to_string()))?,
    };
    let mut body = provider.client_metadata();
    if let Some(scope) = scope {
        body["scope"] = Value::String(scope.into());
    }
    let mut b = req
        .http
        .post(url)
        .header("Content-Type", "application/json")
        .body(body.to_string());
    for (k, v) in req.headers {
        b = b.header(k, v);
    }
    let r = b
        .send()
        .await
        .map_err(|e| AuthError::plain(e.to_string()))?;
    if !r.status().is_success() {
        return Err(parse_error_response(r).await);
    }
    let v: Value = r
        .json()
        .await
        .map_err(|e| AuthError::plain(e.to_string()))?;
    let Some(id) = v.get("client_id").and_then(Value::as_str) else {
        return Err(AuthError::plain(
            "Invalid client registration response: missing client_id",
        ));
    };
    Ok(ClientInformation {
        client_id: id.into(),
        client_secret: v
            .get("client_secret")
            .and_then(Value::as_str)
            .map(str::to_string),
        client_id_issued_at: v.get("client_id_issued_at").and_then(Value::as_f64),
        client_secret_expires_at: v.get("client_secret_expires_at").and_then(Value::as_f64),
        token_endpoint_auth_method: v
            .get("token_endpoint_auth_method")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

pub(super) async fn token_request(
    req: &AuthRequest<'_>,
    as_url: &Url,
    metadata: Option<&Value>,
    params: &mut Vec<(String, String)>,
    client: &ClientInformation,
    resource: Option<&Url>,
) -> Result<OAuthTokens, AuthError> {
    let url = match metadata
        .and_then(|m| m.get("token_endpoint"))
        .and_then(Value::as_str)
    {
        Some(u) => parse_url(u)?,
        None => as_url
            .join("/token")
            .map_err(|e| AuthError::plain(e.to_string()))?,
    };
    if let Some(r) = resource {
        params.push(("resource".into(), r.to_string()));
    }
    let supported: Vec<&str> = metadata
        .and_then(|m| m.get("token_endpoint_auth_methods_supported"))
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let mut b = req.http.post(url).header("Accept", "application/json");
    for (k, v) in req.headers {
        b = b.header(k, v);
    }
    match select_auth_method(client, &supported) {
        "client_secret_basic" => {
            let Some(secret) = &client.client_secret else {
                return Err(AuthError::plain(
                    "client_secret_basic authentication requires a client_secret",
                ));
            };
            let creds = base64::engine::general_purpose::STANDARD
                .encode(format!("{}:{secret}", client.client_id));
            b = b.header("Authorization", format!("Basic {creds}"));
        }
        method => {
            params.push(("client_id".into(), client.client_id.clone()));
            if method == "client_secret_post"
                && let Some(s) = &client.client_secret
            {
                params.push(("client_secret".into(), s.clone()));
            }
        }
    }
    let r = b
        .form(&params)
        .send()
        .await
        .map_err(|e| AuthError::plain(e.to_string()))?;
    if !r.status().is_success() {
        return Err(parse_error_response(r).await);
    }
    let v: Map<String, Value> = r
        .json()
        .await
        .map_err(|e| AuthError::plain(e.to_string()))?;
    let (Some(access), Some(_)) = (
        v.get("access_token").and_then(Value::as_str),
        v.get("token_type").and_then(Value::as_str),
    ) else {
        return Err(AuthError::plain(
            "Invalid token response: access_token and token_type are required",
        ));
    };
    Ok(OAuthTokens {
        access_token: access.into(),
        refresh_token: v
            .get("refresh_token")
            .and_then(Value::as_str)
            .map(str::to_string),
        expires_in: v.get("expires_in").and_then(|e| {
            e.as_f64()
                .or_else(|| e.as_str().and_then(|s| s.parse().ok()))
        }),
        scope: v.get("scope").and_then(Value::as_str).map(str::to_string),
    })
}

/// SDK `selectClientAuthMethod`.
fn select_auth_method(client: &ClientInformation, supported: &[&str]) -> &'static str {
    let has_secret = client.client_secret.is_some();
    const KNOWN: [&str; 3] = ["client_secret_basic", "client_secret_post", "none"];
    if let Some(m) = client.token_endpoint_auth_method.as_deref()
        && let Some(k) = KNOWN.iter().find(|k| **k == m)
        && (supported.is_empty() || supported.contains(&m))
    {
        return k;
    }
    if supported.is_empty() {
        return if has_secret {
            "client_secret_basic"
        } else {
            "none"
        };
    }
    if has_secret && supported.contains(&"client_secret_basic") {
        return "client_secret_basic";
    }
    if has_secret && supported.contains(&"client_secret_post") {
        return "client_secret_post";
    }
    if supported.contains(&"none") {
        return "none";
    }
    if has_secret {
        "client_secret_post"
    } else {
        "none"
    }
}

async fn parse_error_response(r: reqwest::Response) -> AuthError {
    let status = r.status().as_u16();
    let body = r.text().await.unwrap_or_default();
    let parsed: Option<Value> = serde_json::from_str(&body).ok();
    match parsed
        .as_ref()
        .and_then(|v| v.get("error"))
        .and_then(Value::as_str)
    {
        Some(code) => AuthError {
            code: Some(code.to_string()),
            message: parsed
                .as_ref()
                .and_then(|v| v.get("error_description"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        },
        None => AuthError {
            code: Some("server_error".into()),
            message: format!(
                "HTTP {status}: Invalid OAuth error response: invalid error body. Raw body: {body}"
            ),
        },
    }
}

pub(super) fn authorization_url(
    as_url: &Url,
    metadata: Option<&Value>,
    client: &ClientInformation,
    provider: &OAuthProvider,
    state: &str,
    scope: Option<&str>,
    resource: Option<&Url>,
) -> Result<(Url, String), AuthError> {
    let mut url = match metadata {
        Some(m) => {
            let types = m.get("response_types_supported").and_then(Value::as_array);
            if !types.is_some_and(|t| t.iter().any(|v| v == "code")) {
                return Err(AuthError::plain(
                    "Incompatible auth server: does not support response type code",
                ));
            }
            if let Some(methods) = m
                .get("code_challenge_methods_supported")
                .and_then(Value::as_array)
                && !methods.iter().any(|v| v == "S256")
            {
                return Err(AuthError::plain(
                    "Incompatible auth server: does not support code challenge method S256",
                ));
            }
            parse_url(m["authorization_endpoint"].as_str().unwrap_or_default())?
        }
        None => as_url
            .join("/authorize")
            .map_err(|e| AuthError::plain(e.to_string()))?,
    };
    let (verifier, challenge) = pkce();
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("response_type", "code")
            .append_pair("client_id", &client.client_id)
            .append_pair("code_challenge", &challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("redirect_uri", &provider.redirect_url())
            .append_pair("state", state);
        if let Some(scope) = scope {
            q.append_pair("scope", scope);
            if scope.split(' ').any(|s| s == "offline_access") {
                q.append_pair("prompt", "consent");
            }
        }
        if let Some(r) = resource {
            q.append_pair("resource", r.as_str());
        }
    }
    Ok((url, verifier))
}

/// PKCE pair: 43-char unreserved-alphabet verifier (pkce-challenge default) and S256 challenge.
pub fn pkce() -> (String, String) {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";
    let hex = random_hex(43);
    let verifier: String = hex
        .as_bytes()
        .chunks(2)
        .map(|c| {
            let b = u8::from_str_radix(std::str::from_utf8(c).unwrap_or("00"), 16).unwrap_or(0);
            ALPHABET[usize::from(b) % ALPHABET.len()] as char
        })
        .collect();
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(sha2::Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

/// SDK `extractWWWAuthenticateParams`: (resource_metadata, scope, error) from a Bearer challenge.
pub fn www_authenticate_params(
    header: Option<&str>,
) -> (Option<String>, Option<String>, Option<String>) {
    let Some(h) = header else {
        return (None, None, None);
    };
    let mut parts = h.splitn(2, ' ');
    let (scheme, rest) = (parts.next().unwrap_or(""), parts.next());
    if !scheme.eq_ignore_ascii_case("bearer") || rest.is_none_or(str::is_empty) {
        return (None, None, None);
    }
    let field = |name: &str| -> Option<String> {
        let re = regex::Regex::new(&format!(r#"{name}=(?:"([^"]+)"|([^\s,]+))"#)).ok()?;
        let c = re.captures(h)?;
        c.get(1)
            .or_else(|| c.get(2))
            .map(|m| m.as_str().to_string())
    };
    let rm = field("resource_metadata").filter(|u| Url::parse(u).is_ok());
    (rm, field("scope"), field("error"))
}
