//! Purpose: retain a Location lease for native terminal services and active attachments.
//! Scope: lifetime-only forwarding adapter, no duplicate terminal state.
//! Uses: locations/LocationLease, platform-api/pty, runtime/location.
//! Used by: runtime/native_services NativeProvider.
//! Invariants: idle eviction cannot shut down a websocket's terminal underneath it.
//! Contract tests: runtime native service acceptance.

use crate::location::{LocationBundle, LocationGlobals};
use async_trait::async_trait;
use opencode_locations::LocationLease;
use opencode_platform_api::pty::{
    PtyAttachment, PtyCreateInput, PtyError, PtyInfo, PtyOutput, PtyService, PtyUpdateInput,
    ReplayFrom,
};
use std::sync::Arc;

type Lease = Arc<LocationLease<LocationBundle, LocationGlobals>>;
#[derive(Debug)]
pub struct LeasedPty(pub Lease);

#[async_trait]
impl PtyService for LeasedPty {
    async fn list(&self) -> Vec<PtyInfo> {
        self.0.native.pty.list().await
    }
    async fn get(&self, id: &str) -> Result<PtyInfo, PtyError> {
        self.0.native.pty.get(id).await
    }
    async fn create(&self, input: PtyCreateInput) -> Result<PtyInfo, PtyError> {
        self.0.native.pty.create(input).await
    }
    async fn update(&self, id: &str, input: PtyUpdateInput) -> Result<PtyInfo, PtyError> {
        self.0.native.pty.update(id, input).await
    }
    async fn remove(&self, id: &str) -> Result<(), PtyError> {
        self.0.native.pty.remove(id).await
    }
    async fn write(&self, id: &str, text: &str) -> Result<(), PtyError> {
        self.0.native.pty.write(id, text).await
    }
    async fn attach(&self, id: &str, from: ReplayFrom) -> Result<Box<dyn PtyAttachment>, PtyError> {
        Ok(Box::new(Attachment {
            _lease: self.0.clone(),
            inner: self.0.native.pty.attach(id, from).await?,
        }))
    }
    async fn shutdown(&self) {
        self.0.native.pty.shutdown().await;
    }
}

#[derive(Debug)]
struct Attachment {
    _lease: Lease,
    inner: Box<dyn PtyAttachment>,
}
#[async_trait]
impl PtyAttachment for Attachment {
    fn replay(&self) -> &str {
        self.inner.replay()
    }
    fn cursor(&self) -> u64 {
        self.inner.cursor()
    }
    fn write(&self, text: &str) -> Result<(), PtyError> {
        self.inner.write(text)
    }
    fn activate(&self) {
        self.inner.activate();
    }
    async fn next(&self) -> Option<PtyOutput> {
        self.inner.next().await
    }
    fn detach(&self) {
        self.inner.detach();
    }
}
