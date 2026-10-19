//! Purpose: env-derived desktop startup configuration from `main/index.ts:53-140,191-196,350-375`
//! and `main/constants.ts`: release channel, app names/ids, onboarding test mode, sidecar version,
//! loopback NO_PROXY upsert, proxy env resolution, chromium feature switch and sidecar port.
//! Scope: pure functions over an env map plus the onboarding test root directory creation.
//! Never reads or mutates the real process environment.
//! Uses:
//! `-- none
//! External:
//! |-- io::ErrorKind::NotFound
//! |-- io::Result
//! |-- serde::Deserialize
//! |-- serde::Serialize
//! |-- str::to_string
//! |-- str::trim
//! |-- u32::from
//! `-- u8::is_ascii_digit
//! Used by:
//! rust/crates/
//! |-- desktop-host/src/lifecycle.rs
//! `-- desktop-host/tests/lifecycle_config.rs
//! Invariants: unknown channel values fall back to `dev`; loopback hosts are appended once,
//! case-insensitively, after existing trimmed non-empty entries; NO_PROXY and no_proxy are both
//! upserted independently.
//! Contract tests: desktop-host/tests/lifecycle_config.rs

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Environment snapshot. Ordered for deterministic tests.
pub type EnvMap = BTreeMap<String, String>;

pub const LOOPBACK_HOSTS: [&str; 3] = ["127.0.0.1", "localhost", "::1"];
pub const JS_CALL_STACK_FEATURE: &str = "DocumentPolicyIncludeJSCallStacksInCrashReports";
pub const PROXY_BYPASS_LIST: &str = "<-loopback>";
pub const DEV_REMOTE_DEBUGGING_PORT: &str = "9222";
pub const DEFAULT_PROTOCOL_SCHEME: &str = "opencode";
pub const LOCAL_SIDECAR_HOSTNAME: &str = "127.0.0.1";
pub const LOCAL_SIDECAR_USERNAME: &str = "opencode";
/// Health wait budget for the v1 sidecar after readiness (`index.ts:397-404`).
pub const SIDECAR_HEALTH_TIMEOUT_SECS: u64 = 30;
/// Updater polling interval (`index.ts:316`).
pub const UPDATE_CHECK_INTERVAL_SECS: u64 = 10 * 60;
/// Subdirectories created under the onboarding test root (`index.ts:131`).
pub const ONBOARDING_TEST_DIRS: [&str; 6] =
    ["data", "config", "cache", "state", "desktop", "session"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    Dev,
    Beta,
    Prod,
}

impl Channel {
    /// `constants.ts:4-5`: anything other than dev/beta/prod (including unset) is `dev`.
    pub fn parse(raw: Option<&str>) -> Self {
        match raw {
            Some("beta") => Self::Beta,
            Some("prod") => Self::Prod,
            _ => Self::Dev,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dev => "dev",
            Self::Beta => "beta",
            Self::Prod => "prod",
        }
    }

    pub fn app_name(self) -> &'static str {
        match self {
            Self::Dev => "OpenCode Dev",
            Self::Beta => "OpenCode Beta",
            Self::Prod => "OpenCode",
        }
    }

    pub fn app_id(self) -> &'static str {
        match self {
            Self::Dev => "ai.opencode.desktop.dev",
            Self::Beta => "ai.opencode.desktop.beta",
            Self::Prod => "ai.opencode.desktop",
        }
    }
}

/// `constants.ts:7`: updater only runs in packaged non-dev builds.
pub fn updater_enabled(is_packaged: bool, channel: Channel) -> bool {
    is_packaged && channel != Channel::Dev
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SidecarVersion {
    V1,
    V2,
}

impl SidecarVersion {
    /// `index.ts:64`: exactly `OPENCODE_SIDECAR_V2=1` selects v2.
    pub fn from_env(env: &EnvMap) -> Self {
        match env.get("OPENCODE_SIDECAR_V2").map(String::as_str) {
            Some("1") => Self::V2,
            _ => Self::V1,
        }
    }
}

/// `index.ts:63`: exactly `OPENCODE_TEST_ONBOARDING=1`.
pub fn test_onboarding(env: &EnvMap) -> bool {
    env.get("OPENCODE_TEST_ONBOARDING").map(String::as_str) == Some("1")
}

/// Identity applied to the native app before any window exists (`index.ts:125,141-147`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppIdentity {
    pub name: &'static str,
    pub app_id: &'static str,
    pub user_data: PathBuf,
    pub session_data: Option<PathBuf>,
}

