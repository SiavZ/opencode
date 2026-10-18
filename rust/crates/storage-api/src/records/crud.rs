//! Purpose: generic Drizzle-equivalent CRUD over any [`TableRecord`]: insert, insert-or-ignore, upsert,
//! get by primary key, filtered list, partial update and delete.
//! Scope: portable SQL text over `&dyn Sql`. Business rules live in the repositories.
//! Uses:
//! rust/crates/
//! |-- storage-api/src/error.rs
//! |-- storage-api/src/records/mod.rs
//! |-- storage-api/src/sql.rs
//! `-- storage-api/src/value.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! |-- storage-api/src/records/mod.rs
//! `-- storage-sqlite/src/repositories/mod.rs
//! Invariants: mirrors drizzle-orm 1.0.0-rc.2 sqlite-core `buildUpdateSet`: every UPDATE and every
//! `onConflictDoUpdate` SET also assigns `time_updated = Date.now()` when the table has a
//! `$onUpdate` timestamp and the caller did not set it explicitly. Identifiers are always quoted.
//! Contract tests: storage-sqlite/tests/repositories.rs

use std::marker::PhantomData;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::Result;
use crate::records::TableRecord;
use crate::sql::Sql;
use crate::value::Value;

/// `Date.now()` in milliseconds.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// Column carrying Drizzle `Timestamps.time_updated.$onUpdate`.
const ON_UPDATE: &str = "time_updated";

/// Table operations for `T`.
pub struct Crud<T>(PhantomData<T>);

impl<T: TableRecord> Crud<T> {
    fn columns() -> String {
        T::COLUMNS
            .iter()
            .map(|c| format!("\"{c}\""))
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn placeholders(count: usize, start: usize) -> String {
        (start..start + count)
            .map(|i| format!("?{i}"))
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn key_where(start: usize) -> String {
        T::PRIMARY_KEY
            .iter()
            .enumerate()
            .map(|(i, c)| format!("\"{c}\" = ?{}", start + i))
            .collect::<Vec<_>>()
            .join(" AND ")
    }

    /// SET clause for `sets`, adding `time_updated = now` like drizzle when the table has it.
    fn set_clause(sets: &[(&str, Value)], start: usize) -> (String, Vec<Value>) {
        let mut parts = Vec::new();
        let mut values = Vec::new();
        for (column, value) in sets {
            parts.push(format!("\"{column}\" = ?{}", start + values.len()));
            values.push(value.clone());
        }
        if T::COLUMNS.contains(&ON_UPDATE) && !sets.iter().any(|(c, _)| *c == ON_UPDATE) {
            parts.push(format!("\"{ON_UPDATE}\" = ?{}", start + values.len()));
            values.push(Value::Integer(now_ms()));
        }
        (parts.join(", "), values)
    }

    fn insert_sql(conflict: &str) -> String {
        format!(
            "INSERT INTO \"{}\" ({}) VALUES ({}){conflict}",
            T::TABLE,
            Self::columns(),
            Self::placeholders(T::COLUMNS.len(), 1)
        )
    }

    /// Plain INSERT. Constraint failures surface as typed errors.
    pub fn insert(sql: &dyn Sql, row: &T) -> Result<()> {
        sql.execute(&Self::insert_sql(""), &row.to_values()?)
            .map(|_| ())
    }

    /// `onConflictDoNothing()`. Returns true when a row was inserted.
    pub fn insert_or_ignore(sql: &dyn Sql, row: &T) -> Result<bool> {
        sql.execute(
            &Self::insert_sql(" ON CONFLICT DO NOTHING"),
            &row.to_values()?,
        )
        .map(|n| n > 0)
    }

    /// `onConflictDoUpdate({ target: primary key, set: update })`. The SET receives `time_updated`
    /// automatically when the table has it. Each `update` column takes the inserted row's value.
    pub fn upsert(sql: &dyn Sql, row: &T, update: &[&str]) -> Result<()> {
        let values = row.to_values()?;
        let sets: Vec<(&str, Value)> = update
            .iter()
            .map(|column| {
                let index = T::COLUMNS
                    .iter()
                    .position(|c| c == column)
                    .unwrap_or_else(|| panic!("{} has no column {column}", T::TABLE));
                (*column, values[index].clone())
            })
            .collect();
        let (set, extra) = Self::set_clause(&sets, values.len() + 1);
        let target = T::PRIMARY_KEY
            .iter()
            .map(|c| format!("\"{c}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let statement = Self::insert_sql(&format!(" ON CONFLICT ({target}) DO UPDATE SET {set}"));
        let params: Vec<Value> = values.into_iter().chain(extra).collect();
        sql.execute(&statement, &params).map(|_| ())
    }

    /// Row by primary key.
    pub fn get(sql: &dyn Sql, key: &[Value]) -> Result<Option<T>> {
        let statement = format!(
            "SELECT {} FROM \"{}\" WHERE {}",
            Self::columns(),
            T::TABLE,
            Self::key_where(1)
        );
        sql.query_row(&statement, key)?
            .map(|row| T::from_row(&row))
            .transpose()
    }

    /// Rows matching an optional raw WHERE clause (parameters `?1..`) with an optional ORDER BY.
    pub fn list(
        sql: &dyn Sql,
        filter: Option<&str>,
        params: &[Value],
        order: Option<&str>,
    ) -> Result<Vec<T>> {
        let mut statement = format!("SELECT {} FROM \"{}\"", Self::columns(), T::TABLE);
        if let Some(filter) = filter {
            statement.push_str(" WHERE ");
            statement.push_str(filter);
        }
        if let Some(order) = order {
            statement.push_str(" ORDER BY ");
            statement.push_str(order);
        }
        sql.query(&statement, params)?
            .iter()
            .map(T::from_row)
            .collect()
    }

    /// `update(table).set(sets).where(pk = key)`, returning the updated row (`.returning().get()`).
    pub fn update(sql: &dyn Sql, key: &[Value], sets: &[(&str, Value)]) -> Result<Option<T>> {
        let (set, values) = Self::set_clause(sets, 1);
        let statement = format!(
            "UPDATE \"{}\" SET {set} WHERE {} RETURNING {}",
            T::TABLE,
            Self::key_where(values.len() + 1),
            Self::columns()
        );
        let params: Vec<Value> = values.into_iter().chain(key.iter().cloned()).collect();
        sql.query_row(&statement, &params)?
            .map(|row| T::from_row(&row))
            .transpose()
    }

    /// Delete by primary key. Returns true when a row was deleted.
    pub fn delete(sql: &dyn Sql, key: &[Value]) -> Result<bool> {
        let statement = format!("DELETE FROM \"{}\" WHERE {}", T::TABLE, Self::key_where(1));
        sql.execute(&statement, key).map(|n| n > 0)
    }
}
