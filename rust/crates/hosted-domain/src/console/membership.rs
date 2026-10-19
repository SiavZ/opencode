//! Purpose: console workspace and membership rules: workspace name/region validation, default
//! regions by country, invite/update/remove authorization (admin-only, not-self), monthly limit
//! form parsing and the account-erasure plan. Pure rules, no I/O.
//! Scope: per-action decisions; adapters execute the resulting writes in one transaction.
//! Uses:
//! rust/crates/
//! `-- hosted-domain/src/console/common.rs
//! External:
//! |-- char::is_whitespace
//! |-- i64::MAX
//! |-- serde::Deserialize
//! |-- serde::Serialize
//! `-- str::trim
//! Used by:
//! rust/crates/
//! |-- hosted-adapters/src/console/tenant/member.rs
//! |-- hosted-adapters/src/console/tenant/workspace.rs
//! |-- hosted-adapters/tests/console_identity.rs
//! `-- hosted-domain/src/console/mod.rs
//! Invariants: messages equal the baseline thrown errors (`core/src/user.ts:18-21`,
//! `core/src/actor.ts:70-73`); regions are `us|eu|sg|cn`, CN country adds `cn`
//! (`core/src/workspace.ts:83-97`).
//! Contract tests: hosted-domain/src/console/membership.rs (unit tests)

use serde::{Deserialize, Serialize};

use super::common::{Actor, ConsoleError, Role};

/// `Workspace.Region`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Region {
    Us,
    Eu,
    Sg,
    Cn,
}

/// `Workspace.setDefaultRegion`: CN gets all four, everyone else us/eu/sg.
pub fn default_regions(country: Option<&str>) -> Vec<Region> {
    if country.is_some_and(|c| c.to_uppercase() == "CN") {
        return vec![Region::Us, Region::Eu, Region::Sg, Region::Cn];
    }
    vec![Region::Us, Region::Eu, Region::Sg]
}

/// `Workspace.create` input: `z.string().min(1)`.
pub fn validate_workspace_create_name(name: &str) -> Result<(), ConsoleError> {
    if name.chars().count() < 1 {
        return Err(ConsoleError::Validation(
            "Too small: expected string to have >=1 characters".into(),
        ));
    }
    Ok(())
}

/// `Workspace.update` input. Fields absent from the input are left untouched.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct WorkspaceUpdate {
    pub name: Option<String>,
    pub region: Option<Vec<Region>>,
    pub allow_training: Option<bool>,
}

/// Validates `Workspace.update` (admin only, name 1..=255 chars, at least one region).
pub fn authorize_workspace_update(
    actor: &Actor,
    update: &WorkspaceUpdate,
) -> Result<String, ConsoleError> {
    actor.assert_admin()?;
    if let Some(name) = &update.name {
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
    }
    if update.region.as_ref().is_some_and(Vec::is_empty) {
        return Err(ConsoleError::Validation(
            "Too small: expected array to have >=1 items".into(),
        ));
    }
    Ok(actor.workspace()?.to_string())
}

/// Settings form validation (`settings-section.tsx:31-48`): returns the i18n error key.
pub fn settings_form_name(raw: Option<&str>) -> Result<String, &'static str> {
    let name = raw.map(str::trim).unwrap_or("");
    if name.is_empty() {
        return Err("error.workspaceNameRequired");
    }
    if name.encode_utf16().count() > 255 {
        return Err("error.nameTooLong");
    }
    Ok(name.to_string())
}

/// Member form `limit` parsing: blank -> null, `parseInt` otherwise, negative rejected with
/// `error.monthlyLimitInvalid` (`member-section.tsx`). `parseInt("abc")` is NaN which JSON
/// encodes as null in the baseline; the Rust port maps it to `None` as well.
pub fn parse_monthly_limit(raw: Option<&str>) -> Result<Option<i64>, &'static str> {
    let Some(raw) = raw else { return Ok(None) };
    if raw.trim().is_empty() {
        return Ok(None);
    }
    let parsed = js_parse_int(raw);
    match parsed {
        Some(n) if n < 0 => Err("error.monthlyLimitInvalid"),
        other => Ok(other),
    }
}

/// JS `parseInt(s)` for base 10: skip leading whitespace, optional sign, leading digits.
pub fn js_parse_int(s: &str) -> Option<i64> {
    let t = s.trim_start();
    let (neg, rest) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    let value: i64 = digits.parse().unwrap_or(i64::MAX);
    Some(if neg { -value } else { value })
}

/// Invite input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invite {
    pub email: String,
    pub role: Role,
    pub monthly_limit: Option<i64>,
}

/// `User.invite` authorization: admin only. Returns the workspace id.
pub fn authorize_invite(actor: &Actor) -> Result<String, ConsoleError> {
    actor.assert_admin()?;
    Ok(actor.workspace()?.to_string())
}

