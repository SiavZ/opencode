//! Purpose: render console workspace invitations and deliver them through AWS SES v2.
//! Scope: configured invitation mail transport plus a deterministic capture adapter.
//! Uses:
//! `-- none
//! External:
//! |-- async_trait::async_trait
//! `-- aws_credential_types::Credentials
//! |-- aws_sigv4::http_request::{SignableBody, SignableRequest, SigningSettings, sign}
//! |-- aws_sigv4::sign::v4
//! |-- reqwest::Client
//! |-- serde_json::json
//! |-- std::sync::{Arc, Mutex}
//! |-- std::time::SystemTime
//! `-- url::Url
//! Used by:
//! rust/crates/
//! |-- hosted-adapters/src/console/tenant/member.rs
//! `-- hosted-adapters/tests/console_invite_mail.rs
//! Invariants: one SES v2 request per attempted invite, no retries; credentials and captured
//! addresses are redacted from Debug. Production credentials use the baseline explicit SES
//! variables and fail closed when either is absent.
//! Contract tests: hosted-adapters/tests/console_invite_mail.rs

use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use aws_credential_types::Credentials;
use aws_sigv4::http_request::{SignableBody, SignableRequest, SigningSettings, sign};
use aws_sigv4::sign::v4;
use reqwest::Client;
use serde_json::json;
use url::Url;

/// Data needed to render and send the baseline `InviteEmail`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InviteMail {
    pub to: String,
    pub inviter: String,
    pub workspace_id: String,
    pub workspace_name: String,
}

/// Port used by the membership invite flow. The baseline swallows send errors after persistence.
#[async_trait::async_trait]
pub trait InviteMailer: Send + Sync + fmt::Debug {
    async fn send_invite(&self, mail: &InviteMail) -> Result<(), String>;
}

/// SES settings. The endpoint override is private and only exercised by local unit tests.
#[derive(Clone)]
pub struct SesInviteMailConfig {
    pub region: String,
    pub from: String,
    pub access_key_id: Option<String>,
    pub secret_access_key: Option<String>,
    pub session_token: Option<String>,
    endpoint: Option<Url>,
}

impl fmt::Debug for SesInviteMailConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SesInviteMailConfig")
            .field("region", &self.region)
            .field("from", &self.from)
            .field(
                "explicit_credentials_configured",
                &self.access_key_id.is_some(),
            )
            .finish_non_exhaustive()
    }
}

impl SesInviteMailConfig {
    /// Creates the baseline SES settings with caller-supplied credentials.
    pub fn new(
        access_key_id: impl Into<String>,
        secret_access_key: impl Into<String>,
        region: impl Into<String>,
        from: impl Into<String>,
    ) -> Self {
        Self {
            region: region.into(),
            from: from.into(),
            access_key_id: Some(access_key_id.into()),
            secret_access_key: Some(secret_access_key.into()),
            session_token: None,
            endpoint: None,
        }
    }

    /// Overrides SES only for requests to a local loopback address.
    pub fn with_local_endpoint(mut self, endpoint: Url) -> Result<Self, SesInviteMailError> {
        if !matches!(endpoint.scheme(), "http" | "https")
            || !endpoint.host().is_some_and(|host| match host {
                url::Host::Ipv4(address) => address.is_loopback(),
                url::Host::Ipv6(address) => address.is_loopback(),
                url::Host::Domain(_) => false,
            })
        {
            return Err(SesInviteMailError::Configuration(
                "SES test endpoint must use a loopback IP address".into(),
            ));
        }
        self.endpoint = Some(endpoint);
        Ok(self)
    }

