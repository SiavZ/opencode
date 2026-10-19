//! Purpose: persisted project identity (port of packages/opencode/src/project/project.ts:146-310
//! `migrateProjectId`, `saveProjectDirectory`, `fromDirectory`, plus `addSandbox`/`removeSandbox`).
//! Scope: `ProjectStore` over `storage_api::Database` with raw SQL against the baseline tables.
//! Uses:
//! rust/crates/
//! |-- locations/src/error.rs
//! |-- locations/src/git.rs
//! |-- locations/src/project.rs
//! |-- schema/src/ids.rs
//! |-- schema/src/project.rs
//! |-- schema/src/values.rs
//! |-- storage-api/src/lib.rs
//! |-- storage-api/src/records/crud.rs
//! `-- storage-api/src/records/mod.rs
//! External:
//! |-- serde_json::from_value
//! |-- serde_json::to_value
//! |-- tokio::fs::canonicalize
//! |-- tokio::fs::try_exists
//! |-- tracing::info
//! `-- tracing::warn
//! Used by:
//! rust/crates/
//! |-- locations/src/lib.rs
//! |-- locations/src/workspace/worktree.rs
//! `-- runtime/src/legacy.rs
//! Invariants: migration runs in one IMMEDIATE transaction and is a no-op for missing, `global` or
//! unchanged previous ids; it copies the old row only when the new one is absent, clears the old
//! project's directory list, moves sessions (keeping `time_updated`) and workspaces, then deletes
//! the old row, which cascades to its saved permissions exactly like the baseline foreign key.
//! The id cache file is written only after persistence, never for `global`.
//! Contract tests: locations/tests/project.rs

use std::path::Path;
use std::sync::Arc;

use opencode_schema::ids::ProjectID;
use opencode_schema::project::{Commands, Icon, Info, Time, Vcs as VcsLiteral};
use opencode_schema::values::NonNegativeInt;
use opencode_storage_api::records::crud::now_ms;
use opencode_storage_api::records::{Crud, ProjectRow};
use opencode_storage_api::{Database, DatabaseExt, Sql, StorageError, TxOptions, Value};

use crate::error::{LocationError, Result};
use crate::project::ProjectResolver;

const GLOBAL: &str = "global";

/// Result of `Project.fromDirectory`.
#[derive(Debug, Clone, PartialEq)]
pub struct FromDirectory {
    pub project: Info,
    /// The checkout directory for git projects, otherwise the project worktree.
    pub sandbox: String,
}

/// Hook for `Project.Event.Updated` (published on the global bus by the caller).
pub type UpdatedHook = Arc<dyn Fn(&Info) + Send + Sync>;

/// Persisted project service.
#[derive(Clone)]
pub struct ProjectStore {
    db: Arc<dyn Database>,
    resolver: Arc<dyn ProjectResolver>,
    /// `Flag.OPENCODE_FAKE_VCS` (decoded `"git"` or absent).
    pub fake_vcs: bool,
    pub on_updated: Option<UpdatedHook>,
}

impl std::fmt::Debug for ProjectStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProjectStore")
            .field("fake_vcs", &self.fake_vcs)
            .finish_non_exhaustive()
    }
}

impl ProjectStore {
    pub fn new(db: Arc<dyn Database>, resolver: Arc<dyn ProjectResolver>) -> Self {
        Self {
            db,
            resolver,
            fake_vcs: false,
            on_updated: None,
        }
    }

    pub fn database(&self) -> &Arc<dyn Database> {
        &self.db
    }

    /// `Project.migrateProjectId`.
    pub async fn migrate_id(&self, old: Option<&ProjectID>, new: &ProjectID) -> Result<()> {
        let Some(old) = old else { return Ok(()) };
        if old.as_str() == GLOBAL || old == new {
            return Ok(());
        }
        let (old, new) = (old.as_str().to_owned(), new.as_str().to_owned());
        self.db
            .transaction(TxOptions::immediate(), move |sql| {
                migrate(sql, &old, &new).map_err(LocationError::from)
            })
            .await
    }

