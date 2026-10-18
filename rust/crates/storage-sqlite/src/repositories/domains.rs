//! Purpose: permission, workspace, account (+ account_state, control_account), session_share and
//! data_migration repositories, mirroring the SQL of their TS services.
//! Scope: stateless functions over `&dyn Sql`.
//! Uses:
//! rust/crates/
//! |-- storage-api/src/records/collation.rs
//! |-- storage-api/src/records/crud.rs
//! |-- storage-api/src/records/mod.rs
//! `-- storage-api/src/sql.rs
//! External:
//! `-- url::Url
//! Used by:
//! rust/crates/
//! |-- storage-sqlite/src/repositories/mod.rs
//! `-- storage-sqlite/tests/repositories.rs
//! Invariants: permission ids are `psv_` ascending ids, workspace list sorts by `id.localeCompare`,
//! account_state is the single row `id = 1`, share upserts keep `time_created` and bump `time_updated`.
//! Contract tests: storage-sqlite/tests/repositories.rs

pub mod permission {
    use opencode_schema::PermissionSavedID;
    use opencode_storage_api::records::crud::now_ms;
    use opencode_storage_api::records::{Crud, PermissionRow};
    use opencode_storage_api::{Result, Sql};

    /// `PermissionSaved.list`: optionally filtered by project, engine order.
    pub fn list(sql: &dyn Sql, project_id: Option<&str>) -> Result<Vec<PermissionRow>> {
        match project_id {
            Some(id) => {
                Crud::<PermissionRow>::list(sql, Some("\"project_id\" = ?1"), &[id.into()], None)
            }
            None => Crud::<PermissionRow>::list(sql, None, &[], None),
        }
    }

    /// `PermissionSaved.add`: one row per resource with a fresh `psv_` id, `onConflictDoNothing`
    /// (so duplicates of `(project_id, action, resource)` are skipped). Empty input is a no-op.
    pub fn add(sql: &dyn Sql, project_id: &str, action: &str, resources: &[String]) -> Result<()> {
        let now = now_ms();
        for resource in resources {
            Crud::<PermissionRow>::insert_or_ignore(
                sql,
                &PermissionRow {
                    id: PermissionSavedID::create().into_inner(),
                    project_id: project_id.to_owned(),
                    action: action.to_owned(),
                    resource: resource.clone(),
                    time_created: now,
                    time_updated: now,
                },
            )?;
        }
        Ok(())
    }

    /// `PermissionSaved.remove`.
    pub fn remove(sql: &dyn Sql, id: &str) -> Result<()> {
        Crud::<PermissionRow>::delete(sql, &[id.into()]).map(|_| ())
    }
}

pub mod workspace {
    use opencode_storage_api::records::collation::locale_compare;
    use opencode_storage_api::records::{Crud, WorkspaceRow};
    use opencode_storage_api::{Result, Sql};

    /// `Workspace.create` insert (plain insert, duplicate ids fail).
    pub fn insert(sql: &dyn Sql, row: &WorkspaceRow) -> Result<()> {
        Crud::<WorkspaceRow>::insert(sql, row)
    }

    /// `Workspace.get`.
    pub fn get(sql: &dyn Sql, id: &str) -> Result<Option<WorkspaceRow>> {
        Crud::<WorkspaceRow>::get(sql, &[id.into()])
    }

    /// `Workspace.list`: rows of a project sorted by `a.id.localeCompare(b.id)`.
    pub fn list(sql: &dyn Sql, project_id: &str) -> Result<Vec<WorkspaceRow>> {
        let mut rows = Crud::<WorkspaceRow>::list(
            sql,
            Some("\"project_id\" = ?1"),
            &[project_id.into()],
            None,
        )?;
        rows.sort_by(|a, b| locale_compare(&a.id, &b.id));
        Ok(rows)
    }

    /// `Workspace.remove` final delete.
    pub fn remove(sql: &dyn Sql, id: &str) -> Result<bool> {
        Crud::<WorkspaceRow>::delete(sql, &[id.into()])
    }
}

