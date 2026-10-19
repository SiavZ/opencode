//! Purpose: durable idempotent prompt admission (`SessionInput.admit` plus `resolvePrompt` from
//! packages/core/src/session.ts), including the historical message-only retry reconciliation that
//! the baseline lacks (contracts/phase-04/contract.md "Historical retry discrepancy").
//! Scope: one read, then either one reconciliation write transaction or one `prompt.admitted` publish.
//! Uses:
//! rust/crates/
//! |-- event-api/src/lib.rs
//! |-- schema/src/session_event.rs
//! |-- schema/src/values.rs
//! |-- session-api/src/lib.rs
//! |-- session-api/src/prompt.rs
//! |-- session-runtime/src/inbox.rs
//! |-- session-runtime/src/projection.rs
//! `-- storage-api/src/lib.rs
//! External:
//! |-- inbox::find
//! |-- inbox::insert_promoted
//! |-- mime_guess::from_ext
//! |-- serde::de::DeserializeOwned
//! |-- serde_json::Value
//! |-- serde_json::Value::as_str
//! |-- serde_json::from_value
//! |-- serde_json::to_string
//! `-- url::Url::parse
//! Used by:
//! rust/crates/
//! |-- session-runtime/src/lib.rs
//! `-- session-runtime/src/service.rs
//! Invariants: admission commits the `session_input` row before returning (the caller wakes only
//! afterwards). An existing inbox row is returned unchanged (exact-retry equivalence is checked by
//! the caller). A visible `user` message without an inbox row is reconciled by inserting a promoted
//! row at the message seq in one immediate transaction; any other visible message conflicts.
//! Contract tests: session-runtime/tests/admission.rs, session-runtime/tests/admission_historical.rs

use opencode_event_api::PublishOptions;
use opencode_schema::session_event::PromptAdmitted;
use opencode_schema::values::{DateTimeUtc, NonNegativeInt};
use opencode_session_api::prompt::{FileAttachment, InputFileAttachment};
use opencode_session_api::{
    Admitted, Delivery, MessageID, Prompt, PromptInput, SessionError, SessionID,
};
use opencode_storage_api::{DatabaseExt, Sql, StorageError, TxOptions};

use crate::inbox;
use crate::projection::{ProjectionError, SessionEvents, event_error, publish, storage_error};

/// One admission request after prompt resolution.
#[derive(Clone, Debug, PartialEq)]
pub struct Admission {
    pub id: MessageID,
    pub session_id: SessionID,
    pub prompt: Prompt,
    pub delivery: Delivery,
}

/// `SessionInput.admit`: return the recorded inbox row for this message ID, reconcile a historical
/// projected prompt, or durably admit a new pending row through `session.next.prompt.admitted`.
pub async fn admit(events: &dyn SessionEvents, input: Admission) -> Result<Admitted, SessionError> {
    let conflict = || SessionError::PromptConflict {
        session_id: input.session_id.clone(),
        message_id: input.id.clone(),
    };
    let reconcile = input.clone();
    let existing = events
        .database()
        .transaction(TxOptions::immediate(), move |tx: &dyn Sql| {
            lookup(tx, &reconcile)
        })
        .await
        .map_err(|error| match error {
            Lookup::Conflict => conflict(),
            Lookup::Storage(error) => storage_error(error),
        })?;
    if let Some(existing) = existing {
        return Ok(existing);
    }

    let timestamp = DateTimeUtc::now();
    let data = PromptAdmitted {
        timestamp,
        session_id: input.session_id.clone(),
        message_id: input.id.clone(),
        prompt: input.prompt.clone(),
        delivery: input.delivery,
    };
    match publish(events, &data, PublishOptions::default()).await {
        Ok(payload) => {
            let seq = payload.durable.map(|durable| durable.seq).ok_or_else(|| {
                SessionError::Storage {
                    message: ProjectionError::MissingSequence.to_string(),
                    retryable: false,
                }
            })?;
            Ok(Admitted {
                admitted_seq: NonNegativeInt::new(seq).ok_or_else(|| SessionError::Storage {
                    message: format!("invalid admitted seq {seq}"),
                    retryable: false,
                })?,
                id: input.id,
                session_id: input.session_id,
                prompt: input.prompt,
                delivery: input.delivery,
                time_created: timestamp,
                promoted_seq: None,
            })
        }
        Err(error) => {
            // A concurrent admission of the same ID won the race: return its row (`catchDefect`).
            let id = input.id.as_str().to_owned();
            let stored = events
                .database()
                .read(move |tx: &dyn Sql| inbox::find(tx, &id))
                .await
                .map_err(storage_error)?;
            if let Some(stored) = stored {
                return Ok(stored);
            }
            match error.projector_source::<ProjectionError>() {
                Some(ProjectionError::LifecycleConflict { .. }) => Err(conflict()),
                _ => Err(event_error(error)),
            }
        }
    }
}

enum Lookup {
    Conflict,
    Storage(StorageError),
}