    /// `Project.fromDirectory`.
    pub async fn from_directory(&self, directory: &str) -> Result<FromDirectory> {
        tracing::info!(directory, "fromDirectory");
        let data = self.resolver.resolve(directory).await?;
        let global = data.id.as_str() == GLOBAL;
        let data_directory = data.directory.to_string_lossy().into_owned();
        let worktree = if global && data.vcs.is_none() {
            "/".to_owned()
        } else {
            data_directory.clone()
        };
        self.migrate_id(data.previous.as_ref(), &data.id).await?;

        let id = data.id.as_str().to_owned();
        let row = {
            let id = id.clone();
            self.db
                .read(move |sql| Crud::<ProjectRow>::get(sql, &[id.into()]))
                .await?
        };
        let now = now_ms();
        let vcs = (data.vcs.is_some() || self.fake_vcs).then_some(VcsLiteral);
        let mut result = match row {
            Some(row) => info_from_row(&row)?,
            None => Info {
                id: data.id.clone(),
                worktree: worktree.clone(),
                vcs,
                name: None,
                icon: None,
                commands: None,
                time: Time {
                    created: non_negative(now),
                    updated: non_negative(now),
                    initialized: None,
                },
                sandboxes: Vec::new(),
            },
        };
        if global {
            result.worktree = worktree.clone();
        }
        result.vcs = vcs;
        result.time.updated = non_negative(now);
        if !global
            && data_directory != result.worktree
            && !result.sandboxes.contains(&data_directory)
        {
            result.sandboxes.push(data_directory.clone());
        }
        let mut sandboxes = Vec::new();
        for sandbox in std::mem::take(&mut result.sandboxes) {
            if tokio::fs::try_exists(&sandbox).await? {
                sandboxes.push(sandbox);
            }
        }
        result.sandboxes = sandboxes;

        let persisted = row_from_info(&result)?;
        let adopt = (!global).then(|| (id.clone(), data_directory.clone()));
        self.db
            .transaction(TxOptions::default(), move |sql| {
                Crud::<ProjectRow>::upsert(
                    sql,
                    &persisted,
                    &[
                        "worktree",
                        "vcs",
                        "name",
                        "icon_url",
                        "icon_url_override",
                        "icon_color",
                        "time_updated",
                        "time_initialized",
                        "sandboxes",
                        "commands",
                    ],
                )?;
                if let Some((id, directory)) = adopt {
                    sql.execute(
                        "UPDATE \"session\" SET \"project_id\" = ?1, \"time_updated\" = ?2 WHERE (\"project_id\" = ?3 AND \"directory\" = ?4)",
                        &[id.into(), Value::Integer(now_ms()), GLOBAL.into(), directory.into()],
                    )?;
                }
                Ok::<_, LocationError>(())
            })
            .await?;

        if !global {
            self.save_directory(&id, &data_directory).await;
        }
        if let Some(hook) = &self.on_updated {
            hook(&result);
        }
        if let (false, Some(vcs)) = (global, &data.vcs) {
            self.resolver.commit(&vcs.store, &data.id).await;
        }
        let sandbox = if data.vcs.is_some() {
            data_directory
        } else {
            worktree
        };
        Ok(FromDirectory {
            project: result,
            sandbox,
        })
    }

    /// `Project.saveProjectDirectory`: failures only log a warning.
    async fn save_directory(&self, project_id: &str, directory: &str) {
        let opened = canonical(directory).await;
        let project = project_id.to_owned();
        let outcome = self
            .db
            .transaction(TxOptions::default(), move |sql| {
                sql.execute(
                    "INSERT INTO \"project_directory\" (\"project_id\", \"directory\", \"type\", \"strategy\", \"time_created\") VALUES (?1, ?2, NULL, NULL, ?3) ON CONFLICT DO NOTHING",
                    &[project.into(), opened.into(), Value::Integer(now_ms())],
                )
            })
            .await;
        if let Err(error) = outcome {
            tracing::warn!(projectID = project_id, %error, "project directory persistence failed");
        }
    }

    pub async fn get(&self, id: &ProjectID) -> Result<Option<Info>> {
        let id = id.as_str().to_owned();
        let row = self
            .db
            .read(move |sql| Crud::<ProjectRow>::get(sql, &[id.into()]))
            .await?;
        row.as_ref().map(info_from_row).transpose()
    }

    /// `Project.addSandbox`: append when missing, emit Updated. Missing project fails with
    /// `Project not found: <id>`.
    pub async fn add_sandbox(&self, id: &ProjectID, directory: &str) -> Result<Info> {
        self.edit_sandboxes(id, directory.to_owned(), true).await
    }

    /// `Project.removeSandbox`.
    pub async fn remove_sandbox(&self, id: &ProjectID, directory: &str) -> Result<Info> {
        self.edit_sandboxes(id, directory.to_owned(), false).await
    }

