//! Purpose: console API key and BYOK provider credential rules: `sk-` secret generation,
//! own-vs-admin key visibility and display masking, BYOK provider catalog and masking. Pure.
//! Scope: per-action decisions and projections over repository rows.
//! Uses:
//! rust/crates/
//! `-- hosted-domain/src/console/common.rs
//! External:
//! `-- serde::Serialize
//! Used by:
//! rust/crates/
//! |-- hosted-adapters/src/console/tenant.rs
//! `-- hosted-domain/src/console/mod.rs
//! Invariants: secrets are `sk-` + 64 chars of `[A-Za-z0-9]` chosen as `u32 % 62`
//! (`core/src/key.ts:47-55`); only the owner sees `key`, everyone sees `keyDisplay`
//! (`core/src/key.ts:34-39`); BYOK create/remove are admin only (`core/src/provider.ts`).
//! Contract tests: hosted-domain/src/console/credentials.rs (unit tests)

use serde::Serialize;

use super::common::{Actor, ConsoleError, Role};

const KEY_CHARS: &[u8; 62] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

/// Builds the secret from 64 random u32 values like `crypto.getRandomValues(Uint32Array(64))`.
pub fn secret_from_random(random: &[u32; 64]) -> String {
    let mut out = String::with_capacity(67);
    out.push_str("sk-");
    out.extend(random.iter().map(|r| KEY_CHARS[(*r % 62) as usize] as char));
    out
}

/// Generates a fresh `sk-` secret from the OS random source.
pub fn generate_secret() -> String {
    let mut bytes = [0u8; 256];
    // An unavailable OS random source is unrecoverable for key generation.
    getrandom::fill(&mut bytes).expect("OS random source unavailable");
    let mut words = [0u32; 64];
    for (i, w) in words.iter_mut().enumerate() {
        *w = u32::from_le_bytes([
            bytes[i * 4],
            bytes[i * 4 + 1],
            bytes[i * 4 + 2],
            bytes[i * 4 + 3],
        ]);
    }
    secret_from_random(&words)
}

/// `Key.create` input validation: `name` 1..=255 UTF-16 units.
pub fn validate_key_name(name: &str) -> Result<(), ConsoleError> {
    let len = name.encode_utf16().count();
    if len < 1 {
        return Err(ConsoleError::Validation(
            "Too small: expected string to have >=1 characters".into(),
        ));
    }
    if len > 255 {
        return Err(ConsoleError::Validation(
            "Too big: expected string to have <=255 characters".into(),
        ));
    }
    Ok(())
}

/// Key list scope: admins list every workspace key, members only their own.
pub fn key_scope_user(actor: &Actor) -> Result<Option<String>, ConsoleError> {
    match actor {
        Actor::User {
            role: Role::Admin, ..
        } => Ok(None),
        Actor::User { user_id, .. } => Ok(Some(user_id.clone())),
        _ => actor.user_role().map(|_| None),
    }
}

/// Key row as loaded by `Key.list` (joined with the owner email auth row).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyRow {
    pub id: String,
    pub name: String,
    pub key: String,
    pub time_used: Option<String>,
    pub user_id: String,
    pub email: String,
}

/// `Key.list` projection. `key` is omitted (undefined) for keys owned by other users.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct KeyView {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(rename = "timeUsed")]
    pub time_used: Option<String>,
    #[serde(rename = "userID")]
    pub user_id: String,
    pub email: String,
    #[serde(rename = "keyDisplay")]
    pub key_display: String,
}

/// `${key.slice(0, 7)}...${key.slice(-4)}` over UTF-16 units (keys are ASCII).
pub fn key_display(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    let head: String = chars.iter().take(7).collect();
    let tail: String = if chars.len() >= 4 {
        chars[chars.len() - 4..].iter().collect()
    } else {
        chars.iter().collect()
    };
    format!("{head}...{tail}")
}

/// Projects key rows for the actor. Requires a user actor (`Actor.userID()`).
pub fn project_keys(actor: &Actor, rows: Vec<KeyRow>) -> Result<Vec<KeyView>, ConsoleError> {
    let me = match actor {
        Actor::User { user_id, .. } => user_id.clone(),
        other => return other.user_role().map(|_| Vec::new()),
    };
    Ok(rows
        .into_iter()
        .map(|row| KeyView {
            key_display: key_display(&row.key),
            key: (row.user_id == me).then_some(row.key),
            id: row.id,
            name: row.name,
            time_used: row.time_used,
            user_id: row.user_id,
            email: row.email,
        })
        .collect())
}

