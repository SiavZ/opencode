//! Purpose: exercise the real feed backend against a local HTTP server and disposable signatures.
//! Scope: integration fixture, no production keys or network feeds.
//! Uses:
//! rust/crates/
//! `-- desktop-host/src/updater.rs
//! External:
//! |-- axum::{Router, response::Response}
//! |-- base64::Engine
//! |-- futures::stream
//! |-- minisign::{KeyPair, sign}
//! |-- parking_lot::Mutex
//! |-- serde_json::{Value, json}
//! |-- tempfile::TempDir
//! |-- tokio::{net::TcpListener, task::JoinHandle}
//! |-- tokio_util::sync::CancellationToken
//! `-- url::Url
//! Used by:
//! `-- none, entrypoint
//! Invariants: server is shut down and joined, generated test secrets exist only in a tempdir.
//! Contract tests: desktop-host/tests/updater_feed.rs

use axum::{Router, body::Body, http::StatusCode, response::Response, routing::get};
use base64::{Engine, engine::general_purpose::STANDARD};
use opencode_desktop_host::updater::{
    FeedBackend, FeedConfig, MinisignVerifier, UnsupportedInstaller, UpdaterBackend, UpdaterError,
};
use parking_lot::Mutex;
use std::{io::Cursor, sync::Arc, time::Duration};
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

struct Fixture {
    dir: TempDir,
    url: url::Url,
    feed: Arc<Mutex<(StatusCode, String)>>,
    artifact: Arc<Mutex<Vec<u8>>>,
    verifier: Arc<MinisignVerifier>,
    signature: String,
    stop: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}

impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let keys = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        std::fs::write(
            dir.path().join("throwaway.key"),
            keys.sk.to_box(None).unwrap().to_bytes(),
        )
        .unwrap();
        let pubkey = keys.pk.to_box().unwrap().to_bytes();
        std::fs::write(dir.path().join("throwaway.pub"), &pubkey).unwrap();
        let artifact = b"local updater artifact fixture".to_vec();
        let signature = STANDARD.encode(
            minisign::sign(
                Some(&keys.pk),
                &keys.sk,
                Cursor::new(&artifact),
                Some("test-only"),
                None,
            )
            .unwrap()
            .to_bytes(),
        );
        let verifier =
            Arc::new(MinisignVerifier::from_tauri_pubkey(&STANDARD.encode(pubkey)).unwrap());
        let feed = Arc::new(Mutex::new((StatusCode::OK, String::new())));
        let artifact = Arc::new(Mutex::new(artifact));
        let feed_handler = feed.clone();
        let artifact_handler = artifact.clone();
        let app = Router::new()
            .route(
                "/latest.json",
                get(move || {
                    let feed = feed_handler.clone();
                    async move {
                        let (status, body) = feed.lock().clone();
                        Response::builder()
                            .status(status)
                            .body(Body::from(body))
                            .unwrap()
                    }
                }),
            )
            .route(
                "/artifact",
                get(move || {
                    let artifact = artifact_handler.clone();
                    async move { artifact.lock().clone() }
                }),
            )
            .route(
                "/chunked",
                get(|| async {
                    Body::from_stream(futures::stream::iter([
                        Ok::<_, std::io::Error>("1234"),
                        Ok("5678"),
                    ]))
                }),
            )
            .route(
                "/slow",
                get(|| async {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    "{}"
                }),
            )
            .route(
                "/redirect",
                get(|| async { axum::response::Redirect::temporary("/latest.json") }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = url::Url::parse(&format!(
            "http://{}/latest.json",
            listener.local_addr().unwrap()
        ))
        .unwrap();
        let stop = CancellationToken::new();
        let shutdown = stop.clone();
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(shutdown.cancelled_owned())
                .await
                .unwrap();
        });
        let fixture = Self {
            dir,
            url,
            feed,
            artifact,
            verifier,
            signature,
            stop,
            task,
        };
        fixture.release("2.0.0", "darwin-aarch64");
        fixture
    }

    fn release(&self, version: &str, platform: &str) {
        let body = serde_json::json!({"version": version, "platforms": {platform: {"url": self.url.join("artifact").unwrap().as_str(), "signature": self.signature}}});
        *self.feed.lock() = (StatusCode::OK, body.to_string());
    }

    fn config(&self) -> FeedConfig {
        FeedConfig::new(
            self.url.clone(),
            "1.0.0".into(),
            "darwin-aarch64".into(),
            self.dir.path().join("stage"),
        )
    }

    fn backend(&self, config: FeedConfig) -> FeedBackend {
        FeedBackend::new(
            config,
            self.verifier.clone(),
            Arc::new(UnsupportedInstaller),
        )
        .unwrap()
    }

    async fn close(self) {
        self.stop.cancel();
        self.task.await.unwrap();
    }
}