/// Unpackaged builds always use the dev identity regardless of channel.
pub fn app_identity(
    is_packaged: bool,
    channel: Channel,
    app_data: &Path,
    onboarding_test_root: Option<&Path>,
) -> AppIdentity {
    let identity = if is_packaged { channel } else { Channel::Dev };
    AppIdentity {
        name: identity.app_name(),
        app_id: identity.app_id(),
        user_data: match onboarding_test_root {
            Some(root) => root.join("desktop"),
            None => app_data.join(identity.app_id()),
        },
        session_data: onboarding_test_root.map(|root| root.join("session")),
    }
}

/// Isolated directories for onboarding test mode plus the env overrides the baseline applies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OnboardingTestRoot {
    pub root: PathBuf,
    pub env: EnvMap,
}

/// `index.ts:126-140`: creates `<tmp>/opencode-onboarding-<uuid>` with fresh subdirectories.
/// Caller passes the temp base so tests never touch real data.
pub fn create_onboarding_test_root(tmp: &Path) -> io::Result<OnboardingTestRoot> {
    let root = tmp.join(format!("opencode-onboarding-{}", uuid::Uuid::new_v4()));
    match std::fs::remove_dir_all(&root) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    for dir in ONBOARDING_TEST_DIRS {
        std::fs::create_dir_all(root.join(dir))?;
    }
    let path = |dir: &str| root.join(dir).to_string_lossy().into_owned();
    let env = EnvMap::from([
        ("OPENCODE_DB".to_string(), ":memory:".to_string()),
        ("XDG_DATA_HOME".to_string(), path("data")),
        ("XDG_CONFIG_HOME".to_string(), path("config")),
        ("XDG_CACHE_HOME".to_string(), path("cache")),
        ("XDG_STATE_HOME".to_string(), path("state")),
    ]);
    Ok(OnboardingTestRoot { root, env })
}

/// Env the desktop main process always sets before launching anything (`index.ts:123`).
pub fn base_env_overrides() -> EnvMap {
    EnvMap::from([(
        "OPENCODE_DISABLE_EMBEDDED_WEB_UI".to_string(),
        "true".to_string(),
    )])
}

/// `ensureLoopbackNoProxy` (`index.ts:95-113`): mutates both `NO_PROXY` and `no_proxy`.
pub fn ensure_loopback_no_proxy(env: &mut EnvMap) {
    for key in ["NO_PROXY", "no_proxy"] {
        let mut items: Vec<String> = env
            .get(key)
            .map(String::as_str)
            .unwrap_or("")
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .collect();
        for host in LOOPBACK_HOSTS {
            if items.iter().any(|value| value.to_lowercase() == host) {
                continue;
            }
            items.push(host.to_string());
        }
        env.insert(key.to_string(), items.join(","));
    }
}

/// Proxy configuration the baseline loads with `http.setGlobalProxyFromEnv()` (`index.ts:72-79`),
/// following Node 24 `lib/internal/http.js` `parseProxyUrl`/`ProxyConfig`: lowercase variables
/// take precedence (`env.https_proxy || env.HTTPS_PROXY`), empty values fall through, and only
/// `http://`/`https://` proxy URLs are honored. Because `ensureLoopbackNoProxy` always writes a
/// non-empty lowercase `no_proxy`, an uppercase-only `NO_PROXY` user list is shadowed in the
/// baseline; this port preserves that.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProxyEnv {
    pub http_proxy: Option<String>,
    pub https_proxy: Option<String>,
    pub no_proxy: Vec<String>,
}

pub fn proxy_from_env(env: &EnvMap) -> ProxyEnv {
    let pick = |lower: &str, upper: &str| {
        [lower, upper]
            .iter()
            .filter_map(|key| env.get(*key))
            .find(|value| !value.is_empty())
            .cloned()
    };
    let proxy_url = |lower: &str, upper: &str| {
        pick(lower, upper).filter(|url| url.starts_with("http://") || url.starts_with("https://"))
    };
    ProxyEnv {
        http_proxy: proxy_url("http_proxy", "HTTP_PROXY"),
        https_proxy: proxy_url("https_proxy", "HTTPS_PROXY"),
        no_proxy: pick("no_proxy", "NO_PROXY")
            .unwrap_or_default()
            .split(',')
            .map(|value| value.trim().to_lowercase())
            .collect(),
    }
}