/// Invite mail subject (`core/src/user.ts:143`).
pub fn invite_subject(workspace_name: &str) -> String {
    format!("You've been invited to join the {workspace_name} workspace on OpenCode")
}

fn assert_not_self(actor: &Actor, target_user_id: &str) -> Result<(), ConsoleError> {
    if let Actor::User { user_id, .. } = actor
        && user_id == target_user_id
    {
        return Err(ConsoleError::PermissionDenied(
            "Expected not self actor, got self actor".into(),
        ));
    }
    Ok(())
}

/// `User.update`: admin only; demoting self to member is refused.
pub fn authorize_member_update(
    actor: &Actor,
    target_user_id: &str,
    role: Role,
) -> Result<String, ConsoleError> {
    actor.assert_admin()?;
    if role == Role::Member {
        // `Actor.userID()` throws for non-user actors before comparing.
        actor.user_role()?;
        assert_not_self(actor, target_user_id)?;
    }
    Ok(actor.workspace()?.to_string())
}

/// `User.remove`: admin only and never self.
pub fn authorize_member_remove(
    actor: &Actor,
    target_user_id: &str,
) -> Result<String, ConsoleError> {
    actor.assert_admin()?;
    assert_not_self(actor, target_user_id)?;
    Ok(actor.workspace()?.to_string())
}

/// `Account.remove(email)` input: `z.email()`. Minimal structural check.
pub fn validate_erasure_email(email: &str) -> Result<(), ConsoleError> {
    let ok = email.split_once('@').is_some_and(|(local, domain)| {
        !local.is_empty()
            && domain.contains('.')
            && !domain.starts_with('.')
            && !domain.ends_with('.')
    }) && !email.chars().any(char::is_whitespace);
    if ok {
        return Ok(());
    }
    Err(ConsoleError::Validation("Invalid email address".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(role: Role) -> Actor {
        Actor::User {
            user_id: "usr_self".into(),
            workspace_id: "wrk_1".into(),
            account_id: "acc_1".into(),
            role,
        }
    }

    #[test]
    fn regions_by_country() {
        assert_eq!(default_regions(Some("cn")).len(), 4);
        assert_eq!(
            default_regions(Some("US")),
            vec![Region::Us, Region::Eu, Region::Sg]
        );
        assert_eq!(default_regions(None).len(), 3);
        assert_eq!(
            serde_json::to_string(&default_regions(Some("CN"))).unwrap(),
            r#"["us","eu","sg","cn"]"#
        );
    }

    #[test]
    fn update_requires_admin() {
        let upd = WorkspaceUpdate {
            name: Some("x".into()),
            ..Default::default()
        };
        assert_eq!(
            authorize_workspace_update(&user(Role::Member), &upd)
                .unwrap_err()
                .to_string(),
            "Action not allowed. Ask your workspace admin to perform this action."
        );
        assert_eq!(
            authorize_workspace_update(&user(Role::Admin), &upd).unwrap(),
            "wrk_1"
        );
        let empty = WorkspaceUpdate {
            region: Some(vec![]),
            ..Default::default()
        };
        assert!(authorize_workspace_update(&user(Role::Admin), &empty).is_err());
    }

    #[test]
    fn member_rules() {
        let admin = user(Role::Admin);
        assert_eq!(
            authorize_member_remove(&admin, "usr_self")
                .unwrap_err()
                .to_string(),
            "Expected not self actor, got self actor"
        );
        assert!(authorize_member_remove(&admin, "usr_other").is_ok());
        assert!(authorize_member_update(&admin, "usr_self", Role::Admin).is_ok());
        assert!(authorize_member_update(&admin, "usr_self", Role::Member).is_err());
        assert!(authorize_invite(&user(Role::Member)).is_err());
    }

    #[test]
    fn form_parsing() {
        assert_eq!(parse_monthly_limit(Some("  ")), Ok(None));
        assert_eq!(parse_monthly_limit(Some("12abc")), Ok(Some(12)));
        assert_eq!(
            parse_monthly_limit(Some("-1")),
            Err("error.monthlyLimitInvalid")
        );
        assert_eq!(parse_monthly_limit(Some("abc")), Ok(None));
        assert_eq!(
            settings_form_name(Some("  ")),
            Err("error.workspaceNameRequired")
        );
        assert_eq!(
            settings_form_name(Some(&"a".repeat(256))),
            Err("error.nameTooLong")
        );
        assert_eq!(settings_form_name(Some(" ok ")), Ok("ok".into()));
        assert_eq!(
            invite_subject("Acme"),
            "You've been invited to join the Acme workspace on OpenCode"
        );
        assert!(validate_erasure_email("a@b.co").is_ok());
        assert!(validate_erasure_email("nope").is_err());
    }
}