    async fn edit_sandboxes(&self, id: &ProjectID, directory: String, add: bool) -> Result<Info> {
        let id = id.as_str().to_owned();
        let row = self
            .db
            .transaction(TxOptions::default(), move |sql| {
                let missing = || LocationError::NotFound(format!("Project not found: {id}"));
                let row =
                    Crud::<ProjectRow>::get(sql, &[id.clone().into()])?.ok_or_else(missing)?;
                let mut sandboxes = row.sandboxes;
                if !add {
                    sandboxes.retain(|s| *s != directory);
                } else if !sandboxes.contains(&directory) {
                    sandboxes.push(directory);
                }
                Crud::<ProjectRow>::update(
                    sql,
                    &[id.clone().into()],
                    &[("sandboxes", Value::json(&sandboxes)?)],
                )?
                .ok_or_else(missing)
            })
            .await?;
        let info = info_from_row(&row)?;
        if let Some(hook) = &self.on_updated {
            hook(&info);
        }
        Ok(info)
    }
}

/// Body of `migrateProjectId`'s transaction.
fn migrate(sql: &dyn Sql, old_id: &str, new_id: &str) -> Result<(), StorageError> {
    let old = Crud::<ProjectRow>::get(sql, &[old_id.into()])?;
    let new = Crud::<ProjectRow>::get(sql, &[new_id.into()])?;
    if let (Some(old), None) = (&old, &new) {
        let mut copy = old.clone();
        copy.id = new_id.to_owned();
        copy.time_updated = now_ms();
        Crud::<ProjectRow>::insert(sql, &copy)?;
    }
    // Directories may be shared across diverged checkouts: clear and let them re-populate.
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

/// `FSUtil.resolve`: realpath, or the lexical absolute path when it does not exist.
async fn canonical(directory: &str) -> String {
    match tokio::fs::canonicalize(directory).await {
        Ok(path) => path.to_string_lossy().into_owned(),
        Err(_) => crate::git::normalize(Path::new(directory))
            .to_string_lossy()
            .into_owned(),
    }
}

fn non_negative(value: i64) -> NonNegativeInt {
    NonNegativeInt::new(value).unwrap_or(NonNegativeInt::ZERO)
}

/// `fromRow` of project.ts.
pub fn info_from_row(row: &ProjectRow) -> Result<Info> {
    let icon =
        (row.icon_url.is_some() || row.icon_url_override.is_some() || row.icon_color.is_some())
            .then(|| Icon {
                url: row.icon_url.clone(),
                override_url: row.icon_url_override.clone(),
                color: row.icon_color.clone(),
            });
    let vcs = match row.vcs.as_deref() {
        None => None,
        Some(VcsLiteral::VALUE) => Some(VcsLiteral),
        Some(other) => {
            return Err(LocationError::Validation(format!(
                "Expected \"git\", got \"{other}\""
            )));
        }
    };
    let commands = row
        .commands
        .clone()
        .map(serde_json::from_value::<Commands>)
        .transpose()
        .map_err(|e| LocationError::Validation(e.to_string()))?;
    Ok(Info {
        id: ProjectID::new(row.id.clone()),
        worktree: row.worktree.clone(),
        vcs,
        name: row.name.clone(),
        icon,
        commands,
        time: Time {
            created: non_negative(row.time_created),
            updated: non_negative(row.time_updated),
            initialized: row.time_initialized.map(non_negative),
        },
        sandboxes: row.sandboxes.clone(),
    })
}

fn row_from_info(info: &Info) -> Result<ProjectRow> {
    Ok(ProjectRow {
        id: info.id.as_str().to_owned(),
        worktree: info.worktree.clone(),
        vcs: info.vcs.map(|_| VcsLiteral::VALUE.to_owned()),
        name: info.name.clone(),
        icon_url: info.icon.as_ref().and_then(|i| i.url.clone()),
        icon_url_override: info.icon.as_ref().and_then(|i| i.override_url.clone()),
        icon_color: info.icon.as_ref().and_then(|i| i.color.clone()),
        time_created: info.time.created.get(),
        time_updated: info.time.updated.get(),
        time_initialized: info.time.initialized.map(|t| t.get()),
        sandboxes: info.sandboxes.clone(),
        commands: info
            .commands
            .as_ref()
            .map(serde_json::to_value)
            .transpose()
            .map_err(|e| LocationError::Validation(e.to_string()))?,
    })
}
