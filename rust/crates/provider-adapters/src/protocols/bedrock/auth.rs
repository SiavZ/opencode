//! Purpose: Bedrock SigV4 route auth (packages/llm/src/protocols/utils/bedrock-auth.ts): sign the
//! exact URL and JSON body bytes with AWS SigV4 (service `bedrock`) using credentials configured
//! on the route, matching aws4fetch's canonicalization (double-encoded path, unsignable headers
//! excluded, session token signed as `x-amz-security-token`).
//! Scope: `Auth` implementation.
//! Uses:
//! rust/crates/
//! |-- llm-api/src/lib.rs
//! `-- provider-adapters/src/auth.rs
//! External:
//! |-- aws_credential_types::Credentials
//! `-- aws_sigv4::http_request::{SignableBody, SignableRequest, SigningSettings, sign}
//! Used by:
//! rust/crates/
//! |-- provider-adapters/src/protocols/bedrock/mod.rs
//! `-- provider-adapters/tests/bedrock_converse.rs
//! Invariants: signs the same bytes the transport sends; no credentials means an
//! `InvalidRequest` naming both auth paths; the secret never appears in `Debug`.
//! Contract tests: provider-adapters/tests/bedrock_converse.rs

use std::borrow::Cow;
use std::fmt;
use std::time::SystemTime;

use aws_credential_types::Credentials;
use aws_sigv4::http_request::{SignableBody, SignableRequest, SigningSettings, sign};
use aws_sigv4::sign::v4;
use futures::future::BoxFuture;
use opencode_llm_api::LlmError;

use crate::auth::{Auth, AuthInput, Headers, Secret};

/// aws4fetch `UNSIGNABLE_HEADERS`.
const UNSIGNABLE_HEADERS: &[&str] = &[
    "authorization",
    "content-type",
    "content-length",
    "user-agent",
    "presigned-expires",
    "expect",
    "x-amzn-trace-id",
    "range",
    "connection",
];

/// AWS credentials for SigV4 signing (`BedrockCredentials`).
#[derive(Clone)]
pub struct BedrockCredentials {
    pub region: String,
    pub access_key_id: String,
    pub secret_access_key: Secret,
    pub session_token: Option<Secret>,
}

impl fmt::Debug for BedrockCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BedrockCredentials")
            .field("region", &self.region)
            .field("access_key_id", &self.access_key_id)
            .finish_non_exhaustive()
    }
}

/// `BedrockAuth.sigV4(credentials)`. `time` pins the signing clock (tests only need it).
#[derive(Clone, Debug, Default)]
pub struct SigV4Auth {
    pub credentials: Option<BedrockCredentials>,
    pub time: Option<SystemTime>,
}

impl SigV4Auth {
    pub fn new(credentials: Option<BedrockCredentials>) -> Self {
        Self {
            credentials,
            time: None,
        }
    }
}

fn signing_failed(message: impl fmt::Display) -> LlmError {
    LlmError::invalid_request(format!("Bedrock Converse SigV4 signing failed: {message}"))
}

/// Sign `input` and return the headers to add (`authorization`, `x-amz-date`, token).
pub fn sign_headers(
    credentials: &BedrockCredentials,
    input: &AuthInput<'_>,
    headers: &Headers,
    time: SystemTime,
) -> Result<Vec<(String, String)>, LlmError> {
    let identity = Credentials::new(
        credentials.access_key_id.clone(),
        credentials.secret_access_key.expose(),
        credentials
            .session_token
            .as_ref()
            .map(|t| t.expose().to_owned()),
        None,
        "opencode-route",
    )
    .into();
    let mut settings = SigningSettings::default();
    let mut excluded = settings.excluded_headers.take().unwrap_or_default();
    excluded.extend(UNSIGNABLE_HEADERS.iter().map(|h| Cow::Borrowed(*h)));
    settings.excluded_headers = Some(excluded);
    let params = v4::SigningParams::builder()
        .identity(&identity)
        .region(&credentials.region)
        .name("bedrock")
        .time(time)
        .settings(settings)
        .build()
        .map_err(signing_failed)?
        .into();
    let request = SignableRequest::new(
        input.method,
        input.url,
        headers
            .iter()
            .filter(|(k, _)| k.as_str() != "host")
            .map(|(k, v)| (k.as_str(), v.as_str())),
        SignableBody::Bytes(input.body.as_bytes()),
    )
    .map_err(signing_failed)?;
    let (instructions, _) = sign(request, &params).map_err(signing_failed)?.into_parts();
    Ok(instructions
        .headers()
        .map(|(k, v)| (k.to_ascii_lowercase(), v.to_owned()))
        .collect())
}

impl Auth for SigV4Auth {
    fn apply<'a>(
        &'a self,
        input: AuthInput<'a>,
        mut headers: Headers,
    ) -> BoxFuture<'a, Result<Headers, LlmError>> {
        Box::pin(async move {
            let Some(credentials) = &self.credentials else {
                return Err(LlmError::invalid_request(
                    "Bedrock Converse requires either route bearer auth or AWS credentials configured on the route",
                ));
            };
            headers.insert("content-type".into(), "application/json".into());
            let time = self.time.unwrap_or_else(SystemTime::now);
            for (k, v) in sign_headers(credentials, &input, &headers, time)? {
                headers.insert(k, v);
            }
            Ok(headers)
        })
    }
}