    /// Loads the baseline SES key variables; no mailer is configured when neither is present.
    pub fn from_env() -> Result<Option<Self>, SesInviteMailError> {
        let access_key_id = env_value("AWS_SES_ACCESS_KEY_ID");
        let secret_access_key = env_value("AWS_SES_SECRET_ACCESS_KEY");
        if access_key_id.is_some() != secret_access_key.is_some() {
            return Err(SesInviteMailError::Configuration(
                "AWS_SES_ACCESS_KEY_ID and AWS_SES_SECRET_ACCESS_KEY must be set together".into(),
            ));
        }
        let (Some(access_key_id), Some(secret_access_key)) = (access_key_id, secret_access_key)
        else {
            return Ok(None);
        };
        Ok(Some(Self {
            region: env_value("AWS_SES_REGION").unwrap_or_else(|| "us-east-1".into()),
            from: env_value("OPENCODE_CONSOLE_MAIL_FROM")
                .unwrap_or_else(|| "OpenCode Zen <contact@anoma.ly>".into()),
            access_key_id: Some(access_key_id),
            secret_access_key: Some(secret_access_key),
            session_token: env_value("AWS_SESSION_TOKEN"),
            endpoint: None,
        }))
    }
}

fn env_value(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

/// Mail transport initialization or request failure. Never contains credential values.
#[derive(Debug, thiserror::Error)]
pub enum SesInviteMailError {
    #[error("invalid SES mail configuration: {0}")]
    Configuration(String),
    #[error("could not initialize AWS SES credentials: {0}")]
    Credentials(String),
    #[error("could not sign SES invitation request: {0}")]
    Signing(String),
    #[error("SES invitation request failed: {0}")]
    Transport(String),
    #[error("SES invitation request returned HTTP {0}")]
    Rejected(reqwest::StatusCode),
}

/// Configured SES v2 mail sender. No retry is performed, matching the baseline single attempt.
#[derive(Clone)]
pub struct SesInviteMailer {
    config: SesInviteMailConfig,
    client: Client,
    credentials: Credentials,
}

impl fmt::Debug for SesInviteMailer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SesInviteMailer")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl SesInviteMailer {
    pub fn new(config: SesInviteMailConfig) -> Result<Self, SesInviteMailError> {
        if config.region.trim().is_empty()
            || !config
                .region
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            || config.from.trim().is_empty()
        {
            return Err(SesInviteMailError::Configuration(
                "region must be an AWS region name and sender must not be empty".into(),
            ));
        }
        if config.access_key_id.is_some() != config.secret_access_key.is_some() {
            return Err(SesInviteMailError::Configuration(
                "explicit access key and secret key must be set together".into(),
            ));
        }
        let (Some(access_key_id), Some(secret_access_key)) =
            (&config.access_key_id, &config.secret_access_key)
        else {
            return Err(SesInviteMailError::Credentials(
                "AWS_SES_ACCESS_KEY_ID and AWS_SES_SECRET_ACCESS_KEY are required".into(),
            ));
        };
        if let Some(endpoint) = &config.endpoint
            && !matches!(endpoint.scheme(), "http" | "https")
        {
            return Err(SesInviteMailError::Configuration(
                "SES endpoint must use HTTP or HTTPS".into(),
            ));
        }
        let credentials = Credentials::new(
            access_key_id.clone(),
            secret_access_key.clone(),
            config.session_token.clone(),
            None,
            "opencode-console-ses",
        );
        Ok(Self {
            config,
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .map_err(|error| SesInviteMailError::Transport(error.to_string()))?,
            credentials,
        })
    }

    async fn send(&self, mail: &InviteMail) -> Result<(), SesInviteMailError> {
        let url = self.config.endpoint.clone().unwrap_or_else(|| {
            Url::parse(&format!(
                "https://email.{}.amazonaws.com/v2/email/outbound-emails",
                self.config.region
            ))
            .expect("static SES endpoint is valid")
        });
        let invite_url = format!("https://opencode.ai/workspace/{}", mail.workspace_id);
        let message = format!(
            "{} invited you to join the {} workspace.\n\nJoin workspace: {}",
            mail.inviter, mail.workspace_name, invite_url
        );
        let escaped_inviter = html_escape::encode_text(&mail.inviter);
        let escaped_workspace = html_escape::encode_text(&mail.workspace_name);
        let escaped_url = html_escape::encode_double_quoted_attribute(&invite_url);
        let html = format!(
            "<!doctype html><html lang=\"en\"><body><h1>Join your team's OpenCode workspace</h1><p>You have been invited by <strong>{escaped_inviter}</strong> to join the <strong>{escaped_workspace}</strong> workspace on OpenCode.</p><p><a href=\"{escaped_url}\">Join workspace</a></p><p>Button not working? Copy this link: <a href=\"{escaped_url}\">{escaped_url}</a></p></body></html>"
        );
        let payload = json!({
            "FromEmailAddress": self.config.from,
            "Destination": { "ToAddresses": [mail.to] },
            "Content": { "Simple": {
                "Subject": { "Charset": "UTF-8", "Data": format!("You've been invited to join the {} workspace on OpenCode", mail.workspace_name) },
                "Body": {
                    "Text": { "Charset": "UTF-8", "Data": message },
                    "Html": { "Charset": "UTF-8", "Data": html }
                }
            }}
        });
        let body = serde_json::to_vec(&payload)
            .map_err(|error| SesInviteMailError::Transport(error.to_string()))?;
        let identity = self.credentials.clone().into();
        let time = SystemTime::now();
        let mut settings = SigningSettings::default();
        settings.payload_checksum_kind = aws_sigv4::http_request::PayloadChecksumKind::XAmzSha256;
        let params = v4::SigningParams::builder()
            .identity(&identity)
            .region(&self.config.region)
            .name("ses")
            .time(time)
            .settings(settings)
            .build()
            .map_err(|error| SesInviteMailError::Signing(error.to_string()))?
            .into();
        let uri = url.as_str();
        let content_type = "application/json";
        let target = "SES.SendEmail";
        let request = SignableRequest::new(
            "POST",
            uri,
            [("content-type", content_type), ("x-amz-target", target)].into_iter(),
            SignableBody::Bytes(&body),
        )
        .map_err(|error| SesInviteMailError::Signing(error.to_string()))?;
        let (instructions, _) = sign(request, &params)
            .map_err(|error| SesInviteMailError::Signing(error.to_string()))?
            .into_parts();
        let (headers, _) = instructions.into_parts();
        let mut request = self
            .client
            .post(url)
            .header("content-type", content_type)
            .header("x-amz-target", target)
            .body(body);
        for header in headers {
            request = request.header(header.name(), header.value());
        }
        let response = request
            .send()
            .await
            .map_err(|error| SesInviteMailError::Transport(error.to_string()))?;
        if !response.status().is_success() {
            return Err(SesInviteMailError::Rejected(response.status()));
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl InviteMailer for SesInviteMailer {
    async fn send_invite(&self, mail: &InviteMail) -> Result<(), String> {
        self.send(mail).await.map_err(|error| error.to_string())
    }
}

#[derive(Default)]
struct CaptureState {
    sent: Vec<InviteMail>,
    failure: Option<String>,
}

/// Explicit in-memory mail capture for local and acceptance tests; never sends network traffic.
#[derive(Clone, Default)]
pub struct CaptureInviteMailer {
    state: Arc<Mutex<CaptureState>>,
}

impl CaptureInviteMailer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn fail_with(&self, message: impl Into<String>) {
        self.state.lock().expect("capture mutex poisoned").failure = Some(message.into());
    }

    pub fn sent(&self) -> Vec<InviteMail> {
        self.state
            .lock()
            .expect("capture mutex poisoned")
            .sent
            .clone()
    }
}

impl fmt::Debug for CaptureInviteMailer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = self.state.lock().expect("capture mutex poisoned");
        f.debug_struct("CaptureInviteMailer")
            .field("captured_count", &state.sent.len())
            .field("will_fail", &state.failure.is_some())
            .finish()
    }
}

#[async_trait::async_trait]
impl InviteMailer for CaptureInviteMailer {
    async fn send_invite(&self, mail: &InviteMail) -> Result<(), String> {
        let mut state = self.state.lock().expect("capture mutex poisoned");
        state.sent.push(mail.clone());
        match &state.failure {
            Some(message) => Err(message.clone()),
            None => Ok(()),
        }
    }
}

/// Explicit mail failure used when SES is not configured; invite persistence remains baseline-compatible.
#[derive(Debug, Clone, Copy, Default)]
pub struct UnavailableInviteMailer;

#[async_trait::async_trait]
impl InviteMailer for UnavailableInviteMailer {
    async fn send_invite(&self, _mail: &InviteMail) -> Result<(), String> {
        Err("SES invite mail is not configured".into())
    }
}
