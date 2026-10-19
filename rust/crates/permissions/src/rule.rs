//! Purpose: V2 permission rules and evaluation (packages/core/src/permission.ts:72-90 and
//! registry `whollyDisabled`), last matching rule wins, default `ask`.
//! Scope: pure functions over rulesets.
//! Uses:
//! rust/crates/
//! `-- permissions/src/wildcard.rs
//! External:
//! |-- serde::Deserialize
//! |-- serde::Serialize
//! |-- serde_json::to_string
//! `-- wildcard::matches
//! Used by:
//! rust/crates/
//! |-- permissions/src/lib.rs
//! `-- permissions/src/service.rs
//! Invariants: wire shape `{action, resource, effect}`, effect in allow|deny|ask.
//! Contract tests: permissions/src/rule.rs

use crate::wildcard;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Effect {
    Allow,
    Deny,
    Ask,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Rule {
    pub action: String,
    pub resource: String,
    pub effect: Effect,
}

impl Rule {
    pub fn new(action: impl Into<String>, resource: impl Into<String>, effect: Effect) -> Self {
        Self {
            action: action.into(),
            resource: resource.into(),
            effect,
        }
    }
}

pub type Ruleset = Vec<Rule>;

/// `PermissionV2.evaluate`: last rule whose action and resource patterns match, else `ask` on `*`.
pub fn evaluate(action: &str, resource: &str, rulesets: &[&[Rule]]) -> Rule {
    rulesets
        .iter()
        .rev()
        .flat_map(|set| set.iter().rev())
        .find(|rule| {
            wildcard::matches(action, &rule.action) && wildcard::matches(resource, &rule.resource)
        })
        .cloned()
        .unwrap_or_else(|| Rule::new(action, "*", Effect::Ask))
}

/// `PermissionV2.merge`: concatenation in order.
pub fn merge(rulesets: &[&[Rule]]) -> Ruleset {
    rulesets
        .iter()
        .flat_map(|set| set.iter().cloned())
        .collect()
}

/// Registry `whollyDisabled`: the last rule matching the action is a `*` deny.
pub fn wholly_disabled(action: &str, rules: &[Rule]) -> bool {
    rules
        .iter()
        .rev()
        .find(|rule| wildcard::matches(action, &rule.action))
        .is_some_and(|rule| rule.resource == "*" && rule.effect == Effect::Deny)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_match_wins_and_default_is_ask() {
        let rules = vec![
            Rule::new("*", "*", Effect::Allow),
            Rule::new("edit", "secret/*", Effect::Deny),
        ];
        assert_eq!(evaluate("edit", "secret/a", &[&rules]).effect, Effect::Deny);
        assert_eq!(
            evaluate("edit", "public/a", &[&rules]).effect,
            Effect::Allow
        );
        let fallback = evaluate("bash", "ls", &[&[]]);
        assert_eq!(fallback, Rule::new("bash", "*", Effect::Ask));
    }

    #[test]
    fn wholly_disabled_requires_star_deny_last() {
        let rules = vec![Rule::new("bash", "*", Effect::Deny)];
        assert!(wholly_disabled("bash", &rules));
        let rules = vec![
            Rule::new("bash", "*", Effect::Deny),
            Rule::new("bash", "ls", Effect::Allow),
        ];
        assert!(!wholly_disabled("bash", &rules));
    }

    #[test]
    fn wire_shape() {
        let json =
            serde_json::to_string(&Rule::new("read", "*", Effect::Allow)).unwrap_or_default();
        assert_eq!(json, r#"{"action":"read","resource":"*","effect":"allow"}"#);
    }
}
