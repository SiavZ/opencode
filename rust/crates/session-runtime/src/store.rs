//! Purpose: Session reads over the projected tables, porting packages/core/src/session/store.ts,
//! session/info.ts `fromRow`, and the `list`/`messages` queries of packages/core/src/session.ts.
//! Scope: synchronous queries over `&dyn Sql`, wrapped by async reads on a `Database`.
//! Uses:
//! rust/crates/
//! |-- context/src/history.rs
//! |-- session-api/src/info.rs
//! |-- session-api/src/message.rs
//! |-- session-api/src/operations.rs
//! |-- session-runtime/src/projection.rs
//! `-- storage-api/src/lib.rs
//! External:
//! `-- serde_json::{Map, Value}
//! Used by:
//! rust/crates/
//! |-- session-runtime/src/service.rs
//! `-- session-runtime/tests/admission_store.rs
//! Invariants: falsy optional columns (empty strings, 0 archive time) decode as absent like the
//! baseline truthiness checks; a stored model without variant reads as variant `default`.
//! Message decode failures surface as `MessageDecode` with the row's Session and message IDs.
//! Contract tests: session-runtime/tests/admission_store.rs
use std::sync::Arc;

use opencode_schema::provider::ModelRef;
use opencode_schema::values::{AbsolutePath, DateTimeUtc, Finite, RelativePath};
use opencode_schema::{AgentID, ModelID, ProjectID, ProviderID, VariantID, WorkspaceID};
use opencode_session_api::info::{SessionInfo, SessionTime};
use opencode_session_api::message::{Message, Tokens, from_row as message_from_row};
use opencode_session_api::operations::{
    CursorDirection, ListRequest, ListScope, MessagesRequest, Order,
};
use opencode_session_api::{LocationRef, MessageID, SessionError, SessionID};
use opencode_storage_api::{Database, DatabaseExt, Row, Sql, StorageError, Value};
use serde_json::Map;

use crate::projection::storage_error;

/// Failure of a store read: storage or message decode.
#[derive(Debug)]
pub enum ReadError {
    Storage(StorageError),
    Session(SessionError),
}

impl From<StorageError> for ReadError {
    fn from(error: StorageError) -> Self {
        ReadError::Storage(error)
    }
}

impl From<ReadError> for SessionError {
    fn from(error: ReadError) -> Self {
        match error {
            ReadError::Storage(error) => storage_error(error),
            ReadError::Session(error) => error,
        }
    }
}

/// `SessionStore`: process-global reads keyed by Session or message ID.
#[derive(Clone)]
pub struct SessionStore {
    database: Arc<dyn Database>,
}

impl SessionStore {
    pub fn new(database: Arc<dyn Database>) -> Self {
        Self { database }
    }

    /// `SessionStore.get`.
    pub async fn get(&self, session: &SessionID) -> Result<Option<SessionInfo>, SessionError> {
        let id = session.as_str().to_owned();
        self.database
            .read(move |tx: &dyn Sql| get(tx, &id))
            .await
            .map_err(storage_error)
    }

    /// `SessionStore.message`: the owning Session and decoded message.
    pub async fn message(
        &self,
        message: &MessageID,
    ) -> Result<Option<(SessionID, Message)>, SessionError> {
        let id = message.as_str().to_owned();
        self.database
            .read(move |tx: &dyn Sql| message_by_id(tx, &id))
            .await
            .map_err(SessionError::from)
    }

    /// `SessionStore.context` (`SessionHistory.load`): visible history after epoch and compaction.
    pub async fn context(&self, session: &SessionID) -> Result<Vec<Message>, SessionError> {
        let id = session.as_str().to_owned();
        self.database
            .read(move |tx: &dyn Sql| {
                let rows = opencode_context::history::load(tx, &id).map_err(|error| {
                    ReadError::Session(SessionError::Storage {
                        message: error.to_string(),
                        retryable: false,
                    })
                })?;
                rows.into_iter()
                    .map(|row| {
                        let data = match row.data {
                            serde_json::Value::Object(map) => map,
                            _ => Map::new(),
                        };
                        decode_message(&row.session_id, &row.id, &row.message_type, data)
                    })
                    .collect::<Result<Vec<_>, ReadError>>()
            })
            .await
            .map_err(SessionError::from)
    }

    /// `SessionV2.list`.
    pub async fn list(&self, request: ListRequest) -> Result<Vec<SessionInfo>, SessionError> {
        self.database
            .read(move |tx: &dyn Sql| list(tx, &request))
            .await
            .map_err(storage_error)
    }

