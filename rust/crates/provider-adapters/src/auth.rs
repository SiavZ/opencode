//! Purpose: route-level request authentication (packages/llm/src/route/auth.ts): bearer,
//! named header, static headers, header removal, `and_then`/`or_else` composition and the
//! narrow `CredentialSource` port that provider auth (keys, OAuth leases) plugs into.
//! Scope: async trait objects. No credential persistence (Phase 02) and no OAuth flows here.
//! Uses:
//! rust/crates/
//! `-- llm-api/src/lib.rs
//! External:
//! |-- futures::future::BoxFuture
//! `-- indexmap::IndexMap
//! Used by:
//! rust/crates/
//! |-- provider-adapters/src/lib.rs
//! `-- provider-adapters/src/route.rs
//! Invariants: header names are lowercased (effect `Headers` semantics). A missing or empty
//! secret is `Authentication { kind: missing }` with message `Missing auth credential: <source>`.
//! Secrets never appear in `Debug` output.
//! Contract tests: provider-adapters/src/auth.rs (unit tests)

use std::fmt;
use std::sync::Arc;

use futures::future::BoxFuture;
use indexmap::IndexMap;
use opencode_llm_api::{AuthenticationKind, LlmError, LlmErrorReason, LlmRequest};

/// Lowercased header map in insertion order.
pub type Headers = IndexMap<String, String>;

/// What auth sees: the exact URL and body bytes that will be sent (SigV4 signs these).
#[derive(Clone, Copy)]
pub struct AuthInput<'a> {
    pub request: &'a LlmRequest,
    pub method: &'a str,
    pub url: &'a str,
    pub body: &'a str,
}

/// A secret value that redacts itself in `Debug`.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

/// Narrow credential port. Implementations: static values, env lookups, OAuth leases that
/// refresh near expiry (provider-adapters `credentials` module) backed by Phase 02 storage.
pub trait CredentialSource: Send + Sync + fmt::Debug {
    /// Source label used in `Missing auth credential: <source>`.
    fn source(&self) -> String;
    fn load(&self) -> BoxFuture<'_, Result<Option<Secret>, LlmError>>;
}

/// Static secret, `None`/empty means missing.
#[derive(Clone, Debug)]
pub struct StaticCredential {
    pub source: String,
    pub value: Option<Secret>,
}

impl CredentialSource for StaticCredential {
    fn source(&self) -> String {
        self.source.clone()
    }

    fn load(&self) -> BoxFuture<'_, Result<Option<Secret>, LlmError>> {
        Box::pin(async move { Ok(self.value.clone()) })
    }
}

/// Environment variable credential (`Auth.config(name)`), read at request time.
#[derive(Clone, Debug)]
pub struct EnvCredential(pub String);

impl CredentialSource for EnvCredential {
    fn source(&self) -> String {
        self.0.clone()
    }

    fn load(&self) -> BoxFuture<'_, Result<Option<Secret>, LlmError>> {
        Box::pin(async move { Ok(std::env::var(&self.0).ok().map(Secret::new)) })
    }
}

/// `Credential.orElse`: first source that yields a non-empty secret.
#[derive(Clone, Debug)]
pub struct FirstCredential(pub Vec<Arc<dyn CredentialSource>>);

impl CredentialSource for FirstCredential {
    fn source(&self) -> String {
        self.0.last().map(|s| s.source()).unwrap_or_default()
    }

    fn load(&self) -> BoxFuture<'_, Result<Option<Secret>, LlmError>> {
        Box::pin(async move {
            for source in &self.0 {
                if let Ok(Some(secret)) = source.load().await
                    && !secret.expose().is_empty()
                {
                    return Ok(Some(secret));
                }
            }
            Ok(None)
        })
    }
}

/// Route auth: transforms outgoing headers.
pub trait Auth: Send + Sync + fmt::Debug {
    fn apply<'a>(
        &'a self,
        input: AuthInput<'a>,
        headers: Headers,
    ) -> BoxFuture<'a, Result<Headers, LlmError>>;
}

pub type SharedAuth = Arc<dyn Auth>;

pub fn missing_credential(source: &str) -> LlmError {
    LlmError::new(
        "Auth",
        "apply",
        LlmErrorReason::Authentication {
            message: format!("Missing auth credential: {source}"),
            kind: AuthenticationKind::Missing,
            provider_metadata: None,
            http: None,
        },
    )
}

async fn require(source: &dyn CredentialSource) -> Result<Secret, LlmError> {
    match source.load().await? {
        Some(secret) if !secret.expose().is_empty() => Ok(secret),
        _ => Err(missing_credential(&source.source())),
    }
}

/// `Auth.none` / `Auth.passthrough`.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoAuth;

impl Auth for NoAuth {
    fn apply<'a>(
        &'a self,
        _: AuthInput<'a>,
        headers: Headers,
    ) -> BoxFuture<'a, Result<Headers, LlmError>> {
        Box::pin(async move { Ok(headers) })
    }
}

/// Render a loaded secret into one header (`bearer`, `header(name)`, `bearerHeader(name)`).
#[derive(Clone, Debug)]
pub struct CredentialHeader {
    pub name: String,
    /// Prefix such as `Bearer ` or empty.
    pub prefix: String,
    pub credential: Arc<dyn CredentialSource>,
}

impl CredentialHeader {
    pub fn bearer(credential: Arc<dyn CredentialSource>) -> Self {
        Self::bearer_header("authorization", credential)
    }

    pub fn bearer_header(name: &str, credential: Arc<dyn CredentialSource>) -> Self {
        Self {
            name: name.to_ascii_lowercase(),
            prefix: "Bearer ".into(),
            credential,
        }
    }

