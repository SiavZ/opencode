//! Purpose: regress concurrent attachment budget and token revocation behavior.
//! Scope: public authorization boundary with a deterministic blocked reader fixture.
//! Uses:
//! rust/crates/
//! `-- desktop-host/src/attachments.rs
//! External:
//! |-- parking_lot::Mutex
//! |-- tokio::join
//! |-- tokio::select
//! |-- tokio::sync::Semaphore
//! `-- tokio::test
//! Used by:
//! `-- none, entrypoint
//! Invariants: no real user paths or data are read.
//! Contract tests: desktop-host/tests/attachments_concurrency.rs

use opencode_desktop_host::attachments::{
    AttachmentError, AttachmentReader, BoxFuture, PickedFiles, PickerMessages,
};
use parking_lot::Mutex;
use std::sync::Arc;
use tokio::sync::Semaphore;

struct GatedReader {
    entered: Arc<Semaphore>,
    release: Arc<Semaphore>,
}

impl AttachmentReader for GatedReader {
    fn read<'a>(&'a self, _: &'a str, _: u64) -> BoxFuture<'a, Vec<u8>> {
        Box::pin(async move {
            self.entered.add_permits(1);
            self.release.acquire().await.unwrap().forget();
            Ok(vec![0; 6])
        })
    }
}

fn fixture(budget: u64) -> (PickedFiles, Arc<Semaphore>, Arc<Semaphore>) {
    let entered = Arc::new(Semaphore::new(0));
    let release = Arc::new(Semaphore::new(0));
    let auth = PickedFiles::with_reader(
        Box::new(GatedReader {
            entered: entered.clone(),
            release: release.clone(),
        }),
        budget,
        Arc::new(Mutex::new(PickerMessages::default())),
    );
    (auth, entered, release)
}

#[tokio::test]
async fn concurrent_reads_cannot_return_more_than_the_selection_budget() {
    let (auth, entered, release) = fixture(10);
    let token = auth.add(1, ["a".into(), "b".into()]);
    let (a, b, ()) = tokio::join!(
        auth.read(1, &token, "a"),
        auth.read(1, &token, "b"),
        async {
            entered.acquire_many(2).await.unwrap().forget();
            release.add_permits(2);
        }
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    assert!(
        matches!(a, Err(AttachmentError::SizeLimit(_)))
            || matches!(b, Err(AttachmentError::SizeLimit(_)))
    );
    assert!(auth.is_empty());
}

#[tokio::test]
async fn concurrent_reads_within_budget_both_succeed_and_release_the_token() {
    let (auth, entered, release) = fixture(12);
    let token = auth.add(1, ["a".into(), "b".into()]);
    let (a, b, ()) = tokio::join!(
        auth.read(1, &token, "a"),
        auth.read(1, &token, "b"),
        async {
            entered.acquire_many(2).await.unwrap().forget();
            release.add_permits(2);
        }
    );
    assert_eq!(a.unwrap().len() + b.unwrap().len(), 12);
    assert!(auth.is_empty());
}

#[tokio::test]
async fn cancelling_last_read_releases_consumed_token() {
    let (auth, entered, _) = fixture(12);
    let token = auth.add(1, ["a".into()]);
    let mut read = Box::pin(auth.read(1, &token, "a"));
    tokio::select! {
        _ = &mut read => panic!("gated reader must remain pending"),
        permit = entered.acquire() => permit.unwrap().forget(),
    }
    drop(read);
    assert!(auth.is_empty());
}

#[tokio::test]
async fn destroying_sender_revokes_in_flight_reads() {
    let (auth, entered, release) = fixture(12);
    let token = auth.add(1, ["a".into()]);
    let (result, ()) = tokio::join!(auth.read(1, &token, "a"), async {
        entered.acquire().await.unwrap().forget();
        auth.release_sender(1);
        release.add_permits(1);
    });
    assert!(matches!(result, Err(AttachmentError::NotSelected(_))));
    assert!(auth.is_empty());
}
