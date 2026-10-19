//! Purpose: typed row records for every baseline table (`packages/core/src/**/sql.ts`) and the generic
//! table contract ([`TableRecord`], [`Crud`]) that repositories build on.
//! Scope: pure values plus portable SQL over `&dyn Sql`. No business logic: session, message, part,
//! todo, session_message, session_input, session_context_epoch and event semantics belong to their
//! owners (Phase 04 and event-store), which reuse these structs.
//! Uses:
//! rust/crates/
//! |-- storage-api/src/error.rs
//! |-- storage-api/src/records/codec.rs
//! |-- storage-api/src/records/collation.rs
//! |-- storage-api/src/records/crud.rs
//! |-- storage-api/src/records/path.rs
//! |-- storage-api/src/records/secret.rs
//! `-- storage-api/src/value.rs
//! External:
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! |-- locations/src/project_store.rs
//! |-- locations/src/workspace/mod.rs
//! |-- storage-api/src/lib.rs
//! |-- storage-api/src/records/codec.rs
//! |-- storage-api/src/records/crud.rs
//! |-- storage-sqlite/src/repositories/credentials.rs
//! |-- storage-sqlite/src/repositories/domains.rs
//! |-- storage-sqlite/src/repositories/projects.rs
//! |-- storage-sqlite/tests/repositories.rs
//! |-- storage-sqlite/tests/repositories_accounts.rs
//! `-- storage-sqlite/tests/support/mod.rs
//! Invariants: owned by the p02.repos leaf. Column names and order follow the Drizzle table definition,
//! JSON-mode columns are compact `JSON.stringify`, boolean mode is 0/1, path columns follow
//! database/path.ts. Secret fields use [`Secret`] so `Debug` never prints them.
//! Contract tests: storage-sqlite/tests/repositories.rs, storage-api/src/records/path.rs

pub mod codec;
pub mod collation;
pub mod crud;
pub mod path;
pub mod secret;

pub use crud::Crud;
pub use secret::Secret;

use serde_json::Value as Json;

use crate::error::Result;
use crate::value::{Row, Value};

/// A Drizzle table mapped to one Rust struct.
pub trait TableRecord: Sized {
    /// SQL table name.
    const TABLE: &'static str;
    /// Columns in Drizzle definition order.
    const COLUMNS: &'static [&'static str];
    /// Primary key columns.
    const PRIMARY_KEY: &'static [&'static str];
    /// Decode one selected row (all of [`Self::COLUMNS`]).
    fn from_row(row: &Row) -> Result<Self>;
    /// Encode every column in [`Self::COLUMNS`] order.
    fn to_values(&self) -> Result<Vec<Value>>;
}

