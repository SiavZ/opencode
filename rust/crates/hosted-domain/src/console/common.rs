//! Purpose: shared console domain values: prefixed ULID identifiers, microcent money, actors and
//! the typed domain error. Every other console module builds on these.
//! Scope: pure values, no I/O except the OS clock/random source for new identifiers.
//! Uses:
//! `-- none
//! External:
//! |-- getrandom::fill
//! |-- i64::MAX
//! |-- i64::MIN
//! |-- i64::try_from
//! |-- rust_decimal::Decimal
//! |-- serde::Deserialize
//! |-- serde::Serialize
//! |-- serde_json::to_string
//! `-- u128::from_le_bytes
//! Used by:
//! rust/crates/
//! |-- hosted-adapters/src/console/auth.rs
//! |-- hosted-adapters/src/console/auth/issuer.rs
//! |-- hosted-adapters/src/console/tenant.rs
//! |-- hosted-adapters/src/console/tenant/account.rs
//! |-- hosted-adapters/src/console/tenant/keys.rs
//! |-- hosted-adapters/src/console/tenant/member.rs
//! |-- hosted-adapters/src/console/tenant/workspace.rs
//! |-- hosted-adapters/tests/console_identity.rs
//! |-- hosted-adapters/tests/console_identity_auth.rs
//! |-- hosted-domain/src/console/credentials.rs
//! |-- hosted-domain/src/console/identity.rs
//! |-- hosted-domain/src/console/membership.rs
//! `-- hosted-domain/src/console/mod.rs
//! Invariants: ids are `<prefix>_<26 char Crockford ULID>` like `packages/console/core/src/identifier.ts`;
//! money is integer microcents (1 cent = 1_000_000) and conversions round half away from zero
//! like `Math.round` for positive values (`packages/console/core/src/util/price.ts`).
//! Contract tests: hosted-domain/src/console/common.rs (unit tests)

use std::time::{SystemTime, UNIX_EPOCH};

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// Identifier prefixes from `packages/console/core/src/identifier.ts:5-20`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IdKind {
    Account,
    Auth,
    Benchmark,
    Billing,
    Key,
    Lite,
    Model,
    Payment,
    Provider,
    Referral,
    Subscription,
    Usage,
    User,
    Workspace,
}

impl IdKind {
    pub const fn prefix(self) -> &'static str {
        match self {
            IdKind::Account => "acc",
            IdKind::Auth => "aut",
            IdKind::Benchmark => "ben",
            IdKind::Billing => "bil",
            IdKind::Key => "key",
            IdKind::Lite => "lit",
            IdKind::Model => "mod",
            IdKind::Payment => "pay",
            IdKind::Provider => "prv",
            IdKind::Referral => "ref",
            IdKind::Subscription => "sub",
            IdKind::Usage => "usg",
            IdKind::User => "usr",
            IdKind::Workspace => "wrk",
        }
    }
}

const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// Creates `<prefix>_<ulid>` or validates a caller-provided id (`Identifier.create(prefix, given)`).
pub fn create_id(kind: IdKind, given: Option<&str>) -> Result<String, ConsoleError> {
    if let Some(given) = given {
        if given.starts_with(kind.prefix()) {
            return Ok(given.to_string());
        }
        return Err(ConsoleError::Validation(format!(
            "ID {given} does not start with {}",
            kind.prefix()
        )));
    }
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    Ok(format!("{}_{}", kind.prefix(), ulid(millis, random_u128())))
}

fn random_u128() -> u128 {
    let mut buf = [0u8; 16];
    // An unavailable OS random source is unrecoverable for id generation, like `ulid()`.
    getrandom::fill(&mut buf).expect("OS random source unavailable");
    u128::from_le_bytes(buf)
}

/// Encodes a ULID: 10 chars of 48-bit millisecond time, 16 chars of 80 random bits.
pub fn ulid(millis: u64, random: u128) -> String {
    let value: u128 = ((millis as u128 & 0xFFFF_FFFF_FFFF) << 80) | (random & ((1u128 << 80) - 1));
    (0..26)
        .map(|i| CROCKFORD[((value >> (125 - i * 5)) & 0x1F) as usize] as char)
        .collect()
}

/// Integer microcents. `balance`, usage `cost` and fees are stored as MySQL bigint microcents.
pub type MicroCents = i64;
pub const MICROCENTS_PER_CENT: i64 = 1_000_000;

/// `centsToMicroCents`: `Math.round(amount * 1000000)` over a possibly fractional cent amount.
pub fn cents_to_microcents(cents: Decimal) -> MicroCents {
    js_round(cents * Decimal::from(MICROCENTS_PER_CENT))
}

/// `microCentsToCents`: `Math.round(amount / 1000000)`.
pub fn microcents_to_cents(amount: MicroCents) -> i64 {
    js_round(Decimal::from(amount) / Decimal::from(MICROCENTS_PER_CENT))
}

/// JavaScript `Math.round`: halves round toward positive infinity (`-2.5 -> -2`, `2.5 -> 3`).
pub fn js_round(value: Decimal) -> i64 {
    let floor = value.floor();
    let rounded = if value - floor >= Decimal::new(5, 1) {
        floor + Decimal::ONE
    } else {
        floor
    };
    i64::try_from(rounded).unwrap_or(if value.is_sign_negative() {
        i64::MIN
    } else {
        i64::MAX
    })
}

