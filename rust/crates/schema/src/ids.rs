//! Purpose: every branded identifier from `packages/schema/src` (standalone ID modules and
//! the per-domain `ID` brands), with the baseline prefix checks and constructors
//! (`create`, `ascending`, `descending`). Centralized so domain modules never cycle.
//! Scope: pure values plus the identifier generator.
//! Uses:
//! rust/crates/
//! |-- schema/src/identifier.rs
//! `-- schema/src/macros.rs
//! External:
//! |-- serde::{Deserialize, Serialize}
//! `-- thiserror::Error
//! Used by:
//! rust/crates/
//! |-- schema/src/lib.rs
//! `-- schema/src/project.rs
//! Invariants: prefix checks match `isStartsWith` exactly (some without underscore, e.g.
//! `ses`, `per`, `que`, `pty`, `wrk`; some with, e.g. `evt_`, `msg_`). Brands without a
//! check accept any string. Generated IDs use the baseline prefix and direction.
//! ProjectCopyStrategyID trims ECMAScript whitespace on decode and rejects empty results.
//! Contract tests: schema/src/ids.rs, schema/tests/baseline_fixtures.rs,
//! schema/tests/schema_domains_project.rs

use crate::identifier::{self, Direction};
use serde::{Deserialize, Serialize};

/// Decoding failure of a prefix-checked brand, worded like the baseline filter message.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("Expected a string starting with \"{prefix}\", got \"{value}\"")]
pub struct BrandError {
    pub prefix: &'static str,
    pub value: String,
}

/// Errors from `ascending(id)` style constructors that throw in the baseline.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("ID {id} does not start with {prefix}")]
pub struct WrongPrefix {
    pub id: String,
    pub prefix: &'static str,
}

fn generated(prefix: &str, direction: Direction) -> String {
    identifier::create(prefix, direction, None)
}

macro_rules! generated_id {
    ($name:ident, $tag:literal, $direction:ident) => {
        impl $name {
            /// `create()`: `<prefix>_<body>` with the baseline direction.
            pub fn create() -> Self {
                Self(generated($tag, Direction::$direction))
            }
        }
    };
}

crate::brand!(
    /// `session-id.ts` `SessionID` (brand `SessionID`, check `ses`).
    SessionID,
    "ses"
);
generated_id!(SessionID, "ses", Descending);
impl SessionID {
    /// `descending(id?)`: creates when absent, otherwise trusts the given value (no check).
    pub fn descending(id: Option<&str>) -> Self {
        id.map(|id| Self(id.to_string()))
            .unwrap_or_else(Self::create)
    }
}

crate::brand!(
    /// `workspace-id.ts` `WorkspaceID` (brand `WorkspaceV2.ID`, check `wrk`).
    WorkspaceID,
    "wrk"
);
generated_id!(WorkspaceID, "wrk", Ascending);
impl WorkspaceID {
    /// `ascending(id?)`: creates when absent or empty, throws on a wrong prefix.
    pub fn ascending(id: Option<&str>) -> Result<Self, WrongPrefix> {
        match id {
            None | Some("") => Ok(Self::create()),
            Some(id) if id.starts_with("wrk") => Ok(Self(id.to_string())),
            Some(id) => Err(WrongPrefix {
                id: id.to_string(),
                prefix: "wrk",
            }),
        }
    }
}

crate::brand!(
    /// `project-id.ts` `ProjectID` (brand `Project.ID`).
    ProjectID
);
impl ProjectID {
    /// `ProjectID.global`.
    pub fn global() -> Self {
        Self::new("global")
    }
}

/// `project-copy.ts` `StrategyID`: ECMAScript trim, non-empty, plain string JSON.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct ProjectCopyStrategyID(String);