    /// `SessionV2.messages` without the existence check.
    pub async fn messages(&self, request: MessagesRequest) -> Result<Vec<Message>, SessionError> {
        self.database
            .read(move |tx: &dyn Sql| messages(tx, &request))
            .await
            .map_err(SessionError::from)
    }
}

const SESSION_COLUMNS: &str = "id, project_id, workspace_id, parent_id, directory, path, title, agent, model, cost, tokens_input, tokens_output, tokens_reasoning, tokens_cache_read, tokens_cache_write, revert, time_created, time_updated, time_archived";

/// Reads one Session row.
pub fn get(tx: &dyn Sql, id: &str) -> Result<Option<SessionInfo>, StorageError> {
    tx.query_row(
        &format!("SELECT {SESSION_COLUMNS} FROM session WHERE id = ?"),
        &[id.into()],
    )?
    .as_ref()
    .map(info_from_row)
    .transpose()
}

/// `fromRow` in packages/core/src/session/info.ts.
pub fn info_from_row(row: &Row) -> Result<SessionInfo, StorageError> {
    let invalid = |column: &str, message: String| StorageError::Decode {
        column: column.into(),
        message,
    };
    let text = |column: &str| -> Result<Option<String>, StorageError> {
        Ok(row
            .get::<Option<String>>(column)?
            .filter(|value| !value.is_empty()))
    };
    let finite = |column: &str| -> Result<Finite, StorageError> {
        let value: f64 = row.get(column)?;
        Finite::new(value).ok_or_else(|| invalid(column, format!("non-finite {value}")))
    };
    let time = |column: &str, value: i64| {
        DateTimeUtc::from_millis(value).ok_or_else(|| invalid(column, format!("bad time {value}")))
    };
    #[derive(serde::Deserialize)]
    struct StoredModel {
        id: String,
        #[serde(rename = "providerID")]
        provider_id: String,
        variant: Option<String>,
    }
    let model = row
        .get_json_opt::<StoredModel>("model")?
        .map(|model| ModelRef {
            id: ModelID::new(model.id),
            provider_id: ProviderID::new(model.provider_id),
            variant: Some(VariantID::new(
                model.variant.unwrap_or_else(|| "default".into()),
            )),
        });
    let archived: Option<i64> = row.get("time_archived")?;
    let id: String = row.get("id")?;
    Ok(SessionInfo {
        id: SessionID::new(id).map_err(|error| invalid("id", error.to_string()))?,
        parent_id: text("parent_id")?
            .map(SessionID::new)
            .transpose()
            .map_err(|error| invalid("parent_id", error.to_string()))?,
        project_id: ProjectID::new(row.get::<String>("project_id")?),
        agent: text("agent")?.map(AgentID::new),
        model,
        cost: finite("cost")?,
        tokens: Tokens {
            input: finite("tokens_input")?,
            output: finite("tokens_output")?,
            reasoning: finite("tokens_reasoning")?,
            cache: opencode_schema::provider::CacheCost {
                read: finite("tokens_cache_read")?,
                write: finite("tokens_cache_write")?,
            },
        },
        time: SessionTime {
            created: time("time_created", row.get("time_created")?)?,
            updated: time("time_updated", row.get("time_updated")?)?,
            archived: archived
                .filter(|value| *value != 0)
                .map(|value| time("time_archived", value))
                .transpose()?,
        },
        title: row.get("title")?,
        location: LocationRef {
            directory: AbsolutePath::new(row.get::<String>("directory")?),
            workspace_id: text("workspace_id")?
                .map(WorkspaceID::new)
                .transpose()
                .map_err(|error| invalid("workspace_id", error.to_string()))?,
        },
        subpath: text("path")?.map(RelativePath::new),
        revert: row.get_json_opt("revert")?,
    })
}

fn decode_message(
    session: &str,
    id: &str,
    kind: &str,
    data: Map<String, serde_json::Value>,
) -> Result<Message, ReadError> {
    message_from_row(id, kind, data).map_err(|_| {
        let error = match (SessionID::new(session), MessageID::new(id)) {
            (Ok(session_id), Ok(message_id)) => SessionError::MessageDecode {
                session_id,
                message_id,
            },
            _ => SessionError::Storage {
                message: format!("invalid ids on message row {session}/{id}"),
                retryable: false,
            },
        };
        ReadError::Session(error)
    })
}

fn decode_row(row: &Row) -> Result<Message, ReadError> {
    let session: String = row.get("session_id")?;
    let id: String = row.get("id")?;
    let kind: String = row.get("type")?;
    decode_message(&session, &id, &kind, row.get_json("data")?)
}

