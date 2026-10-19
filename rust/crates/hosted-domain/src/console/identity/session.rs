//! Purpose: console session cookie payload (`AuthSession`): ordered account map plus current
//! account, login/logout transitions and non-workspace account selection.
//! Scope: value transitions applied per request by the auth routes.
//! Uses:
//! `-- none
//! External:
//! |-- serde::Deserialize
//! |-- serde::Deserializer
//! |-- serde::Serialize
//! |-- serde::Serializer
//! |-- serde::de::Error::custom
//! |-- serde_json::Map
//! |-- serde_json::Map::deserialize
//! |-- serde_json::Value
//! |-- serde_json::from_value
//! |-- serde_json::to_value
//! `-- str::to_string
//! Used by:
//! rust/crates/
//! `-- hosted-domain/src/console/identity.rs
//! Invariants: key order follows JS object insertion order (`acc_` keys are never integer-like);
//! logout removes `current` and picks the first remaining key (`routes/auth/logout.ts:5-17`).
//! Contract tests: hosted-domain/src/console/identity/session.rs (unit tests)

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Console session cookie payload (`app/src/context/auth.ts:19-28`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthSession {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<OrderedAccounts>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current: Option<String>,
}

/// One account entry of the session map.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionAccount {
    pub id: String,
    pub email: String,
}

/// Insertion-ordered account map; JS object key order decides `Object.values(account)[0]`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OrderedAccounts(pub Vec<(String, SessionAccount)>);

impl OrderedAccounts {
    pub fn get(&self, key: &str) -> Option<&SessionAccount> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// JS `obj[key] = value`: replaces in place or appends. Integer-like keys would reorder
    /// in JS, but account ids are `acc_` prefixed so insertion order holds.
    pub fn insert(&mut self, key: String, value: SessionAccount) {
        match self.0.iter_mut().find(|(k, _)| *k == key) {
            Some(entry) => entry.1 = value,
            None => self.0.push((key, value)),
        }
    }

    pub fn remove(&mut self, key: &str) {
        self.0.retain(|(k, _)| k != key);
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(|(k, _)| k.as_str())
    }

    pub fn first(&self) -> Option<&SessionAccount> {
        self.0.first().map(|(_, v)| v)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl Serialize for OrderedAccounts {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let map: serde_json::Map<String, serde_json::Value> = self
            .0
            .iter()
            .map(|(k, v)| (k.clone(), serde_json::to_value(v).unwrap_or_default()))
            .collect();
        map.serialize(s)
    }
}

impl<'de> Deserialize<'de> for OrderedAccounts {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let map = serde_json::Map::<String, serde_json::Value>::deserialize(d)?;
        map.into_iter()
            .map(|(k, v)| {
                serde_json::from_value(v)
                    .map(|v| (k, v))
                    .map_err(serde::de::Error::custom)
            })
            .collect::<Result<Vec<_>, _>>()
            .map(OrderedAccounts)
    }
}

impl AuthSession {
    /// Callback: add/replace the account and make it current (`routes/auth/[...callback].ts:24-35`).
    pub fn login(&mut self, account_id: &str, email: &str) {
        let accounts = self.account.get_or_insert_with(Default::default);
        accounts.insert(
            account_id.to_string(),
            SessionAccount {
                id: account_id.to_string(),
                email: email.to_string(),
            },
        );
        self.current = Some(account_id.to_string());
    }

    /// Logout: drop the current account and pick the first remaining (`routes/auth/logout.ts:5-17`).
    /// Returns false when there was no current account (session untouched).
    pub fn logout(&mut self) -> bool {
        let Some(current) = self.current.clone() else {
            return false;
        };
        if let Some(accounts) = self.account.as_mut() {
            accounts.remove(&current);
        }
        self.current = self
            .account
            .as_ref()
            .and_then(|a| a.keys().next().map(str::to_string));
        true
    }

    /// Account ids in insertion order.
    pub fn account_ids(&self) -> Vec<String> {
        self.account
            .as_ref()
            .map(|a| a.keys().map(str::to_string).collect())
            .unwrap_or_default()
    }
}

/// Where the actor resolution for a request without workspace goes (`context/auth.ts:46-63`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountSelection {
    /// Use this account, and persist `current` when `update_current` is true.
    Account {
        account: SessionAccount,
        update_current: bool,
    },
    Public,
}

/// Picks the account for non-workspace requests.
pub fn select_session_account(session: &AuthSession) -> AccountSelection {
    let accounts = session.account.clone().unwrap_or_default();
    if let Some(current) = accounts.get(session.current.as_deref().unwrap_or("")) {
        return AccountSelection::Account {
            account: current.clone(),
            update_current: false,
        };
    }
    match accounts.first() {
        Some(first) => AccountSelection::Account {
            account: first.clone(),
            update_current: true,
        },
        None => AccountSelection::Public,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_login_logout_order() {
        let mut s = AuthSession::default();
        s.login("acc_1", "a@x");
        s.login("acc_2", "b@x");
        assert_eq!(
            serde_json::to_string(&s).unwrap(),
            r#"{"account":{"acc_1":{"id":"acc_1","email":"a@x"},"acc_2":{"id":"acc_2","email":"b@x"}},"current":"acc_2"}"#
        );
        assert!(s.logout());
        assert_eq!(s.current.as_deref(), Some("acc_1"));
        assert!(s.logout());
        assert_eq!(s.current, None);
        assert_eq!(serde_json::to_string(&s).unwrap(), r#"{"account":{}}"#);
        assert!(!s.logout());
    }
}