pub mod account {
    use opencode_storage_api::records::crud::now_ms;
    use opencode_storage_api::records::{
        AccountRow, AccountStateRow, ControlAccountRow, Crud, Secret,
    };
    use opencode_storage_api::{Result, Sql, StorageError};

    /// `ACCOUNT_STATE_ID` in account/repo.ts.
    pub const STATE_ID: i64 = 1;

    /// The active account joined with its org (`AccountRepo.current`).
    #[derive(Debug, Clone, PartialEq)]
    pub struct Active {
        pub account: AccountRow,
        pub active_org_id: Option<String>,
    }

    /// `AccountRepo.active`.
    pub fn active(sql: &dyn Sql) -> Result<Option<Active>> {
        let Some(state) = Crud::<AccountStateRow>::get(sql, &[STATE_ID.into()])? else {
            return Ok(None);
        };
        let Some(account_id) = state.active_account_id.filter(|id| !id.is_empty()) else {
            return Ok(None);
        };
        Ok(get(sql, &account_id)?.map(|account| Active {
            account,
            active_org_id: state.active_org_id,
        }))
    }

    /// `AccountRepo.list`: every account, engine order.
    pub fn list(sql: &dyn Sql) -> Result<Vec<AccountRow>> {
        Crud::<AccountRow>::list(sql, None, &[], None)
    }

    /// `AccountRepo.getRow`.
    pub fn get(sql: &dyn Sql, id: &str) -> Result<Option<AccountRow>> {
        Crud::<AccountRow>::get(sql, &[id.into()])
    }

    /// `AccountRepo.use`: upsert the single state row.
    pub fn set_active(sql: &dyn Sql, account_id: &str, org_id: Option<&str>) -> Result<()> {
        sql.execute(
            "INSERT INTO \"account_state\" (\"id\", \"active_account_id\", \"active_org_id\") VALUES (?1, ?2, ?3) ON CONFLICT (\"id\") DO UPDATE SET \"active_account_id\" = ?2, \"active_org_id\" = ?3",
            &[STATE_ID.into(), account_id.into(), org_id.into()],
        )
        .map(|_| ())
    }

    /// `AccountRepo.remove` body (run inside a transaction): clear the state when it points at the
    /// account, then delete the account.
    pub fn remove(sql: &dyn Sql, account_id: &str) -> Result<()> {
        sql.execute(
            "UPDATE \"account_state\" SET \"active_account_id\" = NULL, \"active_org_id\" = NULL WHERE \"active_account_id\" = ?1",
            &[account_id.into()],
        )?;
        Crud::<AccountRow>::delete(sql, &[account_id.into()]).map(|_| ())
    }

    /// `AccountRepo.persistToken`. Drizzle also bumps `time_updated`.
    pub fn persist_token(
        sql: &dyn Sql,
        account_id: &str,
        access_token: &Secret<String>,
        refresh_token: &Secret<String>,
        expiry: Option<i64>,
    ) -> Result<()> {
        Crud::<AccountRow>::update(
            sql,
            &[account_id.into()],
            &[
                ("access_token", access_token.expose().into()),
                ("refresh_token", refresh_token.expose().into()),
                ("token_expiry", expiry.into()),
            ],
        )
        .map(|_| ())
    }

    /// `AccountRepo.persistAccount` input.
    #[derive(Debug, Clone, PartialEq)]
    pub struct Persist {
        pub id: String,
        pub email: String,
        pub url: String,
        pub access_token: Secret<String>,
        pub refresh_token: Secret<String>,
        pub expiry: i64,
        pub org_id: Option<String>,
    }