impl ProxyEnv {
    /// Proxy for a request, `None` when direct (Node `ProxyConfig.shouldUseProxy`). Only `http`
    /// and `https` are proxied. Entries match: `*`, exact host, exact `host:port`, `.suffix`
    /// (also the bare suffix), `*.suffix`, and IPv4 `a.b.c.d-e.f.g.h` ranges. A bare `example.com`
    /// does not match subdomains.
    pub fn proxy_for(&self, scheme: &str, host: &str, port: u16) -> Option<&str> {
        let proxy = match scheme {
            "https" => self.https_proxy.as_deref(),
            "http" => self.http_proxy.as_deref(),
            _ => None,
        }?;
        let host = host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_lowercase();
        let host_with_port = if port == 0 {
            host.clone()
        } else {
            format!("{host}:{port}")
        };
        let bypass = self.no_proxy.iter().any(|entry| {
            if entry == "*" || *entry == host || *entry == host_with_port {
                return true;
            }
            if let Some(suffix) = entry.strip_prefix('.')
                && (host == suffix || host.ends_with(&format!(".{suffix}")))
            {
                return true;
            }
            if entry.starts_with("*.") && host.ends_with(&entry[1..]) {
                return true;
            }
            ipv4_range_contains(entry, &host)
        });
        if bypass { None } else { Some(proxy) }
    }
}

fn ipv4_range_contains(entry: &str, host: &str) -> bool {
    let Some(host) = parse_ipv4(host) else {
        return false;
    };
    let mut parts = entry.split('-');
    let (Some(start), Some(end)) = (parts.next(), parts.next()) else {
        return false;
    };
    match (parse_ipv4(start.trim()), parse_ipv4(end.trim())) {
        (Some(start), Some(end)) => (start..=end).contains(&host),
        _ => false,
    }
}

fn parse_ipv4(value: &str) -> Option<u32> {
    value.parse::<std::net::Ipv4Addr>().ok().map(u32::from)
}

/// Value for the `enable-features` switch (`index.ts:194-195`).
pub fn enable_features_switch(existing: Option<&str>) -> String {
    match existing {
        Some(features) if !features.is_empty() => format!("{JS_CALL_STACK_FEATURE},{features}"),
        _ => JS_CALL_STACK_FEATURE.to_string(),
    }
}

/// Chromium switches appended at startup, in baseline order (`index.ts:193-196`).
pub fn command_line_switches(
    is_packaged: bool,
    existing_features: Option<&str>,
) -> Vec<(&'static str, String)> {
    let mut switches = vec![
        ("proxy-bypass-list", PROXY_BYPASS_LIST.to_string()),
        ("enable-features", enable_features_switch(existing_features)),
    ];
    if !is_packaged {
        switches.push((
            "remote-debugging-port",
            DEV_REMOTE_DEBUGGING_PORT.to_string(),
        ));
    }
    switches
}

/// `OPENCODE_PORT` with JavaScript `parseInt(value, 10)` semantics (`index.ts:351-355`): leading
/// whitespace and sign allowed, trailing garbage ignored, empty or non-numeric means "pick a free
/// port". The value is not range checked here, matching the baseline.
pub fn sidecar_port_from_env(env: &EnvMap) -> Option<i64> {
    let raw = env.get("OPENCODE_PORT")?;
    if raw.is_empty() {
        return None;
    }
    parse_int(raw)
}

fn parse_int(raw: &str) -> Option<i64> {
    let text = raw.trim_start();
    let (negative, digits) = match text.as_bytes().first() {
        Some(b'-') => (true, &text[1..]),
        Some(b'+') => (false, &text[1..]),
        _ => (false, text),
    };
    let end = digits.bytes().take_while(u8::is_ascii_digit).count();
    if end == 0 {
        return None;
    }
    let value: i64 = digits[..end].parse().ok()?;
    Some(if negative { -value } else { value })
}

/// Loopback sidecar URL (`index.ts:373-374`).
pub fn local_sidecar_url(port: u16) -> String {
    format!("http://{LOCAL_SIDECAR_HOSTNAME}:{port}")
}
