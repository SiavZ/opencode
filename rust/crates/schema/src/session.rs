//! Purpose: `session.ts` SessionV2.Info and Session.ListAnchor.
//! Scope: pure values.
//! Uses:
//! rust/crates/
//! |-- schema/src/ids.rs
//! |-- schema/src/location.rs
//! |-- schema/src/provider.rs
//! |-- schema/src/revert.rs
//! |-- schema/src/session_message.rs
//! `-- schema/src/values.rs
//! External:
//! `-- serde::{Deserialize, Serialize}
//! Used by:
//! rust/crates/
//! `-- schema/src/lib.rs
//! Invariants: key order `id, parentID, projectID, agent, model, cost, tokens, time, title,
//! location, subpath, revert`; optional keys omitted and `null` rejected.
//! Contract tests: schema/src/session.rs (unit tests), schema/tests/schema_session.rs

use crate::ids::{AgentID, ProjectID, SessionID};
use crate::location;
use crate::provider::ModelRef;
use crate::revert;
use crate::session_message::Tokens;
use crate::values::{DateTimeUtc, Finite, RelativePath, optional};
use serde::{Deserialize, Serialize};

/// `SessionV2.Info.time`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Time {
    pub created: DateTimeUtc,
    pub updated: DateTimeUtc,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub archived: Option<DateTimeUtc>,
}

/// `SessionV2.Info`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Info {
    pub id: SessionID,
    #[serde(
        rename = "parentID",
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub parent_id: Option<SessionID>,
    #[serde(rename = "projectID")]
    pub project_id: ProjectID,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub agent: Option<AgentID>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub model: Option<ModelRef>,
    pub cost: Finite,
    pub tokens: Tokens,
    pub time: Time,
    pub title: String,
    pub location: location::Ref,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub subpath: Option<RelativePath>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub revert: Option<revert::State>,
}

/// `Session.ListAnchor.direction`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Previous,
    Next,
}

/// `Session.ListAnchor` (`time` is any finite number, not a DateTime).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListAnchor {
    pub id: SessionID,
    pub time: Finite,
    pub direction: Direction,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_matches_baseline() {
        // fixture: captured from bun at baseline (packages/schema 907b3bc51).
        let input = r#"{"title":"t","location":{"directory":"/d"},"time":{"updated":2,"created":1},"tokens":{"input":1,"output":2,"reasoning":3,"cache":{"read":4,"write":5}},"cost":1.25,"projectID":"global","id":"ses_1"}"#;
        let info: Info = serde_json::from_str(input).unwrap();
        assert_eq!(
            serde_json::to_string(&info).unwrap(),
            r#"{"id":"ses_1","projectID":"global","cost":1.25,"tokens":{"input":1,"output":2,"reasoning":3,"cache":{"read":4,"write":5}},"time":{"created":1,"updated":2},"title":"t","location":{"directory":"/d"}}"#
        );
    }

    #[test]
    fn info_rejects_bad_values() {
        let base = r#""title":"t","location":{"directory":"/d"},"time":{"updated":2,"created":1},"tokens":{"input":1,"output":2,"reasoning":3,"cache":{"read":4,"write":5}},"cost":1,"projectID":"global""#;
        assert!(serde_json::from_str::<Info>(&format!(r#"{{{base},"id":"x_1"}}"#)).is_err());
        assert!(
            serde_json::from_str::<Info>(&format!(r#"{{{base},"id":"ses_1","parentID":null}}"#))
                .is_err()
        );
        let anchor = r#"{"id":"ses_1","time":1.5,"direction":"up"}"#;
        assert!(serde_json::from_str::<ListAnchor>(anchor).is_err());
    }
}
