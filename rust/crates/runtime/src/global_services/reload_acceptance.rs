//! Purpose: prove legacy configuration changes do not dispose canonical retained terminals.
//! Scope: actual native PTY and independent legacy configuration producers.
//! Uses:
//! rust/crates/
//! `-- runtime/tests/global_services.rs
//! External:
//! `-- tokio::time
//! Used by:
//! rust/crates/
//! `-- runtime/tests/global_services.rs
//! Invariants: config reload retains canonical services; explicit disposal still tears them down.
//! Contract tests: runtime/tests/global_services.rs

use super::{Process, ProcessOptions, call, json};
use opencode_platform_api::pty::{PtyCreateInput, PtyStatus};
use opencode_schema::location::Ref;
use opencode_schema::values::AbsolutePath;

#[tokio::test]
async fn legacy_configuration_reload_retains_canonical_pty_until_explicit_disposal() {
    let root = tempfile::tempdir().unwrap();
    let checkout = root.path().canonicalize().unwrap().join("checkout");
    std::fs::create_dir_all(&checkout).unwrap();
    let process = Process::start(ProcessOptions::isolated(root.path())).await.unwrap();
    let location = Ref { directory: AbsolutePath::new(checkout.display().to_string()), workspace_id: None };
    let lease = process.locations.get(&location).await.unwrap();
    let pty = lease.native.pty.clone();
    let created = pty.create(PtyCreateInput {
        command: Some("/bin/sh".into()),
        args: Some(vec!["-c".into(),"printf retained".into()]),
        ..PtyCreateInput::default()
    }).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5),async {
        while pty.get(&created.id).await.unwrap().status != PtyStatus::Exited {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await.unwrap();
    drop(lease);
    for (operation,payload) in [("config.update",json!({"model":"fixture/local"})),("global.config.update",json!({"model":"fixture/global"}))] {
        call(&process,&checkout,operation,json!({}),json!({}),Some(payload)).await.unwrap();
        let lease = process.locations.get(&location).await.unwrap();
        assert_eq!(lease.native.pty.get(&created.id).await.unwrap().status,PtyStatus::Exited);
    }
    call(&process,&checkout,"instance.dispose",json!({}),json!({}),None).await.unwrap();
    assert!(pty.list().await.is_empty());
    process.shutdown().await;
}
