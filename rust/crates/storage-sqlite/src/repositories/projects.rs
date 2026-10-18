//! Purpose: project and project_directory repositories (packages/opencode/src/project/project.ts and
//! packages/core/src/project/directories.ts SQL).
//! Scope: stateless functions over `&dyn Sql`.
//! Uses:
//! rust/crates/
//! |-- storage-api/src/records/codec.rs
//! |-- storage-api/src/records/crud.rs
//! |-- storage-api/src/records/mod.rs
//! `-- storage-api/src/sql.rs
//! External:
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! |-- storage-sqlite/src/repositories/mod.rs
//! `-- storage-sqlite/tests/repositories.rs
//! Invariants: drizzle drops `undefined` from `set`, so `None` inputs leave the stored column alone on
//! update and upsert, while plain inserts write NULL. Every UPDATE of project or session bumps
//! `time_updated` (drizzle `$onUpdate`) unless the caller sets it.
//! Contract tests: storage-sqlite/tests/repositories.rs

pub mod project {
    use opencode_storage_api::records::codec;
    use opencode_storage_api::records::crud::now_ms;
    use opencode_storage_api::records::{Crud, ProjectRow};
    use opencode_storage_api::{Result, Sql, Value};
    use serde_json::Value as Json;

    /// The global project id (`ProjectV2.ID.global`).
    pub const GLOBAL: &str = "global";

    /// `Project.get` / `select().from(ProjectTable).where(id)`.
    pub fn get(sql: &dyn Sql, id: &str) -> Result<Option<ProjectRow>> {
        Crud::<ProjectRow>::get(sql, &[id.into()])
    }

    /// `Project.list`: every row, engine order.
    pub fn list(sql: &dyn Sql) -> Result<Vec<ProjectRow>> {
        Crud::<ProjectRow>::list(sql, None, &[], None)
    }

    /// Plain insert (used by `Project.migrateID` to copy a project under a new id).
    pub fn insert(sql: &dyn Sql, row: &ProjectRow) -> Result<()> {
        Crud::<ProjectRow>::insert(sql, row)
    }

    /// Fields `Project.fromDirectory` writes. `None` optionals are NULL on insert and untouched on conflict.
    #[derive(Debug, Clone, PartialEq)]
    pub struct Upsert {
        pub id: String,
        pub worktree: String,
        pub vcs: Option<String>,
        pub name: Option<String>,
        pub icon_url: Option<String>,
        pub icon_url_override: Option<String>,
        pub icon_color: Option<String>,
        pub time_created: i64,
        pub time_updated: i64,
        pub time_initialized: Option<i64>,
        pub sandboxes: Vec<String>,
        pub commands: Option<Json>,
    }

    /// `insert(ProjectTable).values(..).onConflictDoUpdate({ target: id, set: .. })` from fromDirectory.
    pub fn upsert(sql: &dyn Sql, input: &Upsert) -> Result<()> {
        let row = ProjectRow {
            id: input.id.clone(),
            worktree: input.worktree.clone(),
            vcs: input.vcs.clone(),
            name: input.name.clone(),
            icon_url: input.icon_url.clone(),
            icon_url_override: input.icon_url_override.clone(),
            icon_color: input.icon_color.clone(),
            time_created: input.time_created,
            time_updated: input.time_updated,
            time_initialized: input.time_initialized,
            sandboxes: input.sandboxes.clone(),
            commands: input.commands.clone(),
        };
        let mut set = vec!["worktree", "vcs"];
        let optional = [
            ("name", input.name.is_some()),
            ("icon_url", input.icon_url.is_some()),
            ("icon_url_override", input.icon_url_override.is_some()),
            ("icon_color", input.icon_color.is_some()),
            ("time_updated", true),
            ("time_initialized", input.time_initialized.is_some()),
            ("sandboxes", true),
            ("commands", input.commands.is_some()),
        ];
        set.extend(optional.iter().filter(|(_, on)| *on).map(|(c, _)| *c));
        Crud::<ProjectRow>::upsert(sql, &row, &set)
    }

    /// `Project.update` input. `None` leaves the column unchanged.
    #[derive(Debug, Clone, Default, PartialEq)]
    pub struct Update {
        pub name: Option<String>,
        pub icon_url: Option<String>,
        pub icon_url_override: Option<String>,
        pub icon_color: Option<String>,
        pub commands: Option<Json>,
    }

