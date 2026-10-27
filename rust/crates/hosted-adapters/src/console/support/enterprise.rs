//! Purpose: deliver enterprise inquiries to Salesforce, SES and EmailOctopus concurrently.
//! Scope: configured outbound HTTP integrations, one attempt per service without retries.
//! Uses:
//! `-- none
//! External:
//! |-- aws_credential_types::Credentials
//! |-- aws_sigv4::{http_request, sign::v4}
//! |-- reqwest::Client
//! |-- serde::{Deserialize, Serialize}
//! |-- serde_json::{Value, json}
//! `-- url::Url
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/support.rs
//! Invariants: credentials never appear in Debug/errors; redirect following is disabled; overrides require loopback IPs.
//! Contract tests: hosted-adapters/tests/console_support_enterprise.rs

use aws_credential_types::Credentials;
use aws_sigv4::http_request::{SignableBody, SignableRequest, SigningSettings, sign};
use aws_sigv4::sign::v4;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use url::Url;

#[derive(Clone, Default)]
pub struct EnterpriseConfig {
    pub salesforce_url: Option<String>,
    pub salesforce_client_id: Option<String>,
    pub salesforce_client_secret: Option<String>,
    pub octopus_api_key: Option<String>,
    pub sales_inbox: Option<String>,
    pub ses_access_key: Option<String>,
    pub ses_secret_key: Option<String>,
    pub ses_session_token: Option<String>,
    local_base: Option<Url>,
}

impl std::fmt::Debug for EnterpriseConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EnterpriseConfig")
            .field("salesforce_configured", &self.salesforce_url.is_some())
            .field("octopus_configured", &self.octopus_api_key.is_some())
            .field("ses_configured", &self.ses_access_key.is_some())
            .finish_non_exhaustive()
    }
}

impl EnterpriseConfig {
    pub fn from_env() -> Self {
        let env = |name| std::env::var(name).ok().filter(|value| !value.is_empty());
        Self {
            salesforce_url: env("SALESFORCE_INSTANCE_URL"),
            salesforce_client_id: env("SALESFORCE_CLIENT_ID"),
            salesforce_client_secret: env("SALESFORCE_CLIENT_SECRET"),
            octopus_api_key: env("EMAILOCTOPUS_API_KEY"),
            sales_inbox: env("ENTERPRISE_SALES_INBOX_EMAIL"),
            ses_access_key: env("AWS_SES_ACCESS_KEY_ID"),
            ses_secret_key: env("AWS_SES_SECRET_ACCESS_KEY"),
            ses_session_token: env("AWS_SESSION_TOKEN"),
            local_base: None,
        }
    }

    /// Overrides every external integration endpoint only for isolated HTTP fixture servers.
    pub fn with_local_base(mut self, base: Url) -> Result<Self, &'static str> {
        if !local_url(&base) {
            return Err("enterprise fixture endpoint must be a loopback IP URL");
        }
        self.local_base = Some(base);
        Ok(self)
    }
}

fn local_url(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https")
        && url.username().is_empty()
        && url.password().is_none()
        && url.host().is_some_and(|host| match host {
            url::Host::Ipv4(ip) => ip.is_loopback(),
            url::Host::Ipv6(ip) => ip.is_loopback(),
            url::Host::Domain(_) => false,
        })
}

#[derive(Clone, Deserialize, Serialize)]
pub struct Inquiry {
    pub name: String,
    pub role: String,
    pub company: Option<String>,
    pub email: String,
    pub phone: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Delivery {
    pub lead: bool,
    pub mail: bool,
    pub octopus: bool,
}

impl Delivery {
    pub fn accepted(self) -> bool {
        self.lead || self.mail || self.octopus
    }
}

#[derive(Clone)]
pub struct EnterpriseClient {
    config: EnterpriseConfig,
    client: Client,
}

impl EnterpriseClient {
    /// Public newsletter action ignores upstream status but propagates transport failure.
    pub async fn email_signup(&self, email: &Value) -> Result<bool, &'static str> {
        let key = self
            .config
            .octopus_api_key
            .as_deref()
            .ok_or("Email signup is not configured")?;
        let url = self
            .config
            .local_base
            .clone()
            .unwrap_or_else(|| Url::parse("https://api.emailoctopus.com").unwrap())
            .join("/lists/8b9bb82c-9d5f-11f0-975f-0df6fd1e4945/contacts")
            .map_err(|_| "Email signup is not configured")?;
        self.client
            .put(url)
            .bearer_auth(key)
            .json(&json!({"email_address":email}))
            .send()
            .await
            .map_err(|_| "Email signup request failed")?;
        Ok(true)
    }

    pub fn new(config: EnterpriseConfig) -> Result<Self, reqwest::Error> {
        Ok(Self {
            config,
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
        })
    }

    pub async fn deliver(&self, inquiry: &Inquiry) -> Delivery {
        let (lead, mail, octopus) = tokio::join!(
            self.lead(inquiry),
            self.mail(inquiry),
            self.subscribe(inquiry)
        );
        Delivery {
            lead: lead.unwrap_or(false),
            mail: mail.unwrap_or(false),
            octopus: octopus.unwrap_or(false),
        }
    }

