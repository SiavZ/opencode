//! Purpose: console MySQL tenant repositories (account, auth links, workspace, user/membership,
//! key, BYOK provider, disabled model tables) executing the identity/membership/credential
//! rules from hosted-domain against the baseline schema.
//! Scope: repository calls per request; transactions only where the baseline uses them.
//! Uses:
//! rust/crates/
//! |-- hosted-adapters/src/console/db.rs
//! |-- hosted-adapters/src/console/tenant/account.rs
//! |-- hosted-adapters/src/console/tenant/keys.rs
//! |-- hosted-adapters/src/console/tenant/member.rs
//! |-- hosted-adapters/src/console/tenant/workspace.rs
//! `-- hosted-domain/src/console/common.rs
//! External:
//! |-- mysql_async::prelude::Queryable
//! `-- time::PrimitiveDateTime
//! Used by:
//! rust/bins/
//! |-- hosted/src/console/auth.rs
//! `-- hosted/src/console/workspace.rs
//! rust/crates/
//! |-- hosted-adapters/src/console/auth.rs
//! |-- hosted-adapters/src/console/mod.rs
//! `-- hosted-adapters/tests/console_identity.rs
//! Invariants: every workspace-scoped query filters by the actor workspace id; statements and
//! filters mirror `packages/console/core/src/{account,user,workspace,key,provider,model}.ts`.
//! Contract tests: hosted-adapters/tests/console_identity.rs

mod account;
mod keys;
mod member;
mod workspace;

pub use account::{AccountRow, WorkspaceSummary};
pub use keys::ProviderRow;
pub use member::MemberRow;
pub use workspace::{BlockBatch, WorkspaceInfo};

use opencode_hosted_domain::console::common::ConsoleError;

use super::db::{ConsoleDb, DbError};

/// Repository error: a baseline domain error (message preserved) or a store failure.
#[derive(Debug, thiserror::Error)]
pub enum TenantError {
    #[error(transparent)]
    Console(#[from] ConsoleError),
    #[error(transparent)]
    Db(#[from] DbError),
}

impl From<mysql_async::Error> for TenantError {
    fn from(e: mysql_async::Error) -> Self {
        TenantError::Db(DbError::Mysql(e))
    }
}

pub type TenantResult<T> = Result<T, TenantError>;

/// Tenant repository over the console database.
#[derive(Clone, Debug)]
pub struct TenantStore {
    db: ConsoleDb,
}

impl TenantStore {
    pub fn new(db: ConsoleDb) -> Self {
        Self { db }
    }

    pub fn db(&self) -> &ConsoleDb {
        &self.db
    }
}

/// Formats a MySQL `timestamp(3)` like JS `Date.toISOString()`.
pub fn iso(t: time::PrimitiveDateTime) -> String {
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        t.year(),
        u8::from(t.month()),
        t.day(),
        t.hour(),
        t.minute(),
        t.second(),
        t.millisecond()
    )
}

pub(crate) fn iso_opt(t: Option<time::PrimitiveDateTime>) -> Option<String> {
    t.map(iso)
}

/// `IN (?, ?, ...)` placeholder list for `n >= 1` values.
pub(crate) fn placeholders(n: usize) -> String {
    vec!["?"; n].join(", ")
}
