//! Purpose: late-bind the public Session revert service to the same native snapshots as its runner.
//! Scope: Location lookup with a lease held for every snapshot operation.
//! Uses:
//! rust/crates/
//! |-- locations/src/registry.rs
//! |-- platform-api/src/snapshot.rs
//! `-- runtime/src/location.rs
//! External:
//! |-- async_trait::async_trait
//! `-- std::collections::BTreeMap
//! Used by:
//! rust/crates/
//! `-- runtime/src/adapters.rs
//! Invariants: no duplicate snapshot root and no filesystem operation outlives its Location lease.
//! Contract tests: runtime/tests/production.rs, runtime/tests/session_lifecycle.rs

use crate::location::{LocationBundle, LocationGlobals, Locations};
use async_trait::async_trait;
use opencode_platform_api::snapshot::{FileDiff, SnapshotError, SnapshotOperation, Snapshots};
use opencode_schema::location::Ref;
use std::collections::BTreeMap;

#[derive(Debug)]
pub(crate) struct LocationSnapshots {
    pub locations: Locations,
    pub location: Ref,
}

impl LocationSnapshots {
    async fn services(
        &self,
        operation: SnapshotOperation,
    ) -> Result<opencode_locations::LocationLease<LocationBundle, LocationGlobals>, SnapshotError>
    {
        self.locations
            .get(&self.location)
            .await
            .map_err(|error| SnapshotError {
                operation,
                message: error.to_string(),
            })
    }
}

#[async_trait]
impl Snapshots for LocationSnapshots {
    async fn capture(&self) -> Option<String> {
        self.services(SnapshotOperation::Capture)
            .await
            .ok()?
            .snapshots
            .capture()
            .await
    }

    async fn files(&self, from: &str, to: &str) -> Result<Vec<String>, SnapshotError> {
        self.services(SnapshotOperation::Files)
            .await?
            .snapshots
            .files(from, to)
            .await
    }

    async fn diff(
        &self,
        from: &str,
        to: &str,
        context: Option<u32>,
        paths: Option<&[String]>,
    ) -> Result<Vec<FileDiff>, SnapshotError> {
        self.services(SnapshotOperation::Diff)
            .await?
            .snapshots
            .diff(from, to, context, paths)
            .await
    }

    async fn preview(
        &self,
        files: &BTreeMap<String, String>,
        context: Option<u32>,
    ) -> Result<Vec<FileDiff>, SnapshotError> {
        self.services(SnapshotOperation::Preview)
            .await?
            .snapshots
            .preview(files, context)
            .await
    }

    async fn restore(&self, files: &BTreeMap<String, String>) -> Result<(), SnapshotError> {
        self.services(SnapshotOperation::Restore)
            .await?
            .snapshots
            .restore(files)
            .await
    }

    async fn checkout(&self, snapshot: &str) -> Result<(), SnapshotError> {
        self.services(SnapshotOperation::Restore)
            .await?
            .snapshots
            .checkout(snapshot)
            .await
    }
}