    async fn lead(&self, inquiry: &Inquiry) -> Option<bool> {
        let base = match &self.config.local_base {
            Some(base) => base.clone(),
            None => Url::parse(self.config.salesforce_url.as_deref()?).ok()?,
        };
        if self.config.local_base.is_none()
            && (base.scheme() != "https"
                || !base.username().is_empty()
                || base.password().is_some())
        {
            return None;
        }
        let response = self
            .client
            .post(base.join("/services/oauth2/token").ok()?)
            .form(&[
                ("grant_type", "client_credentials"),
                ("client_id", self.config.salesforce_client_id.as_deref()?),
                (
                    "client_secret",
                    self.config.salesforce_client_secret.as_deref()?,
                ),
            ])
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            return Some(false);
        }
        let auth: Value = response.json().await.ok()?;
        let token = auth.get("access_token")?.as_str()?;
        let instance = auth
            .get("instance_url")
            .and_then(Value::as_str)
            .and_then(|v| Url::parse(v).ok())
            .unwrap_or(base);
        // Fixture credentials never leave loopback; production tokens require TLS and no URL credentials.
        if self.config.local_base.is_some() && !local_url(&instance) {
            return Some(false);
        }
        if self.config.local_base.is_none()
            && (instance.scheme() != "https"
                || !instance.username().is_empty()
                || instance.password().is_some())
        {
            return Some(false);
        }
        Some(self.client.post(instance.join("/services/data/v59.0/sobjects/Lead").ok()?).bearer_auth(token).json(&json!({
            "LastName":inquiry.name,
            "Company":inquiry.company.as_deref().map(str::trim).filter(|v|!v.is_empty()).unwrap_or("Website"),
            "Email":inquiry.email,"Phone":inquiry.phone,"Title":inquiry.role,"Description":inquiry.message,"LeadSource":"Website"
        })).send().await.ok()?.status().is_success())
    }

    async fn subscribe(&self, inquiry: &Inquiry) -> Option<bool> {
        let url = self
            .config
            .local_base
            .clone()
            .unwrap_or_else(|| Url::parse("https://api.emailoctopus.com").unwrap())
            .join("/lists/1b381e5e-39bd-11f1-ba4a-cdd4791f0c43/contacts")
            .ok()?;
        let mut parts = inquiry.name.split_whitespace();
        let first = parts.next().unwrap_or("");
        let last = parts.collect::<Vec<_>>().join(" ");
        let mut payload = json!({"email_address":inquiry.email});
        if !first.is_empty() {
            payload["fields"] = json!({"FirstName":first});
            if !last.is_empty() {
                payload["fields"]["LastName"] = json!(last);
            }
        }
        Some(
            self.client
                .put(url)
                .bearer_auth(self.config.octopus_api_key.as_deref()?)
                .json(&payload)
                .send()
                .await
                .ok()?
                .status()
                .is_success(),
        )
    }

    async fn mail(&self, inquiry: &Inquiry) -> Option<bool> {
        let url = self
            .config
            .local_base
            .clone()
            .unwrap_or_else(|| Url::parse("https://email.us-east-1.amazonaws.com").unwrap())
            .join("/v2/email/outbound-emails")
            .ok()?;
        let company = inquiry
            .company
            .as_deref()
            .filter(|v| !v.is_empty())
            .map(|v| format!("{v}<br>"))
            .unwrap_or_default();
        let phone = inquiry
            .phone
            .as_deref()
            .filter(|v| !v.is_empty())
            .map(|v| format!("{v}<br>"))
            .unwrap_or_default();
        let content = format!(
            "{}<br><br>\n--<br>\n{}<br>\n{}<br>\n{}{}<br>\n{}",
            inquiry.message, inquiry.name, inquiry.role, company, inquiry.email, phone
        )
        .trim()
        .to_string();
        let body = serde_json::to_vec(&json!({
            "FromEmailAddress":"OpenCode Zen <contact@anoma.ly>",
            "Destination":{"ToAddresses":[self.config.sales_inbox.as_deref()?]},
            "ReplyToAddresses":[inquiry.email],
            "Content":{"Simple":{"Subject":{"Charset":"UTF-8","Data":format!("Enterprise Inquiry from {}",inquiry.name)},"Body":{"Text":{"Charset":"UTF-8","Data":content},"Html":{"Charset":"UTF-8","Data":content}}}}
        })).ok()?;
        let identity = Credentials::new(
            self.config.ses_access_key.clone()?,
            self.config.ses_secret_key.clone()?,
            self.config.ses_session_token.clone(),
            None,
            "opencode-enterprise",
        )
        .into();
        let mut settings = SigningSettings::default();
        settings.payload_checksum_kind = aws_sigv4::http_request::PayloadChecksumKind::XAmzSha256;
        let params = v4::SigningParams::builder()
            .identity(&identity)
            .region("us-east-1")
            .name("ses")
            .time(std::time::SystemTime::now())
            .settings(settings)
            .build()
            .ok()?
            .into();
        let request = SignableRequest::new(
            "POST",
            url.as_str(),
            [
                ("content-type", "application/json"),
                ("x-amz-target", "SES.SendEmail"),
            ]
            .into_iter(),
            SignableBody::Bytes(&body),
        )
        .ok()?;
        let (instructions, _) = sign(request, &params).ok()?.into_parts();
        let (headers, _) = instructions.into_parts();
        let mut request = self
            .client
            .post(url)
            .header("content-type", "application/json")
            .header("x-amz-target", "SES.SendEmail")
            .body(body);
        for header in headers {
            request = request.header(header.name(), header.value());
        }
        Some(request.send().await.ok()?.status().is_success())
    }
}