    /// `Project.update`: returns the updated row, `None` when the project does not exist.
    pub fn update(sql: &dyn Sql, id: &str, input: &Update) -> Result<Option<ProjectRow>> {
        let mut sets: Vec<(&str, Value)> = Vec::new();
        for (column, value) in [
            ("name", &input.name),
            ("icon_url", &input.icon_url),
            ("icon_url_override", &input.icon_url_override),
            ("icon_color", &input.icon_color),
        ] {
            if let Some(value) = value {
                sets.push((column, value.into()));
            }
        }
        if let Some(commands) = &input.commands {
            sets.push(("commands", Value::json(commands)?));
        }
        sets.push(("time_updated", Value::Integer(now_ms())));
        Crud::<ProjectRow>::update(sql, &[id.into()], &sets)
    }

    /// `Project.setInitialized`: `time_initialized = now` (drizzle also bumps `time_updated`).
    pub fn set_initialized(sql: &dyn Sql, id: &str) -> Result<()> {
        Crud::<ProjectRow>::update(
            sql,
            &[id.into()],
            &[("time_initialized", Value::Integer(now_ms()))],
        )
        .map(|_| ())
    }

    /// `Project.addSandbox` / `removeSandbox` write: replace the list and bump `time_updated`.
    pub fn set_sandboxes(
        sql: &dyn Sql,
        id: &str,
        sandboxes: &[String],
    ) -> Result<Option<ProjectRow>> {
        Crud::<ProjectRow>::update(
            sql,
            &[id.into()],
            &[
                ("sandboxes", codec::absolute_array::encode(sandboxes)?),
                ("time_updated", Value::Integer(now_ms())),
            ],
        )
    }

    /// `Project.addSandbox`: append when missing. `None` when the project does not exist.
    pub fn add_sandbox(sql: &dyn Sql, id: &str, directory: &str) -> Result<Option<ProjectRow>> {
        let Some(row) = get(sql, id)? else {
            return Ok(None);
        };
        let mut sandboxes = row.sandboxes;
        if !sandboxes.iter().any(|s| s == directory) {
            sandboxes.push(directory.to_owned());
        }
        set_sandboxes(sql, id, &sandboxes)
    }

    /// `Project.removeSandbox`: drop every equal entry. `None` when the project does not exist.
    pub fn remove_sandbox(sql: &dyn Sql, id: &str, directory: &str) -> Result<Option<ProjectRow>> {
        let Some(row) = get(sql, id)? else {
            return Ok(None);
        };
        let sandboxes: Vec<String> = row
            .sandboxes
            .into_iter()
            .filter(|s| s != directory)
            .collect();
        set_sandboxes(sql, id, &sandboxes)
    }

    /// `Project.fromDirectory`: move sessions of the global project in `directory` to `project_id`.
    /// Drizzle's `$onUpdate` also sets `session.time_updated = now`.
    pub fn adopt_global_sessions(
        sql: &dyn Sql,
        project_id: &str,
        directory: &str,
    ) -> Result<usize> {
        sql.execute(
            "UPDATE \"session\" SET \"project_id\" = ?1, \"time_updated\" = ?2 WHERE (\"project_id\" = ?3 AND \"directory\" = ?4)",
            &[
                project_id.into(),
                Value::Integer(now_ms()),
                GLOBAL.into(),
                codec::directory::encode(directory)?,
            ],
        )
    }

    /// `Project.migrateID` body (run it inside an immediate transaction, like TS). Copies the old row
    /// to `new_id` when only the old exists, clears the old directory list, re-points sessions (keeping
    /// their `time_updated`) and workspaces, then deletes the old row.
    pub fn migrate_id(sql: &dyn Sql, old_id: &str, new_id: &str) -> Result<()> {
        if old_id == GLOBAL || old_id == new_id {
            return Ok(());
        }
        let old = get(sql, old_id)?;
        let new = get(sql, new_id)?;
        if let (Some(old), None) = (&old, &new) {
            let mut copy = old.clone();
            copy.id = new_id.to_owned();
            copy.time_updated = now_ms();
            insert(sql, &copy)?;
        }
        sql.execute(
            "DELETE FROM \"project_directory\" WHERE \"project_id\" = ?1",
            &[old_id.into()],
        )?;
        sql.execute(
            "UPDATE \"session\" SET \"project_id\" = ?1, \"time_updated\" = \"time_updated\" WHERE \"project_id\" = ?2",
            &[new_id.into(), old_id.into()],
        )?;
        sql.execute(
            "UPDATE \"workspace\" SET \"project_id\" = ?1 WHERE \"project_id\" = ?2",
            &[new_id.into(), old_id.into()],
        )?;
        if old.is_some() {
            Crud::<ProjectRow>::delete(sql, &[old_id.into()])?;
        }
        Ok(())
    }
}

