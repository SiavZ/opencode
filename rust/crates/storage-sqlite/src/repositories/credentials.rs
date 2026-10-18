//! Purpose: credential repository, port of packages/core/src/credential.ts:51-138 (global credential
//! secrets: all/list/get/create/update/remove) over the `credential` table.
//! Scope: stateless functions over `&dyn Sql`. `create` wraps its delete+insert in a savepoint so it
//! is atomic inside any caller transaction, like the TS `db.transaction`.
//! Uses:
//! rust/crates/
//! |-- schema/src/ids.rs
//! |-- storage-api/src/records/crud.rs
//! |-- storage-api/src/records/mod.rs
//! |-- storage-api/src/records/secret.rs
//! `-- storage-api/src/sql.rs
//! External:
//! |-- serde::{Deserialize, Serialize}
//! `-- serde_json
//! Used by:
//! rust/crates/
//! |-- storage-sqlite/src/repositories/mod.rs
//! `-- storage-sqlite/tests/credentials.rs
//! Invariants: rows without `integration_id` are invisible (all/list/get skip them), lists are ordered
//! by `time_created` ascending, `create` replaces every credential of the integration ("rotation"),
//! new labels default to "default", `update` with neither label nor value is a no-op (an empty label
//! alone counts as absent, like `!updates.label`), stored values are compact JSON of the tagged union and
//! a stored value that fails to decode is a `Decode` error (TS `decodeUnknownSync` dies).
//! Legacy columns `connector_id`, `method_id`, `active` are written NULL and never read, as in TS.
//! Contract tests: storage-sqlite/tests/credentials.rs

use opencode_schema::CredentialID;
use opencode_storage_api::records::crud::now_ms;
use opencode_storage_api::records::{CredentialRow, Crud, Secret};
use opencode_storage_api::{Result, Sql, SqlExt, StorageError, Value};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value as Json};

/// `Credential.Value`: tagged union on `type`. Field order follows the schema so stored JSON matches.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum CredentialValue {
    /// `Credential.OAuth`.
    Oauth {
        #[serde(rename = "methodID")]
        method_id: String,
        refresh: String,
        access: String,
        expires: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        metadata: Option<Map<String, Json>>,
    },
    /// `Credential.Key`.
    Key {
        key: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        metadata: Option<Map<String, Json>>,
    },
}

impl std::fmt::Debug for CredentialValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CredentialValue::Oauth {
                method_id, expires, ..
            } => f
                .debug_struct("Oauth")
                .field("method_id", method_id)
                .field("expires", expires)
                .finish_non_exhaustive(),
            CredentialValue::Key { .. } => f.debug_struct("Key").finish_non_exhaustive(),
        }
    }
}

/// `Credential.Info`.
#[derive(Debug, Clone, PartialEq)]
pub struct CredentialInfo {
    pub id: CredentialID,
    pub integration_id: String,
    pub label: String,
    pub value: Secret<CredentialValue>,
}

fn stored(row: CredentialRow) -> Result<Option<CredentialInfo>> {
    let Some(integration_id) = row.integration_id else {
        return Ok(None);
    };
    let value: CredentialValue =
        serde_json::from_value(row.value.into_inner()).map_err(|error| StorageError::Decode {
            column: "value".into(),
            message: error.to_string(),
        })?;
    Ok(Some(CredentialInfo {
        id: CredentialID::new(row.id),
        integration_id,
        label: row.label,
        value: Secret::new(value),
    }))
}

fn collect(rows: Vec<CredentialRow>) -> Result<Vec<CredentialInfo>> {
    let mut out = Vec::new();
    for row in rows {
        if let Some(info) = stored(row)? {
            out.push(info);
        }
    }
    Ok(out)
}

/// `Credential.all`: every visible credential, oldest first.
pub fn all(sql: &dyn Sql) -> Result<Vec<CredentialInfo>> {
    collect(Crud::<CredentialRow>::list(
        sql,
        None,
        &[],
        Some("\"time_created\" ASC"),
    )?)
}

/// `Credential.list`: credentials of one integration, oldest first.
pub fn list(sql: &dyn Sql, integration_id: &str) -> Result<Vec<CredentialInfo>> {
    collect(Crud::<CredentialRow>::list(
        sql,
        Some("\"integration_id\" = ?1"),
        &[integration_id.into()],
        Some("\"time_created\" ASC"),
    )?)
}

/// `Credential.get`.
pub fn get(sql: &dyn Sql, id: &str) -> Result<Option<CredentialInfo>> {
    match Crud::<CredentialRow>::get(sql, &[id.into()])? {
        Some(row) => stored(row),
        None => Ok(None),
    }
}

/// `Credential.create`: replace any credential of the integration with a fresh `cred_` record.
pub fn create(
    sql: &dyn Sql,
    integration_id: &str,
    value: &CredentialValue,
    label: Option<&str>,
) -> Result<CredentialInfo> {
    let info = CredentialInfo {
        id: CredentialID::create(),
        integration_id: integration_id.to_owned(),
        label: label.unwrap_or("default").to_owned(),
        value: Secret::new(value.clone()),
    };
    let now = now_ms();
    let row = CredentialRow {
        id: info.id.as_str().to_owned(),
        integration_id: Some(info.integration_id.clone()),
        label: info.label.clone(),
        value: Secret::new(to_json(value)?),
        connector_id: None,
        method_id: None,
        active: None,
        time_created: now,
        time_updated: now,
    };
    sql.savepoint(|tx| -> Result<()> {
        tx.execute(
            "DELETE FROM \"credential\" WHERE \"integration_id\" = ?1",
            &[integration_id.into()],
        )?;
        Crud::<CredentialRow>::insert(tx, &row)
    })?;
    Ok(info)
}

/// `Credential.update`: change label and/or value. Neither (or an empty label without value) is a no-op.
pub fn update(
    sql: &dyn Sql,
    id: &str,
    label: Option<&str>,
    value: Option<&CredentialValue>,
) -> Result<()> {
    // `!updates.label && !updates.value`: an empty label alone is a no-op, but with a value it is written.
    if label.is_none_or(str::is_empty) && value.is_none() {
        return Ok(());
    }
    let mut sets: Vec<(&str, Value)> = Vec::new();
    if let Some(label) = label {
        sets.push(("label", label.into()));
    }
    if let Some(value) = value {
        sets.push(("value", Value::json(&to_json(value)?)?));
    }
    Crud::<CredentialRow>::update(sql, &[id.into()], &sets).map(|_| ())
}

/// `Credential.remove`.
pub fn remove(sql: &dyn Sql, id: &str) -> Result<()> {
    Crud::<CredentialRow>::delete(sql, &[id.into()]).map(|_| ())
}

fn to_json(value: &CredentialValue) -> Result<Json> {
    serde_json::to_value(value)
        .map_err(|error| StorageError::Validation(format!("credential value: {error}")))
}