    /// `AccountRepo.persistAccount` body (run inside a transaction): upsert the account with a
    /// normalized server url, then make it active.
    pub fn persist(sql: &dyn Sql, input: &Persist) -> Result<()> {
        let url = normalize_server_url(&input.url)?;
        let now = now_ms();
        Crud::<AccountRow>::upsert(
            sql,
            &AccountRow {
                id: input.id.clone(),
                email: input.email.clone(),
                url,
                access_token: input.access_token.clone(),
                refresh_token: input.refresh_token.clone(),
                token_expiry: Some(input.expiry),
                time_created: now,
                time_updated: now,
            },
            &[
                "email",
                "url",
                "access_token",
                "refresh_token",
                "token_expiry",
            ],
        )?;
        set_active(sql, &input.id, input.org_id.as_deref())
    }

    /// `normalizeServerUrl` (account/url.ts): drop query and hash, trim trailing `/` from the path.
    pub fn normalize_server_url(input: &str) -> Result<String> {
        let mut url = url::Url::parse(input)
            .map_err(|error| StorageError::Validation(format!("Invalid URL: {input}: {error}")))?;
        url.set_query(None);
        url.set_fragment(None);
        let origin = url.origin().ascii_serialization();
        let pathname = url.path().trim_end_matches('/');
        if pathname.is_empty() {
            return Ok(origin);
        }
        Ok(format!("{origin}{pathname}"))
    }

    /// Legacy `control_account` rows (read for import, never written by the baseline).
    pub fn control_accounts(sql: &dyn Sql) -> Result<Vec<ControlAccountRow>> {
        Crud::<ControlAccountRow>::list(sql, None, &[], None)
    }

    /// Insert a legacy row (fixtures and import tooling).
    pub fn insert_control_account(sql: &dyn Sql, row: &ControlAccountRow) -> Result<()> {
        Crud::<ControlAccountRow>::insert(sql, row)
    }
}

pub mod session_share {
    use opencode_storage_api::records::crud::now_ms;
    use opencode_storage_api::records::{Crud, Secret, SessionShareRow};
    use opencode_storage_api::{Result, Sql};

    /// `ShareNext.get`.
    pub fn get(sql: &dyn Sql, session_id: &str) -> Result<Option<SessionShareRow>> {
        Crud::<SessionShareRow>::get(sql, &[session_id.into()])
    }

    /// `ShareNext.create` upsert on `session_id`: keeps `time_created`, bumps `time_updated`.
    pub fn upsert(
        sql: &dyn Sql,
        session_id: &str,
        id: &str,
        secret: &Secret<String>,
        url: &str,
    ) -> Result<()> {
        let now = now_ms();
        Crud::<SessionShareRow>::upsert(
            sql,
            &SessionShareRow {
                session_id: session_id.to_owned(),
                id: id.to_owned(),
                secret: secret.clone(),
                url: url.to_owned(),
                time_created: now,
                time_updated: now,
            },
            &["id", "secret", "url"],
        )
    }

    /// `ShareNext.remove`.
    pub fn remove(sql: &dyn Sql, session_id: &str) -> Result<bool> {
        Crud::<SessionShareRow>::delete(sql, &[session_id.into()])
    }
}

pub mod data_migration {
    use opencode_storage_api::records::crud::now_ms;
    use opencode_storage_api::records::{Crud, DataMigrationRow};
    use opencode_storage_api::{Result, Sql};

    /// Completion time of a named data migration, if it ran.
    pub fn completed(sql: &dyn Sql, name: &str) -> Result<Option<i64>> {
        Ok(Crud::<DataMigrationRow>::get(sql, &[name.into()])?.map(|row| row.time_completed))
    }

    /// Record completion (idempotent, first completion time wins).
    pub fn mark(sql: &dyn Sql, name: &str) -> Result<bool> {
        Crud::<DataMigrationRow>::insert_or_ignore(
            sql,
            &DataMigrationRow {
                name: name.to_owned(),
                time_completed: now_ms(),
            },
        )
    }

    /// Every recorded data migration ordered by name.
    pub fn list(sql: &dyn Sql) -> Result<Vec<DataMigrationRow>> {
        Crud::<DataMigrationRow>::list(sql, None, &[], Some("\"name\" ASC"))
    }
}