pub mod project_directory {
    use opencode_storage_api::records::codec;
    use opencode_storage_api::records::crud::now_ms;
    use opencode_storage_api::records::{Crud, ProjectDirectoryRow};
    use opencode_storage_api::{Result, Sql, Value};

    /// `ProjectDirectories.create` behavior.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub enum Behavior {
        /// `onConflictDoNothing()` (also the default when `behavior` is omitted).
        #[default]
        Ignore,
        /// `onConflictDoUpdate` of `strategy` when it differs.
        Replace,
    }

    /// One directory as returned by `list`/`get`.
    #[derive(Debug, Clone, PartialEq)]
    pub struct Directory {
        pub directory: String,
        pub strategy: Option<String>,
    }

    /// `ProjectDirectories.create`: true when a row was inserted or its strategy changed.
    pub fn create(
        sql: &dyn Sql,
        project_id: &str,
        directory: &str,
        strategy: Option<&str>,
        behavior: Behavior,
    ) -> Result<bool> {
        let values = [
            project_id.into(),
            codec::absolute::encode(directory)?,
            Value::Null,
            strategy.into(),
            Value::Integer(now_ms()),
        ];
        let insert = "INSERT INTO \"project_directory\" (\"project_id\", \"directory\", \"type\", \"strategy\", \"time_created\") VALUES (?1, ?2, ?3, ?4, ?5)";
        let statement = match (behavior, strategy) {
            (Behavior::Ignore, _) => {
                format!("{insert} ON CONFLICT DO NOTHING RETURNING \"directory\"")
            }
            (Behavior::Replace, Some(_)) => format!(
                "{insert} ON CONFLICT (\"project_id\", \"directory\") DO UPDATE SET \"strategy\" = ?4 WHERE (\"project_directory\".\"strategy\" IS NULL OR \"project_directory\".\"strategy\" <> ?4) RETURNING \"directory\""
            ),
            (Behavior::Replace, None) => format!(
                "{insert} ON CONFLICT (\"project_id\", \"directory\") DO UPDATE SET \"strategy\" = NULL WHERE \"project_directory\".\"strategy\" IS NOT NULL RETURNING \"directory\""
            ),
        };
        Ok(sql.query_row(&statement, &values)?.is_some())
    }

    /// `ProjectDirectories.remove`: true when a row was deleted.
    pub fn remove(sql: &dyn Sql, project_id: &str, directory: &str) -> Result<bool> {
        Crud::<ProjectDirectoryRow>::delete(
            sql,
            &[project_id.into(), codec::absolute::encode(directory)?],
        )
    }

    /// `ProjectDirectories.list`: newest first, then by directory.
    pub fn list(sql: &dyn Sql, project_id: &str) -> Result<Vec<Directory>> {
        Ok(Crud::<ProjectDirectoryRow>::list(
            sql,
            Some("\"project_id\" = ?1"),
            &[project_id.into()],
            Some("\"time_created\" DESC, \"directory\" ASC"),
        )?
        .into_iter()
        .map(|row| Directory {
            directory: row.directory,
            strategy: row.strategy,
        })
        .collect())
    }

    /// `ProjectDirectories.get`.
    pub fn get(sql: &dyn Sql, project_id: &str, directory: &str) -> Result<Option<Directory>> {
        Ok(Crud::<ProjectDirectoryRow>::get(
            sql,
            &[project_id.into(), codec::absolute::encode(directory)?],
        )?
        .map(|row| Directory {
            directory: row.directory,
            strategy: row.strategy,
        }))
    }

    /// `ProjectDirectories.contains`.
    pub fn contains(sql: &dyn Sql, project_id: &str, directory: &str) -> Result<bool> {
        Ok(get(sql, project_id, directory)?.is_some())
    }

    /// Full rows, for callers that need `type` or `time_created`.
    pub fn rows(sql: &dyn Sql, project_id: &str) -> Result<Vec<ProjectDirectoryRow>> {
        Crud::<ProjectDirectoryRow>::list(
            sql,
            Some("\"project_id\" = ?1"),
            &[project_id.into()],
            Some("\"time_created\" DESC, \"directory\" ASC"),
        )
    }
}
