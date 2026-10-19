//! Purpose: contract tests for single-instance arbitration (Electron `requestSingleInstanceLock`
//! and `second-instance`, `main/index.ts:198-216`).
//! Scope: live local sockets in a tempdir, primary and secondary in one process (unix only;
//! the Windows named-pipe transport compiles behind cfg and needs a Windows runner).
//! Uses:
//! rust/crates/
//! `-- desktop-host/src/lifecycle/single_instance.rs
//! External:
//! |-- tempfile::Builder::new
//! |-- tempfile::TempDir
//! |-- tokio::io::AsyncWriteExt
//! |-- tokio::net::UnixStream::connect
//! `-- tokio::test
//! Used by:
//! `-- none, entrypoint
//! Invariants: tempdir only, never the real user data directory.
//! Contract tests: this file
#![cfg(unix)]

use opencode_desktop_host::lifecycle::single_instance::*;

fn launch(argv: &[&str]) -> SecondInstance {
    SecondInstance {
        argv: argv.iter().map(|a| (*a).to_string()).collect(),
        cwd: Some("/work".into()),
    }
}

fn short_tempdir() -> tempfile::TempDir {
    // Unix socket paths are length limited (~104 bytes on macOS).
    tempfile::Builder::new()
        .prefix("oc-si")
        .tempdir_in("/tmp")
        .expect("tempdir")
}

#[tokio::test]
async fn first_is_primary_and_second_forwards_argv() {
    let dir = short_tempdir();
    let Arbitration::Primary(mut primary) = acquire(dir.path(), "test", launch(&["app"]))
        .await
        .expect("acquire")
    else {
        panic!("first must be primary");
    };
    assert!(dir.path().join(LOCK_FILE).exists());

    let second = acquire(dir.path(), "test", launch(&["app", "opencode://open?x=1"]))
        .await
        .expect("second");
    assert!(matches!(second, Arbitration::Secondary));
    assert_eq!(
        primary.recv().await,
        Some(launch(&["app", "opencode://open?x=1"]))
    );

    // Order of multiple secondaries is preserved.
    for i in 0..3 {
        let arg = format!("--n={i}");
        assert!(matches!(
            acquire(dir.path(), "test", launch(&["app", &arg]))
                .await
                .expect("secondary"),
            Arbitration::Secondary
        ));
    }
    for i in 0..3 {
        assert_eq!(primary.recv().await.unwrap().argv[1], format!("--n={i}"));
    }

    primary.shutdown().await;
    assert!(!dir.path().join(LOCK_FILE).exists());
    assert!(!dir.path().join(SOCKET_FILE).exists());

    // After shutdown the next launch becomes primary again.
    let Arbitration::Primary(next) = acquire(dir.path(), "test", launch(&["app"]))
        .await
        .expect("again")
    else {
        panic!("expected primary after shutdown");
    };
    next.shutdown().await;
}

#[tokio::test]
async fn stale_lock_and_socket_are_reclaimed() {
    let dir = short_tempdir();
    std::fs::write(dir.path().join(LOCK_FILE), "99999\n").expect("lock");
    // A socket file nobody listens on.
    drop(std::os::unix::net::UnixListener::bind(dir.path().join(SOCKET_FILE)).expect("bind"));
    assert!(dir.path().join(SOCKET_FILE).exists());

    let Arbitration::Primary(mut primary) = acquire(dir.path(), "test", launch(&["app"]))
        .await
        .expect("acquire")
    else {
        panic!("stale lock must be reclaimed");
    };
    assert!(matches!(
        acquire(dir.path(), "test", launch(&["b"]))
            .await
            .expect("secondary"),
        Arbitration::Secondary
    ));
    assert_eq!(primary.recv().await.unwrap().argv, vec!["b"]);
    primary.shutdown().await;
}

#[tokio::test]
async fn full_queue_rejects_instead_of_hanging() {
    let dir = short_tempdir();
    let Arbitration::Primary(mut primary) = acquire(dir.path(), "test", launch(&["app"]))
        .await
        .expect("acquire")
    else {
        panic!("primary");
    };
    for _ in 0..SECOND_INSTANCE_QUEUE {
        acquire(dir.path(), "test", launch(&["x"]))
            .await
            .expect("fill");
    }
    let overflow = acquire(dir.path(), "test", launch(&["y"])).await;
    assert!(matches!(overflow, Err(SingleInstanceError::Rejected)));
    assert!(primary.try_recv().is_some());
    primary.shutdown().await;
}

#[tokio::test]
async fn oversized_argv_is_rejected_locally() {
    let dir = short_tempdir();
    let big = "a".repeat(MAX_MESSAGE_BYTES as usize);
    let result = acquire(dir.path(), "test", launch(&[&big])).await;
    assert!(matches!(result, Err(SingleInstanceError::TooLarge)));
}

#[tokio::test]
async fn garbage_client_does_not_break_the_primary() {
    use tokio::io::AsyncWriteExt;
    let dir = short_tempdir();
    let Arbitration::Primary(mut primary) = acquire(dir.path(), "test", launch(&["app"]))
        .await
        .expect("acquire")
    else {
        panic!("primary");
    };
    let mut raw = tokio::net::UnixStream::connect(dir.path().join(SOCKET_FILE))
        .await
        .expect("connect");
    raw.write_all(b"not json\n").await.expect("write");
    drop(raw);
    assert!(matches!(
        acquire(dir.path(), "test", launch(&["ok"]))
            .await
            .expect("secondary"),
        Arbitration::Secondary
    ));
    assert_eq!(primary.recv().await.unwrap().argv, vec!["ok"]);
    primary.shutdown().await;
}

#[tokio::test]
async fn dropping_primary_frees_the_lock() {
    let dir = short_tempdir();
    let first = acquire(dir.path(), "test", launch(&["app"]))
        .await
        .expect("acquire");
    drop(first);
    assert!(!dir.path().join(LOCK_FILE).exists());
    let Arbitration::Primary(next) = acquire(dir.path(), "test", launch(&["app"]))
        .await
        .expect("again")
    else {
        panic!("primary");
    };
    next.shutdown().await;
}