fn message_by_id(tx: &dyn Sql, id: &str) -> Result<Option<(SessionID, Message)>, ReadError> {
    let Some(row) = tx.query_row(
        "SELECT id, session_id, type, data FROM session_message WHERE id = ?",
        &[id.into()],
    )?
    else {
        return Ok(None);
    };
    let message = decode_row(&row)?;
    let session: String = row.get("session_id")?;
    let session = SessionID::new(session).map_err(|error| {
        ReadError::Storage(StorageError::Decode {
            column: "session_id".into(),
            message: error.to_string(),
        })
    })?;
    Ok(Some((session, message)))
}

/// `order` after flipping for `previous` cursors/anchors.
fn effective(order: Option<Order>, previous: bool) -> Order {
    match (order.unwrap_or_default(), previous) {
        (Order::Asc, true) => Order::Desc,
        (Order::Desc, true) => Order::Asc,
        (order, false) => order,
    }
}

fn list(tx: &dyn Sql, input: &ListRequest) -> Result<Vec<SessionInfo>, StorageError> {
    let previous = input.anchor.as_ref().is_some_and(|anchor| {
        anchor.direction == opencode_session_api::info::ListDirection::Previous
    });
    let order = effective(input.order, previous);
    let mut conditions = Vec::new();
    let mut params: Vec<Value> = Vec::new();
    match &input.scope {
        ListScope::All => {}
        ListScope::Directory(directory) => {
            conditions.push("directory = ?".to_owned());
            params.push(directory.as_str().into());
        }
        ListScope::Project { project, .. } => {
            // Baseline accepts `subpath` but does not filter on it.
            conditions.push("project_id = ?".to_owned());
            params.push(project.as_str().into());
        }
    }
    if let Some(workspace) = &input.workspace_id {
        conditions.push("workspace_id = ?".to_owned());
        params.push(workspace.as_str().into());
    }
    if let Some(search) = input.search.as_deref().filter(|search| !search.is_empty()) {
        conditions.push("title LIKE ?".to_owned());
        params.push(format!("%{search}%").into());
    }
    if let Some(anchor) = &input.anchor {
        let op = if order == Order::Asc { ">" } else { "<" };
        conditions.push(format!(
            "(time_created {op} ? OR (time_created = ? AND id {op} ?))"
        ));
        let time = anchor.time.get();
        params.extend([
            Value::Real(time),
            Value::Real(time),
            anchor.id.as_str().into(),
        ]);
    }
    let direction = if order == Order::Asc { "ASC" } else { "DESC" };
    let mut sql = format!("SELECT {SESSION_COLUMNS} FROM session");
    if !conditions.is_empty() {
        sql.push_str(&format!(" WHERE {}", conditions.join(" AND ")));
    }
    sql.push_str(&format!(
        " ORDER BY time_created {direction}, id {direction}"
    ));
    if let Some(limit) = input.limit {
        sql.push_str(&format!(" LIMIT {limit}"));
    }
    let mut rows = tx
        .query(&sql, &params)?
        .iter()
        .map(info_from_row)
        .collect::<Result<Vec<_>, _>>()?;
    if previous {
        rows.reverse();
    }
    Ok(rows)
}

fn messages(tx: &dyn Sql, input: &MessagesRequest) -> Result<Vec<Message>, ReadError> {
    let previous = matches!(input.cursor, Some((_, CursorDirection::Previous)));
    let order = effective(input.order, previous);
    let session = input.session_id.as_str();
    let mut sql =
        "SELECT id, session_id, type, data FROM session_message WHERE session_id = ?".to_owned();
    let mut params: Vec<Value> = vec![session.into()];
    if let Some((cursor, _)) = &input.cursor {
        let Some(anchor) = tx.query_row(
            "SELECT seq FROM session_message WHERE session_id = ? AND id = ?",
            &[session.into(), cursor.as_str().into()],
        )?
        else {
            return Ok(Vec::new());
        };
        sql.push_str(if order == Order::Asc {
            " AND seq > ?"
        } else {
            " AND seq < ?"
        });
        params.push(Value::Integer(anchor.get("seq")?));
    }
    sql.push_str(if order == Order::Asc {
        " ORDER BY seq ASC"
    } else {
        " ORDER BY seq DESC"
    });
    if let Some(limit) = input.limit {
        sql.push_str(&format!(" LIMIT {limit}"));
    }
    let rows = tx.query(&sql, &params)?;
    let ordered: Box<dyn Iterator<Item = &Row>> = if previous {
        Box::new(rows.iter().rev())
    } else {
        Box::new(rows.iter())
    };
    ordered.map(decode_row).collect()
}