    pub fn header(name: &str, credential: Arc<dyn CredentialSource>) -> Self {
        Self {
            name: name.to_ascii_lowercase(),
            prefix: String::new(),
            credential,
        }
    }
}

impl Auth for CredentialHeader {
    fn apply<'a>(
        &'a self,
        _: AuthInput<'a>,
        mut headers: Headers,
    ) -> BoxFuture<'a, Result<Headers, LlmError>> {
        Box::pin(async move {
            let secret = require(self.credential.as_ref()).await?;
            headers.insert(
                self.name.clone(),
                format!("{}{}", self.prefix, secret.expose()),
            );
            Ok(headers)
        })
    }
}

/// `Auth.headers(...)`: set static headers.
#[derive(Clone, Debug)]
pub struct StaticHeaders(pub Headers);

impl Auth for StaticHeaders {
    fn apply<'a>(
        &'a self,
        _: AuthInput<'a>,
        mut headers: Headers,
    ) -> BoxFuture<'a, Result<Headers, LlmError>> {
        Box::pin(async move {
            for (k, v) in &self.0 {
                headers.insert(k.to_ascii_lowercase(), v.clone());
            }
            Ok(headers)
        })
    }
}

/// `Auth.remove(name)`.
#[derive(Clone, Debug)]
pub struct RemoveHeader(pub String);

impl Auth for RemoveHeader {
    fn apply<'a>(
        &'a self,
        _: AuthInput<'a>,
        mut headers: Headers,
    ) -> BoxFuture<'a, Result<Headers, LlmError>> {
        Box::pin(async move {
            headers.shift_remove(&self.0.to_ascii_lowercase());
            Ok(headers)
        })
    }
}

/// `auth.andThen(that)`.
#[derive(Clone, Debug)]
pub struct AndThen(pub SharedAuth, pub SharedAuth);

impl Auth for AndThen {
    fn apply<'a>(
        &'a self,
        input: AuthInput<'a>,
        headers: Headers,
    ) -> BoxFuture<'a, Result<Headers, LlmError>> {
        Box::pin(async move {
            let headers = self.0.apply(input, headers).await?;
            self.1.apply(input, headers).await
        })
    }
}

/// `auth.orElse(that)`: on any failure of the first, apply the second to the original headers.
#[derive(Clone, Debug)]
pub struct OrElse(pub SharedAuth, pub SharedAuth);

impl Auth for OrElse {
    fn apply<'a>(
        &'a self,
        input: AuthInput<'a>,
        headers: Headers,
    ) -> BoxFuture<'a, Result<Headers, LlmError>> {
        Box::pin(async move {
            match self.0.apply(input, headers.clone()).await {
                Ok(h) => Ok(h),
                Err(_) => self.1.apply(input, headers).await,
            }
        })
    }
}

/// Convenience: bearer auth from a literal key (`Auth.bearer("sk-...")`).
pub fn bearer_value(value: impl Into<String>) -> SharedAuth {
    Arc::new(CredentialHeader::bearer(Arc::new(StaticCredential {
        source: "value".into(),
        value: Some(Secret::new(value)),
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> (LlmRequest, String, String) {
        unreachable_request()
    }

    // Auth never inspects the request in these tests, so build one through a dummy route
    // only where needed. Header-only auth is exercised through `CredentialHeader` directly.
    fn unreachable_request() -> (LlmRequest, String, String) {
        let route = crate::route::tests_support::dummy_route();
        let model = opencode_llm_api::Model::new("m", "p", route);
        (LlmRequest::new(model), "https://x/y".into(), "{}".into())
    }

    #[tokio::test]
    async fn bearer_sets_lowercase_authorization() {
        let (request, url, body) = input();
        let input = AuthInput {
            request: &request,
            method: "POST",
            url: &url,
            body: &body,
        };
        let headers = bearer_value("sk-1")
            .apply(input, Headers::new())
            .await
            .unwrap();
        assert_eq!(headers.get("authorization").unwrap(), "Bearer sk-1");
    }

    #[tokio::test]
    async fn empty_secret_is_missing_authentication() {
        let (request, url, body) = input();
        let input = AuthInput {
            request: &request,
            method: "POST",
            url: &url,
            body: &body,
        };
        let auth = CredentialHeader::header(
            "x-api-key",
            Arc::new(StaticCredential {
                source: "ANTHROPIC_API_KEY".into(),
                value: Some(Secret::new("")),
            }),
        );
        let err = auth.apply(input, Headers::new()).await.unwrap_err();
        assert_eq!(err.reason.tag(), "Authentication");
        assert_eq!(
            err.reason.message(),
            "Missing auth credential: ANTHROPIC_API_KEY"
        );
    }

    #[tokio::test]
    async fn or_else_falls_back_and_and_then_chains() {
        let (request, url, body) = input();
        let input = AuthInput {
            request: &request,
            method: "POST",
            url: &url,
            body: &body,
        };
        let missing: SharedAuth = Arc::new(CredentialHeader::bearer(Arc::new(StaticCredential {
            source: "A".into(),
            value: None,
        })));
        let chain = AndThen(
            Arc::new(OrElse(missing, bearer_value("b"))),
            Arc::new(StaticHeaders(Headers::from([(
                "X-Extra".into(),
                "1".into(),
            )]))),
        );
        let headers = chain.apply(input, Headers::new()).await.unwrap();
        assert_eq!(headers.get("authorization").unwrap(), "Bearer b");
        assert_eq!(headers.get("x-extra").unwrap(), "1");
        assert_eq!(format!("{:?}", Secret::new("s")), "<redacted>");
    }
}
