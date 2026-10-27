//! Purpose: normalize referral codes and shape referral reward/status contracts.
//! Scope: pure referral rules; MySQL lifecycle is owned by the billing store.
//! Uses:
//! rust/crates/
//! `-- hosted-domain/src/console/common.rs
//! External:
//! `-- serde::{Deserialize, Serialize}
//! Used by:
//! rust/crates/
//! |-- hosted-adapters/src/console/billing_store.rs
//! `-- hosted-domain/src/console/mod.rs
//! Invariants: referral codes are uppercase ASCII alphanumeric, at most ten characters; reward is $5 in microcents.
//! Contract tests: hosted-domain/src/console/referral.rs

use serde::{Deserialize, Serialize};

use super::common::{MicroCents, cents_to_microcents};

pub const CODE_LENGTH: usize = 10;
pub const REWARD_AMOUNT: MicroCents = 500_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RewardSource {
    Inviter,
    Invitee,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RewardStatus {
    Pending,
    Available,
    Applied,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Reward {
    pub id: String,
    pub source: RewardSource,
    pub status: RewardStatus,
    pub email: Option<String>,
    pub amount: i64,
    pub time_created: String,
    pub time_applied: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub referral_code: String,
    pub has_referral: bool,
    pub reward_amount: i64,
    pub rewards: Vec<Reward>,
}

/// Mirrors `Referral.normalizeCode` (uppercase, strip non-alphanumerics, then truncate).
pub fn normalize_code(code: Option<&str>) -> Option<String> {
    let normalized: String = code?
        .chars()
        .flat_map(char::to_uppercase)
        .filter(char::is_ascii_alphanumeric)
        .take(CODE_LENGTH)
        .collect();
    (!normalized.is_empty()).then_some(normalized)
}

pub fn reward_amount_cents() -> i64 {
    cents_to_microcents(500.into()) / 1_000_000
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn referral_codes_match_baseline_normalization() {
        assert_eq!(
            normalize_code(Some("ab-cd 1234567890")),
            Some("ABCD123456".into())
        );
        assert_eq!(normalize_code(Some("---")), None);
        assert_eq!(normalize_code(None), None);
        assert_eq!(normalize_code(Some("abßcd")), Some("ABSSCD".into()));
        assert_eq!(normalize_code(Some("aﬃb")), Some("AFFIB".into()));
        assert_eq!(normalize_code(Some("ßßßßßß")), Some("SSSSSSSSSS".into()));
        assert_eq!(REWARD_AMOUNT, 500_000_000);
        assert_eq!(reward_amount_cents(), 500);
    }
}