/// BYOK providers offered by the UI (`provider-section.tsx:10-14`).
pub const BYOK_PROVIDERS: [(&str, &str, &str); 3] = [
    ("OpenAI", "openai", "sk-"),
    ("Anthropic", "anthropic", "sk-ant-"),
    ("Google Gemini", "google", "AI"),
];

/// UI mask `${credentials.slice(0, 8)}...${credentials.slice(-8)}`.
pub fn mask_credentials(credentials: &str) -> String {
    let chars: Vec<char> = credentials.chars().collect();
    let head: String = chars.iter().take(8).collect();
    let tail: String = if chars.len() >= 8 {
        chars[chars.len() - 8..].iter().collect()
    } else {
        chars.iter().collect()
    };
    format!("{head}...{tail}")
}

/// `Provider.create` input: provider 1..=64 chars; admin only.
pub fn authorize_provider_create(actor: &Actor, provider: &str) -> Result<String, ConsoleError> {
    let len = provider.encode_utf16().count();
    if len < 1 {
        return Err(ConsoleError::Validation(
            "Too small: expected string to have >=1 characters".into(),
        ));
    }
    if len > 64 {
        return Err(ConsoleError::Validation(
            "Too big: expected string to have <=64 characters".into(),
        ));
    }
    actor.assert_admin()?;
    Ok(actor.workspace()?.to_string())
}

/// `Provider.remove`: admin only.
pub fn authorize_provider_remove(actor: &Actor) -> Result<String, ConsoleError> {
    actor.assert_admin()?;
    Ok(actor.workspace()?.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_shape() {
        let s = generate_secret();
        assert!(s.starts_with("sk-"));
        assert_eq!(s.len(), 67);
        assert!(s[3..].bytes().all(|b| b.is_ascii_alphanumeric()));
        let mut r = [0u32; 64];
        r[0] = 62;
        r[1] = 61;
        r[2] = 26;
        assert!(secret_from_random(&r).starts_with("sk-A9a"));
    }

    #[test]
    fn visibility_and_display() {
        let member = Actor::User {
            user_id: "usr_me".into(),
            workspace_id: "wrk".into(),
            account_id: "acc".into(),
            role: Role::Member,
        };
        let rows = vec![
            KeyRow {
                id: "key_1".into(),
                name: "mine".into(),
                key: "sk-ABCDEFGHIJKLMNOP".into(),
                time_used: None,
                user_id: "usr_me".into(),
                email: "me@x".into(),
            },
            KeyRow {
                id: "key_2".into(),
                name: "theirs".into(),
                key: "sk-0123456789WXYZ".into(),
                time_used: None,
                user_id: "usr_other".into(),
                email: "o@x".into(),
            },
        ];
        let views = project_keys(&member, rows).unwrap();
        assert_eq!(views[0].key.as_deref(), Some("sk-ABCDEFGHIJKLMNOP"));
        assert_eq!(views[0].key_display, "sk-ABCD...MNOP");
        assert_eq!(views[1].key, None);
        let json = serde_json::to_string(&views[1]).unwrap();
        assert!(!json.contains("\"key\""));
        assert!(json.contains("\"keyDisplay\":\"sk-0123...WXYZ\""));
        assert_eq!(key_scope_user(&member).unwrap().as_deref(), Some("usr_me"));
        assert!(
            project_keys(
                &Actor::System {
                    workspace_id: "w".into()
                },
                vec![]
            )
            .is_err()
        );
    }

    #[test]
    fn byok_rules() {
        assert_eq!(
            mask_credentials("sk-ant-0123456789abcdef"),
            "sk-ant-0...89abcdef"
        );
        let member = Actor::User {
            user_id: "u".into(),
            workspace_id: "w".into(),
            account_id: "a".into(),
            role: Role::Member,
        };
        assert!(authorize_provider_create(&member, "openai").is_err());
        assert!(authorize_provider_remove(&member).is_err());
        assert!(validate_key_name("").is_err());
    }
}