#[tokio::test]
async fn local_feed_verified_stage_and_explicit_install_gap() {
    let fixture = Fixture::new().await;
    let backend = fixture.backend(fixture.config());
    assert!(matches!(
        backend.quit_and_install(),
        Err(UpdaterError::NothingDownloaded)
    ));
    let check = backend.check_for_updates().await.unwrap().unwrap();
    assert!(check.is_update_available);
    assert_eq!(check.version.as_deref(), Some("2.0.0"));
    backend.download_update().await.unwrap();
    assert_eq!(
        std::fs::read(backend.staged_path().unwrap()).unwrap(),
        *fixture.artifact.lock()
    );
    assert!(matches!(
        backend.quit_and_install(),
        Err(UpdaterError::Unsupported(_))
    ));
    println!(
        "127.0.0.1 latest.json -> 2.0.0 -> minisign verified -> staged -> Unsupported install"
    );
    fixture.close().await;
}

#[tokio::test]
async fn tampered_artifact_is_never_staged_and_retry_recovers() {
    let fixture = Fixture::new().await;
    let backend = fixture.backend(fixture.config());
    backend.check_for_updates().await.unwrap();
    let original = fixture.artifact.lock().clone();
    fixture.artifact.lock().push(0);
    assert!(matches!(
        backend.download_update().await,
        Err(UpdaterError::Signature(_))
    ));
    assert!(backend.staged_path().is_none());
    assert!(!fixture.config().staging_dir.exists());
    *fixture.artifact.lock() = original;
    backend.download_update().await.unwrap();
    assert!(backend.staged_path().is_some());
    fixture.close().await;
}

#[tokio::test]
async fn latest_channel_allows_downgrades_but_not_prereleases_or_equal_versions() {
    let fixture = Fixture::new().await;
    let backend = fixture.backend(fixture.config());
    for (version, available) in [
        ("v0.9.0", true),
        ("1.0.0", false),
        ("2.0.0-beta.1", false),
        ("2.0.0", true),
    ] {
        fixture.release(version, "darwin-aarch64");
        assert_eq!(
            backend
                .check_for_updates()
                .await
                .unwrap()
                .unwrap()
                .is_update_available,
            available,
            "{version}"
        );
    }
    fixture.close().await;
}

#[tokio::test]
async fn malformed_missing_platform_http_error_and_no_content_recover() {
    let fixture = Fixture::new().await;
    let backend = fixture.backend(fixture.config());
    *fixture.feed.lock() = (StatusCode::OK, "not json".into());
    assert!(matches!(
        backend.check_for_updates().await,
        Err(UpdaterError::InvalidFeed(_))
    ));
    fixture.release("2.0.0", "windows-x86_64");
    assert!(matches!(
        backend.check_for_updates().await,
        Err(UpdaterError::MissingPlatform(_))
    ));
    *fixture.feed.lock() = (StatusCode::SERVICE_UNAVAILABLE, "".into());
    assert!(
        backend
            .check_for_updates()
            .await
            .unwrap_err()
            .is_retryable()
    );
    *fixture.feed.lock() = (StatusCode::NO_CONTENT, "".into());
    assert!(backend.check_for_updates().await.unwrap().is_none());
    fixture.release("2.0.0", "darwin-aarch64");
    assert!(
        backend
            .check_for_updates()
            .await
            .unwrap()
            .unwrap()
            .is_update_available
    );
    fixture.close().await;
}

#[tokio::test]
async fn response_limits_timeout_and_redirect_are_enforced() {
    let fixture = Fixture::new().await;
    let mut config = fixture.config();
    config.max_feed_bytes = 1;
    assert!(matches!(
        fixture.backend(config).check_for_updates().await,
        Err(UpdaterError::InvalidFeed(_))
    ));
    let mut config = fixture.config();
    config.max_artifact_bytes = 1;
    let backend = fixture.backend(config);
    backend.check_for_updates().await.unwrap();
    assert!(matches!(
        backend.download_update().await,
        Err(UpdaterError::InvalidFeed(_))
    ));
    assert!(backend.staged_path().is_none());
    let mut config = fixture.config();
    config.feed_url = fixture.url.join("slow").unwrap();
    config.timeout = Duration::from_millis(20);
    assert!(
        fixture
            .backend(config)
            .check_for_updates()
            .await
            .unwrap_err()
            .is_retryable()
    );
    let mut config = fixture.config();
    config.feed_url = fixture.url.join("redirect").unwrap();
    assert!(matches!(
        fixture.backend(config).check_for_updates().await,
        Err(UpdaterError::Network {
            retryable: false,
            ..
        })
    ));
    fixture.close().await;
}