impl From<StorageError> for Lookup {
    fn from(error: StorageError) -> Self {
        Lookup::Storage(error)
    }
}

/// Inbox lookup plus historical reconciliation, in one immediate transaction so concurrent retries
/// observe either no row or the single reconciled row.
fn lookup(tx: &dyn Sql, input: &Admission) -> Result<Option<Admitted>, Lookup> {
    if let Some(found) = inbox::find(tx, input.id.as_str())? {
        return Ok(Some(found));
    }
    let Some(row) = tx.query_row(
        "SELECT session_id, type, seq, time_created, data FROM session_message WHERE id = ?",
        &[input.id.as_str().into()],
    )?
    else {
        return Ok(None);
    };
    let session: String = row.get("session_id")?;
    let kind: String = row.get("type")?;
    if session != input.session_id.as_str() || kind != "user" {
        return Err(Lookup::Conflict);
    }
    let data: serde_json::Value = row.get_json("data")?;
    let visible = Prompt {
        text: data
            .get("text")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        files: field(&data, "files")?,
        agents: field(&data, "agents")?,
    };
    if serde_json::to_string(&visible).ok() != serde_json::to_string(&input.prompt).ok() {
        return Err(Lookup::Conflict);
    }
    let seq: i64 = row.get("seq")?;
    let time_created: i64 = row.get("time_created")?;
    // Historical messages carry no delivery mode; the retry's requested delivery is recorded.
    inbox::insert_promoted(
        tx,
        &input.id,
        &input.session_id,
        &input.prompt,
        input.delivery,
        seq,
        time_created,
    )?;
    Ok(inbox::find(tx, input.id.as_str())?)
}

fn field<T: serde::de::DeserializeOwned>(
    data: &serde_json::Value,
    key: &str,
) -> Result<Option<T>, StorageError> {
    data.get(key)
        .map(|value| serde_json::from_value(value.clone()))
        .transpose()
        .map_err(|error| StorageError::Decode {
            column: format!("session_message.data.{key}"),
            message: error.to_string(),
        })
}

/// `resolvePrompt`: attach a MIME type to every file (data URI media type, `application/x-directory`
/// for a trailing slash, otherwise the extension lookup with `application/octet-stream` fallback).
pub fn resolve_prompt(input: PromptInput) -> Prompt {
    Prompt {
        text: input.text,
        agents: input.agents,
        files: input
            .files
            .map(|files| files.into_iter().map(resolve_file).collect()),
    }
}

fn resolve_file(file: InputFileAttachment) -> FileAttachment {
    let mime = data_mime(&file.uri).unwrap_or_else(|| {
        let target = match url::Url::parse(&file.uri) {
            Ok(url) => url.path().to_owned(),
            Err(_) => file.name.clone().unwrap_or_else(|| file.uri.clone()),
        };
        if target.ends_with('/') {
            return "application/x-directory".into();
        }
        mime_type(&target)
    });
    FileAttachment {
        uri: file.uri,
        mime,
        name: file.name,
        description: file.description,
        source: file.source,
    }
}

/// `/^data:([^;,]+)[;,]/i`.
fn data_mime(uri: &str) -> Option<String> {
    let prefix = uri.get(..5)?;
    if !prefix.eq_ignore_ascii_case("data:") {
        return None;
    }
    let rest = &uri[5..];
    let end = rest.find([';', ','])?;
    (end > 0).then(|| rest[..end].to_owned())
}

/// `FSUtil.mimeType`: mime-types `lookup(path) || "application/octet-stream"`.
fn mime_type(path: &str) -> String {
    let extension = path
        .rsplit(['/', '\\'])
        .next()
        .and_then(|name| name.rsplit_once('.').map(|(_, ext)| ext))
        .unwrap_or(path);
    mime_guess::from_ext(&extension.to_ascii_lowercase())
        .first_raw()
        .unwrap_or("application/octet-stream")
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(uri: &str, name: Option<&str>) -> InputFileAttachment {
        InputFileAttachment {
            uri: uri.into(),
            name: name.map(Into::into),
            description: None,
            source: None,
        }
    }

    fn mime(uri: &str, name: Option<&str>) -> String {
        resolve_file(file(uri, name)).mime
    }

    #[test]
    fn resolves_mime_like_baseline() {
        // fixture: expectations follow packages/core/test/session-prompt.test.ts and mime-types.
        assert_eq!(
            mime("data:image/png;base64,aGVsbG8=", Some("image.png")),
            "image/png"
        );
        assert_eq!(mime("DATA:text/plain,hi", None), "text/plain");
        assert_eq!(mime("file:///repo/src/", None), "application/x-directory");
        assert_eq!(mime("file:///repo/a.json", None), "application/json");
        assert_eq!(mime("relative/dir/", None), "application/x-directory");
        assert_eq!(mime("notes", Some("notes.md")), "text/markdown");
        assert_eq!(
            mime("file:///repo/LICENSE", None),
            "application/octet-stream"
        );
        assert_eq!(mime("data:,plain", None), "application/octet-stream");
    }
}