impl ProjectCopyStrategyID {
    pub fn new(value: impl Into<String>) -> Result<Self, ProjectCopyStrategyError> {
        let value = value.into();
        // JS trim includes BOM but not Rust's extra whitespace U+0085.
        let value = value.trim_matches(|c| {
            matches!(c,
                '\u{0009}'..='\u{000d}' | '\u{0020}' | '\u{00a0}' | '\u{1680}' |
                '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' |
                '\u{205f}' | '\u{3000}' | '\u{feff}'
            )
        });
        if value.is_empty() {
            return Err(ProjectCopyStrategyError);
        }
        Ok(Self(value.to_owned()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub fn into_inner(self) -> String {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("Expected a non-empty string")]
pub struct ProjectCopyStrategyError;

impl<'de> Deserialize<'de> for ProjectCopyStrategyID {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

crate::brand!(@common ProjectCopyStrategyID);

crate::brand!(
    /// `integration-id.ts` `IntegrationID` (brand `Integration.ID`).
    IntegrationID
);
crate::brand!(
    /// `integration-id.ts` `IntegrationMethodID` (brand `Integration.MethodID`).
    IntegrationMethodID
);
crate::brand!(
    /// `integration.ts` `AttemptID` (brand `Integration.AttemptID`, created as `con_`).
    IntegrationAttemptID
);
impl IntegrationAttemptID {
    pub fn create() -> Self {
        Self(generated("con", Direction::Ascending))
    }
}

crate::brand!(
    /// `event.ts` `Event.ID` (check `evt_`).
    EventID,
    "evt_"
);
generated_id!(EventID, "evt", Ascending);

crate::brand!(
    /// `session-message.ts` `Session.Message.ID` (check `msg_`).
    MessageID,
    "msg_"
);
generated_id!(MessageID, "msg", Ascending);

crate::brand!(
    /// `permission.ts` `PermissionV2.ID` (check `per`).
    PermissionID,
    "per"
);
generated_id!(PermissionID, "per", Ascending);

crate::brand!(
    /// `question.ts` `QuestionV2.ID` (check `que`).
    QuestionID,
    "que"
);
generated_id!(QuestionID, "que", Ascending);

crate::brand!(
    /// `pty.ts` `PtyID` (check `pty`).
    PtyID,
    "pty"
);
generated_id!(PtyID, "pty", Ascending);

crate::brand!(
    /// `credential.ts` `Credential.ID` (created as `cred_`, no check).
    CredentialID
);
impl CredentialID {
    pub fn create() -> Self {
        Self(generated("cred", Direction::Ascending))
    }
}

crate::brand!(
    /// `permission-saved.ts` `PermissionSaved.ID` (created as `psv_`, no check).
    PermissionSavedID
);
impl PermissionSavedID {
    pub fn create() -> Self {
        Self(generated("psv", Direction::Ascending))
    }
}

crate::brand!(
    /// `agent.ts` `AgentV2.ID`.
    AgentID
);
crate::brand!(
    /// `model.ts` `ModelV2.ID`.
    ModelID
);
crate::brand!(
    /// `model.ts` `VariantID`.
    VariantID
);
crate::brand!(
    /// `model.ts` `Family`.
    ModelFamily
);
crate::brand!(
    /// `plugin.ts` `Plugin.ID`.
    PluginID
);

crate::brand!(
    /// `provider.ts` `ProviderV2.ID`.
    ProviderID
);
impl ProviderID {
    pub fn opencode() -> Self {
        Self::new("opencode")
    }
    pub fn anthropic() -> Self {
        Self::new("anthropic")
    }
    pub fn openai() -> Self {
        Self::new("openai")
    }
    pub fn google() -> Self {
        Self::new("google")
    }
    pub fn google_vertex() -> Self {
        Self::new("google-vertex")
    }
    pub fn github_copilot() -> Self {
        Self::new("github-copilot")
    }
    pub fn amazon_bedrock() -> Self {
        Self::new("amazon-bedrock")
    }
    pub fn azure() -> Self {
        Self::new("azure")
    }
    pub fn openrouter() -> Self {
        Self::new("openrouter")
    }
    pub fn mistral() -> Self {
        Self::new("mistral")
    }
    pub fn gitlab() -> Self {
        Self::new("gitlab")
    }
}

/// V1 identifiers from `v1/*.ts`, kept for compatibility, persistence and migration.
pub mod v1 {
    use super::{Direction, generated};

    crate::brand!(
        /// `v1/permission.ts` `PermissionID` (check `per`).
        PermissionID,
        "per"
    );
    crate::brand!(
        /// `v1/question.ts` `QuestionID` (check `que`).
        QuestionID,
        "que"
    );
    crate::brand!(
        /// `v1/session.ts` `MessageID` (check `msg`, no underscore).
        MessageID,
        "msg"
    );
    crate::brand!(
        /// `v1/session.ts` `PartID` (check `prt`).
        PartID,
        "prt"
    );

    macro_rules! v1_ascending {
        ($name:ident, $tag:literal) => {
            impl $name {
                /// `ascending(id?)`: creates when absent, otherwise trusts the value.
                pub fn ascending(id: Option<&str>) -> Self {
                    id.map(|id| Self(id.to_string()))
                        .unwrap_or_else(|| Self(generated($tag, Direction::Ascending)))
                }
            }
        };
    }
    v1_ascending!(PermissionID, "per");
    v1_ascending!(QuestionID, "que");
    v1_ascending!(MessageID, "msg");
    v1_ascending!(PartID, "prt");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_checks_match_baseline() {
        assert!(SessionID::new("sesX").is_ok());
        assert!(EventID::new("evtX").is_err());
        assert!(MessageID::new("msgX").is_err());
        assert!(v1::MessageID::new("msgX").is_ok());
        let error = serde_json::from_str::<PermissionID>("\"perx\"").map(|_| ());
        assert!(error.is_ok());
        let error = serde_json::from_str::<QuestionID>("\"abc\"").unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Expected a string starting with \"que\", got \"abc\"")
        );
    }

    #[test]
    fn generated_ids_have_baseline_prefixes() {
        assert!(SessionID::create().as_str().starts_with("ses_"));
        assert!(EventID::create().as_str().starts_with("evt_"));
        assert!(CredentialID::create().as_str().starts_with("cred_"));
        assert!(IntegrationAttemptID::create().as_str().starts_with("con_"));
        assert_eq!(SessionID::create().as_str().len(), 30);
    }

    #[test]
    fn workspace_ascending_throws_on_wrong_prefix() {
        let error = WorkspaceID::ascending(Some("abc")).unwrap_err();
        assert_eq!(error.to_string(), "ID abc does not start with wrk");
        assert!(
            WorkspaceID::ascending(Some(""))
                .unwrap()
                .as_str()
                .starts_with("wrk_")
        );
    }

    #[test]
    fn brands_serialize_as_bare_strings() {
        let id = ProviderID::google_vertex();
        assert_eq!(serde_json::to_string(&id).unwrap(), "\"google-vertex\"");
    }
}