#[tokio::test]
async fn staged_disk_tampering_and_persistence_failure_fail_closed() {
    let fixture = Fixture::new().await;
    let backend = fixture.backend(fixture.config());
    backend.check_for_updates().await.unwrap();
    backend.download_update().await.unwrap();
    std::fs::write(backend.staged_path().unwrap(), "modified on disk").unwrap();
    assert!(matches!(
        backend.quit_and_install(),
        Err(UpdaterError::Signature(_))
    ));
    let mut config = fixture.config();
    config.staging_dir = fixture.dir.path().join("not-a-directory");
    std::fs::write(&config.staging_dir, "occupied").unwrap();
    let backend = fixture.backend(config);
    backend.check_for_updates().await.unwrap();
    assert!(matches!(
        backend.download_update().await,
        Err(UpdaterError::Persistence(_))
    ));
    assert!(backend.staged_path().is_none());
    fixture.close().await;
}

#[tokio::test]
async fn chunked_bounds_empty_feed_and_invalid_version_fail_closed() {
    let fixture = Fixture::new().await;
    let mut config = fixture.config();
    config.feed_url = fixture.url.join("chunked").unwrap();
    config.max_feed_bytes = 5;
    assert!(matches!(
        fixture.backend(config).check_for_updates().await,
        Err(UpdaterError::InvalidFeed(_))
    ));
    let backend = fixture.backend(fixture.config());
    *fixture.feed.lock() = (StatusCode::OK, String::new());
    assert!(matches!(
        backend.check_for_updates().await,
        Err(UpdaterError::InvalidFeed(_))
    ));
    fixture.release("not-semver", "darwin-aarch64");
    assert!(matches!(
        backend.check_for_updates().await,
        Err(UpdaterError::InvalidFeed(_))
    ));
    fixture.close().await;
}

#[tokio::test]
async fn invalid_signature_and_wrong_key_are_rejected() {
    let fixture = Fixture::new().await;
    let backend = fixture.backend(fixture.config());
    let mut feed: serde_json::Value = serde_json::from_str(&fixture.feed.lock().1).unwrap();
    feed["platforms"]["darwin-aarch64"]["signature"] = "invalid base64!".into();
    fixture.feed.lock().1 = feed.to_string();
    backend.check_for_updates().await.unwrap();
    assert!(matches!(
        backend.download_update().await,
        Err(UpdaterError::Signature(_))
    ));
    fixture.release("2.0.0", "darwin-aarch64");
    let other = Fixture::new().await;
    let backend = FeedBackend::new(
        fixture.config(),
        other.verifier.clone(),
        Arc::new(UnsupportedInstaller),
    )
    .unwrap();
    backend.check_for_updates().await.unwrap();
    assert!(matches!(
        backend.download_update().await,
        Err(UpdaterError::Signature(_))
    ));
    assert!(backend.staged_path().is_none());
    other.close().await;
    fixture.close().await;
}

#[tokio::test]
async fn insecure_feed_or_artifact_url_is_rejected() {
    let fixture = Fixture::new().await;
    for url in [
        "http://example.com/latest.json",
        "file:///tmp/feed",
        "https://user:secret@example.com/latest.json",
    ] {
        let mut config = fixture.config();
        config.feed_url = url.parse().unwrap();
        assert!(matches!(
            FeedBackend::new(
                config,
                fixture.verifier.clone(),
                Arc::new(UnsupportedInstaller)
            ),
            Err(UpdaterError::InvalidFeed(_))
        ));
    }
    let backend = fixture.backend(fixture.config());
    *fixture.feed.lock() = (StatusCode::OK, serde_json::json!({"version":"2.0.0", "platforms":{"darwin-aarch64":{"url":"http://example.com/artifact", "signature":fixture.signature}}}).to_string());
    assert!(matches!(
        backend.check_for_updates().await,
        Err(UpdaterError::InvalidFeed(_))
    ));
    fixture.close().await;
}
