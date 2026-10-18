//! Purpose: enterprise `Share` service (create/get/remove/removeAdmin/sync/data/syncOld) over an
//! `ObjectStore`, executing hosted-domain sharing rules with the baseline's object operation order.
//! Scope: async orchestration over one store. No locking: read/merge/write is not atomic,
//! matching packages/enterprise/src/core/share.ts (concurrent syncs can lose updates).
//! Uses:
//! rust/crates/
//! |-- hosted-adapters/src/sharing/objects.rs
//! `-- hosted-domain/src/sharing/share.rs
//! External:
//! |-- futures::future::try_join_all
//! `-- uuid::Uuid
//! Used by:
//! rust/crates/
//! |-- hosted-adapters/src/sharing/http.rs
//! |-- hosted-adapters/src/sharing/mod.rs
//! `-- hosted-adapters/tests/sharing_baseline.rs
//! Invariants: `Promise.all` groups in the baseline run concurrently here too (try_join), so the
//! first failure surfaces while siblings may still have been applied. `remove` deletes the info
//! object before listing data prefixes and deletes listed objects one by one (share.ts:134-148).
//! Contract tests: hosted-adapters/tests/sharing_baseline.rs

use futures::future::try_join_all;
use opencode_hosted_domain::sharing::ShareError;
use opencode_hosted_domain::sharing::share::{
    Compaction, ShareData, ShareInfo, Snapshot, authorize, compaction_key, event_prefix, info_key,
    legacy_step, merge, new_info, old_data_key, removal_prefixes, snapshot_key,
};

use crate::sharing::objects::{JsonStore, ObjectStore, StoreError};

fn internal(error: StoreError) -> ShareError {
    ShareError::Internal(error.to_string())
}

pub struct ShareService<'a> {
    pub store: &'a dyn ObjectStore,
    /// `process.env.NODE_ENV === "test"`.
    pub node_env_test: bool,
}

impl ShareService<'_> {
    fn json(&self) -> JsonStore<'_> {
        JsonStore { store: self.store }
    }

    pub async fn get(&self, id: &str) -> Result<Option<ShareInfo>, ShareError> {
        self.json().read(&info_key(id)).await.map_err(internal)
    }

    async fn read_snapshot(&self, id: &str) -> Result<Option<Vec<ShareData>>, ShareError> {
        Ok(self
            .json()
            .read::<Snapshot>(&snapshot_key(id))
            .await
            .map_err(internal)?
            .map(|s| s.data))
    }

    async fn write_snapshot(&self, id: &str, data: Vec<ShareData>) -> Result<(), ShareError> {
        self.json()
            .write(&snapshot_key(id), &Snapshot { data })
            .await
            .map_err(internal)
    }

    async fn legacy(&self, id: &str) -> Result<Vec<ShareData>, ShareError> {
        let json = self.json();
        let compaction: Compaction = json
            .read(&compaction_key(id))
            .await
            .map_err(internal)?
            .unwrap_or_default();
        let list = json
            .list(&event_prefix(id), None, None, compaction.event.clone())
            .await
            .map_err(internal)?;
        if list.is_empty() {
            if !compaction.data.is_empty() {
                self.write_snapshot(id, compaction.data.clone()).await?;
            }
            return Ok(compaction.data);
        }
        // Reads run in reversed order (share.ts:94,102), legacy_step reverses the ascending list.
        let reads = list
            .iter()
            .rev()
            .map(|key| json.read::<Vec<ShareData>>(key));
        let mut contents = try_join_all(reads).await.map_err(internal)?;
        contents.reverse();
        let events: Vec<(Vec<String>, Option<Vec<ShareData>>)> =
            list.into_iter().zip(contents).collect();
        let next = legacy_step(&compaction, &events)?.unwrap_or(compaction);
        futures::try_join!(
            async {
                json.write(&compaction_key(id), &next)
                    .await
                    .map_err(internal)
            },
            self.write_snapshot(id, next.data.clone()),
        )?;
        Ok(next.data)
    }

    /// `Share.create` (share.ts:117-128).
    pub async fn create(&self, session_id: &str) -> Result<ShareInfo, ShareError> {
        let info = new_info(
            session_id,
            uuid::Uuid::new_v4().to_string(),
            self.node_env_test,
        );
        if self.get(&info.id).await?.is_some() {
            return Err(ShareError::AlreadyExists(info.id));
        }
        futures::try_join!(
            async {
                self.json()
                    .write(&info_key(&info.id), &info)
                    .await
                    .map_err(internal)
            },
            self.write_snapshot(&info.id, Vec::new()),
        )?;
        Ok(info)
    }

    /// `Share.remove` (share.ts:134-148).
    pub async fn remove(&self, id: &str, secret: &str) -> Result<(), ShareError> {
        let share = self.get(id).await?;
        authorize(share.as_ref(), id, secret)?;
        let json = self.json();
        json.remove(&info_key(id)).await.map_err(internal)?;
        let lists = removal_prefixes(id)
            .map(|prefix| async move { json.list(&prefix, None, None, None).await });
        let groups = try_join_all(lists).await.map_err(internal)?;
        for item in groups.into_iter().flatten() {
            json.remove(&item).await.map_err(internal)?;
        }
        Ok(())
    }

    /// `Share.removeAdmin` (share.ts:150-154).
    pub async fn remove_admin(&self, id: &str) -> Result<(), ShareError> {
        let share = self
            .get(id)
            .await?
            .ok_or_else(|| ShareError::NotFound(id.into()))?;
        self.remove(&share.id, &share.secret).await
    }

    /// `Share.sync` (share.ts:156-168).
    pub async fn sync(&self, id: &str, secret: &str, data: &[ShareData]) -> Result<(), ShareError> {
        let share = self.get(id).await?;
        authorize(share.as_ref(), id, secret)?;
        let current = match self.read_snapshot(id).await? {
            Some(current) => current,
            None => self.legacy(id).await?,
        };
        self.write_snapshot(id, merge(&[&current, data])?).await
    }

    /// `Share.data` (share.ts:170-172). Missing shares read as `[]` (no info check).
    pub async fn data(&self, id: &str) -> Result<Vec<ShareData>, ShareError> {
        match self.read_snapshot(id).await? {
            Some(data) => Ok(data),
            None => self.legacy(id).await,
        }
    }

    /// `Share.syncOld` (share.ts:174-213). No HTTP route calls it in the baseline.
    pub async fn sync_old(
        &self,
        id: &str,
        secret: &str,
        data: &[ShareData],
    ) -> Result<(), ShareError> {
        let share = self.get(id).await?;
        authorize(share.as_ref(), id, secret)?;
        let json = self.json();
        let writes = data.iter().map(|item| async move {
            let key = old_data_key(id, item)?;
            json.write(&key, &item.data).await.map_err(internal)
        });
        try_join_all(writes).await?;
        Ok(())
    }
}