/// Workspace roles from `packages/console/core/src/schema/user.sql.ts:5`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Admin,
    Member,
}

impl Role {
    pub const fn as_str(self) -> &'static str {
        match self {
            Role::Admin => "admin",
            Role::Member => "member",
        }
    }
}

/// Actor context from `packages/console/core/src/actor.ts:5-36`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "properties", rename_all = "lowercase")]
pub enum Actor {
    Account {
        #[serde(rename = "accountID")]
        account_id: String,
        email: String,
    },
    Public {},
    User {
        #[serde(rename = "userID")]
        user_id: String,
        #[serde(rename = "workspaceID")]
        workspace_id: String,
        #[serde(rename = "accountID")]
        account_id: String,
        role: Role,
    },
    System {
        #[serde(rename = "workspaceID")]
        workspace_id: String,
    },
}

impl Actor {
    fn type_name(&self) -> &'static str {
        match self {
            Actor::Account { .. } => "account",
            Actor::Public {} => "public",
            Actor::User { .. } => "user",
            Actor::System { .. } => "system",
        }
    }

    /// `Actor.workspace()`.
    pub fn workspace(&self) -> Result<&str, ConsoleError> {
        match self {
            Actor::User { workspace_id, .. } | Actor::System { workspace_id } => Ok(workspace_id),
            other => Err(ConsoleError::Actor(format!(
                "actor of type \"{}\" is not associated with a workspace",
                other.type_name()
            ))),
        }
    }

    /// `Actor.account()`.
    pub fn account(&self) -> Result<&str, ConsoleError> {
        match self {
            Actor::User { account_id, .. } | Actor::Account { account_id, .. } => Ok(account_id),
            other => Err(ConsoleError::Actor(format!(
                "actor of type \"{}\" is not associated with an account",
                other.type_name()
            ))),
        }
    }

    /// `Actor.assert("user")` then role.
    pub fn user_role(&self) -> Result<Role, ConsoleError> {
        match self {
            Actor::User { role, .. } => Ok(*role),
            other => Err(ConsoleError::Actor(format!(
                "Expected actor type user, got {}",
                other.type_name()
            ))),
        }
    }

    /// `Actor.assertAdmin()`.
    pub fn assert_admin(&self) -> Result<(), ConsoleError> {
        if self.user_role()? == Role::Admin {
            return Ok(());
        }
        Err(ConsoleError::PermissionDenied(
            "Action not allowed. Ask your workspace admin to perform this action.".into(),
        ))
    }
}

/// Typed console domain error. Display text equals the baseline thrown `Error.message`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConsoleError {
    #[error("{0}")]
    Validation(String),
    #[error("{0}")]
    PermissionDenied(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    Actor(String),
    /// A baseline capability that the Rust port does not implement yet, see phase-12 status.md.
    #[error("not yet ported: {0}")]
    NotYetPorted(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_have_prefix_and_ulid_body() {
        let id = create_id(IdKind::Workspace, None).unwrap();
        assert!(id.starts_with("wrk_"));
        assert_eq!(id.len(), 4 + 26);
        assert_eq!(create_id(IdKind::Key, Some("key_x")).unwrap(), "key_x");
        assert_eq!(
            create_id(IdKind::Key, Some("usr_x"))
                .unwrap_err()
                .to_string(),
            "ID usr_x does not start with key"
        );
    }

    #[test]
    fn ulid_matches_reference_encoding() {
        // ulid spec example: time 1469918176385 encodes as 01ARYZ6S41.
        assert_eq!(&ulid(1_469_918_176_385, 0)[..10], "01ARYZ6S41");
        assert_eq!(&ulid(0, 0), "00000000000000000000000000");
    }

    #[test]
    fn money_rounds_like_math_round() {
        assert_eq!(cents_to_microcents(Decimal::from(500)), 500_000_000);
        assert_eq!(cents_to_microcents(Decimal::new(15, 7)), 2); // 0.0000015 cents -> 1.5 -> 2
        assert_eq!(microcents_to_cents(1_500_000), 2);
        assert_eq!(microcents_to_cents(-1_500_000), -1);
        assert_eq!(microcents_to_cents(1_499_999), 1);
    }

    #[test]
    fn actor_rules_match_baseline_messages() {
        let member = Actor::User {
            user_id: "usr_1".into(),
            workspace_id: "wrk_1".into(),
            account_id: "acc_1".into(),
            role: Role::Member,
        };
        assert_eq!(
            member.assert_admin().unwrap_err().to_string(),
            "Action not allowed. Ask your workspace admin to perform this action."
        );
        assert_eq!(
            Actor::Public {}.workspace().unwrap_err().to_string(),
            "actor of type \"public\" is not associated with a workspace"
        );
        let json = serde_json::to_string(&Actor::System {
            workspace_id: "wrk_1".into(),
        })
        .unwrap();
        assert_eq!(
            json,
            r#"{"type":"system","properties":{"workspaceID":"wrk_1"}}"#
        );
    }
}