macro_rules! record {
    (
        $(#[$meta:meta])*
        $name:ident in $table:literal key [$($pk:literal),+] {
            $( $(#[$fmeta:meta])* $field:ident ($column:literal): $ty:ty => $codec:ident ),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq)]
        pub struct $name {
            $( $(#[$fmeta])* pub $field: $ty, )+
        }

        impl TableRecord for $name {
            const TABLE: &'static str = $table;
            const COLUMNS: &'static [&'static str] = &[$($column),+];
            const PRIMARY_KEY: &'static [&'static str] = &[$($pk),+];

            fn from_row(row: &Row) -> Result<Self> {
                Ok(Self { $( $field: codec::$codec::decode(row, $column)?, )+ })
            }

            fn to_values(&self) -> Result<Vec<Value>> {
                Ok(vec![$( codec::$codec::encode(&self.$field)? ),+])
            }
        }
    };
}

record! {
    /// `ProjectTable` (project/sql.ts).
    ProjectRow in "project" key ["id"] {
        id ("id"): String => text,
        worktree ("worktree"): String => absolute,
        vcs ("vcs"): Option<String> => opt_text,
        name ("name"): Option<String> => opt_text,
        icon_url ("icon_url"): Option<String> => opt_text,
        icon_url_override ("icon_url_override"): Option<String> => opt_text,
        icon_color ("icon_color"): Option<String> => opt_text,
        time_created ("time_created"): i64 => int,
        time_updated ("time_updated"): i64 => int,
        time_initialized ("time_initialized"): Option<i64> => opt_int,
        sandboxes ("sandboxes"): Vec<String> => absolute_array,
        /// `{ start?: string }`.
        commands ("commands"): Option<Json> => opt_json,
    }
}

record! {
    /// `ProjectDirectoryTable` (project/sql.ts).
    ProjectDirectoryRow in "project_directory" key ["project_id", "directory"] {
        project_id ("project_id"): String => text,
        directory ("directory"): String => absolute,
        /// `"main" | "root" | "git_worktree"`.
        kind ("type"): Option<String> => opt_text,
        strategy ("strategy"): Option<String> => opt_text,
        time_created ("time_created"): i64 => int,
    }
}

record! {
    /// `PermissionTable` (permission/sql.ts).
    PermissionRow in "permission" key ["id"] {
        id ("id"): String => text,
        project_id ("project_id"): String => text,
        action ("action"): String => text,
        resource ("resource"): String => text,
        time_created ("time_created"): i64 => int,
        time_updated ("time_updated"): i64 => int,
    }
}

record! {
    /// `WorkspaceTable` (control-plane/workspace.sql.ts). `directory` is plain text, not a path column.
    WorkspaceRow in "workspace" key ["id"] {
        id ("id"): String => text,
        kind ("type"): String => text,
        name ("name"): String => text,
        branch ("branch"): Option<String> => opt_text,
        directory ("directory"): Option<String> => opt_text,
        extra ("extra"): Option<Json> => opt_json,
        project_id ("project_id"): String => text,
        time_used ("time_used"): i64 => int,
    }
}

record! {
    /// `AccountTable` (account/sql.ts).
    AccountRow in "account" key ["id"] {
        id ("id"): String => text,
        email ("email"): String => text,
        url ("url"): String => text,
        access_token ("access_token"): Secret<String> => secret_text,
        refresh_token ("refresh_token"): Secret<String> => secret_text,
        token_expiry ("token_expiry"): Option<i64> => opt_int,
        time_created ("time_created"): i64 => int,
        time_updated ("time_updated"): i64 => int,
    }
}

record! {
    /// `AccountStateTable` (account/sql.ts). Single row with id 1.
    AccountStateRow in "account_state" key ["id"] {
        id ("id"): i64 => int,
        active_account_id ("active_account_id"): Option<String> => opt_text,
        active_org_id ("active_org_id"): Option<String> => opt_text,
    }
}

record! {
    /// `ControlAccountTable` (account/sql.ts, legacy).
    ControlAccountRow in "control_account" key ["email", "url"] {
        email ("email"): String => text,
        url ("url"): String => text,
        access_token ("access_token"): Secret<String> => secret_text,
        refresh_token ("refresh_token"): Secret<String> => secret_text,
        token_expiry ("token_expiry"): Option<i64> => opt_int,
        active ("active"): bool => boolean,
        time_created ("time_created"): i64 => int,
        time_updated ("time_updated"): i64 => int,
    }
}

record! {
    /// `CredentialTable` (credential/sql.ts). `value` stays raw JSON until the repository decodes it,
    /// because TS skips rows without `integration_id` before decoding.
    CredentialRow in "credential" key ["id"] {
        id ("id"): String => text,
        integration_id ("integration_id"): Option<String> => opt_text,
        label ("label"): String => text,
        value ("value"): Secret<Json> => secret_json,
        connector_id ("connector_id"): Option<String> => opt_text,
        method_id ("method_id"): Option<String> => opt_text,
        active ("active"): Option<bool> => opt_boolean,
        time_created ("time_created"): i64 => int,
        time_updated ("time_updated"): i64 => int,
    }
}

record! {
    /// `DataMigrationTable` (data-migration.sql.ts).
    DataMigrationRow in "data_migration" key ["name"] {
        name ("name"): String => text,
        time_completed ("time_completed"): i64 => int,
    }
}

record! {
    /// `SessionShareTable` (share/sql.ts).
    SessionShareRow in "session_share" key ["session_id"] {
        session_id ("session_id"): String => text,
        id ("id"): String => text,
        secret ("secret"): Secret<String> => secret_text,
        url ("url"): String => text,
        time_created ("time_created"): i64 => int,
        time_updated ("time_updated"): i64 => int,
    }
}

record! {
    /// `SessionTable` (session/sql.ts). Plain record, semantics owned by Phase 04.
    SessionRow in "session" key ["id"] {
        id ("id"): String => text,
        project_id ("project_id"): String => text,
        workspace_id ("workspace_id"): Option<String> => opt_text,
        parent_id ("parent_id"): Option<String> => opt_text,
        slug ("slug"): String => text,
        directory ("directory"): String => directory,
        path ("path"): Option<String> => opt_path,
        title ("title"): String => text,
        version ("version"): String => text,
        share_url ("share_url"): Option<String> => opt_text,
        summary_additions ("summary_additions"): Option<i64> => opt_int,
        summary_deletions ("summary_deletions"): Option<i64> => opt_int,
        summary_files ("summary_files"): Option<i64> => opt_int,
        summary_diffs ("summary_diffs"): Option<Json> => opt_json,
        metadata ("metadata"): Option<Json> => opt_json,
        cost ("cost"): f64 => real,
        tokens_input ("tokens_input"): i64 => int,
        tokens_output ("tokens_output"): i64 => int,
        tokens_reasoning ("tokens_reasoning"): i64 => int,
        tokens_cache_read ("tokens_cache_read"): i64 => int,
        tokens_cache_write ("tokens_cache_write"): i64 => int,
        revert ("revert"): Option<Json> => opt_json,
        permission ("permission"): Option<Json> => opt_json,
        agent ("agent"): Option<String> => opt_text,
        model ("model"): Option<Json> => opt_json,
        time_created ("time_created"): i64 => int,
        time_updated ("time_updated"): i64 => int,
        time_compacting ("time_compacting"): Option<i64> => opt_int,
        time_archived ("time_archived"): Option<i64> => opt_int,
    }
}

record! {
    /// `MessageTable` (session/sql.ts, V1 messages).
    MessageRow in "message" key ["id"] {
        id ("id"): String => text,
        session_id ("session_id"): String => text,
        time_created ("time_created"): i64 => int,
        time_updated ("time_updated"): i64 => int,
        data ("data"): Json => json,
    }
}

record! {
    /// `PartTable` (session/sql.ts, V1 parts).
    PartRow in "part" key ["id"] {
        id ("id"): String => text,
        message_id ("message_id"): String => text,
        session_id ("session_id"): String => text,
        time_created ("time_created"): i64 => int,
        time_updated ("time_updated"): i64 => int,
        data ("data"): Json => json,
    }
}

record! {
    /// `TodoTable` (session/sql.ts).
    TodoRow in "todo" key ["session_id", "position"] {
        session_id ("session_id"): String => text,
        content ("content"): String => text,
        status ("status"): String => text,
        priority ("priority"): String => text,
        position ("position"): i64 => int,
        time_created ("time_created"): i64 => int,
        time_updated ("time_updated"): i64 => int,
    }
}

record! {
    /// `SessionMessageTable` (session/sql.ts).
    SessionMessageRow in "session_message" key ["id"] {
        id ("id"): String => text,
        session_id ("session_id"): String => text,
        kind ("type"): String => text,
        seq ("seq"): i64 => int,
        time_created ("time_created"): i64 => int,
        time_updated ("time_updated"): i64 => int,
        data ("data"): Json => json,
    }
}

record! {
    /// `SessionInputTable` (session/sql.ts).
    SessionInputRow in "session_input" key ["id"] {
        id ("id"): String => text,
        session_id ("session_id"): String => text,
        prompt ("prompt"): Json => json,
        delivery ("delivery"): String => text,
        admitted_seq ("admitted_seq"): i64 => int,
        promoted_seq ("promoted_seq"): Option<i64> => opt_int,
        time_created ("time_created"): i64 => int,
    }
}

record! {
    /// `SessionContextEpochTable` (session/sql.ts).
    SessionContextEpochRow in "session_context_epoch" key ["session_id"] {
        session_id ("session_id"): String => text,
        baseline ("baseline"): String => text,
        snapshot ("snapshot"): Json => json,
        baseline_seq ("baseline_seq"): i64 => int,
    }
}

record! {
    /// `EventSequenceTable` (event/sql.ts).
    EventSequenceRow in "event_sequence" key ["aggregate_id"] {
        aggregate_id ("aggregate_id"): String => text,
        seq ("seq"): i64 => int,
        owner_id ("owner_id"): Option<String> => opt_text,
    }
}

record! {
    /// `EventTable` (event/sql.ts).
    EventRow in "event" key ["id"] {
        id ("id"): String => text,
        aggregate_id ("aggregate_id"): String => text,
        seq ("seq"): i64 => int,
        kind ("type"): String => text,
        data ("data"): Json => json,
    }
}
