//! Purpose: exercise native reference Git clone and tracking refresh without external services.
//! Scope: temporary repositories and subprocess-local Git URL rewriting only.
//! Uses:
//! rust/crates/
//! `-- integration-adapters/src/references.rs
//! External:
//! |-- tokio::process::Command
//! `-- tempfile::tempdir
//! Used by:
//! `-- none, entrypoint
//! Invariants: Git receives no user config or credentials; cache resets cannot touch the fixture source.
//! Contract tests: this file

use opencode_integration_adapters::references::{Repository, ensure, validate_branch};
use std::{collections::BTreeMap, path::Path};

async fn git(directory: &Path, args: &[&str], environment: &BTreeMap<String, String>) {
    let output = tokio::process::Command::new("git")
        .args(args)
        .current_dir(directory)
        .env_clear()
        .envs(environment)
        .output()
        .await
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
async fn repository_clone_refresh_and_branch_isolation_use_real_git() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    let remote = root.path().join("remote/owner/repo.git");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::create_dir_all(remote.parent().unwrap()).unwrap();
    let environment = BTreeMap::from([
        ("PATH".into(), std::env::var("PATH").unwrap()),
        ("HOME".into(), root.path().display().to_string()),
        ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
        ("GIT_AUTHOR_NAME".into(), "Fixture".into()),
        ("GIT_AUTHOR_EMAIL".into(), "fixture@example.invalid".into()),
        ("GIT_COMMITTER_NAME".into(), "Fixture".into()),
        (
            "GIT_COMMITTER_EMAIL".into(),
            "fixture@example.invalid".into(),
        ),
        ("GIT_CONFIG_COUNT".into(), "1".into()),
        (
            "GIT_CONFIG_KEY_0".into(),
            format!(
                "url.file://{}/.insteadOf",
                root.path().join("remote").display()
            ),
        ),
        (
            "GIT_CONFIG_VALUE_0".into(),
            "https://fixture.invalid/".into(),
        ),
    ]);
    git(
        root.path(),
        &["init", "--bare", remote.to_str().unwrap()],
        &environment,
    )
    .await;
    git(&source, &["init", "-b", "dev"], &environment).await;
    std::fs::write(source.join("README.md"), "first").unwrap();
    git(&source, &["add", "README.md"], &environment).await;
    git(&source, &["commit", "-m", "first"], &environment).await;
    git(
        &source,
        &["remote", "add", "origin", remote.to_str().unwrap()],
        &environment,
    )
    .await;
    git(&source, &["push", "origin", "dev"], &environment).await;
    let repository = Repository::parse("https://fixture.invalid/owner/repo.git", None).unwrap();
    let cache = root.path().join("cache");
    let path = ensure(&repository, &cache, Some("dev"), &environment)
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(path.join("README.md")).unwrap(),
        "first"
    );
    std::fs::write(path.join("README.md"), "discard cached local edit").unwrap();
    std::fs::write(source.join("README.md"), "second").unwrap();
    git(&source, &["commit", "-am", "second"], &environment).await;
    git(&source, &["push", "origin", "dev"], &environment).await;
    assert_eq!(
        ensure(&repository, &cache, Some("dev"), &environment)
            .await
            .unwrap(),
        path
    );
    assert_eq!(
        std::fs::read_to_string(path.join("README.md")).unwrap(),
        "second"
    );
    assert_eq!(
        std::fs::read_to_string(source.join("README.md")).unwrap(),
        "second"
    );
    assert_ne!(
        repository.cache_path(&cache, Some("feature/a")).unwrap(),
        path
    );
    assert!(
        repository
            .cache_path(&cache, Some("feature/a"))
            .unwrap()
            .display()
            .to_string()
            .ends_with("repo@feature%2Fa")
    );
    for value in ["../bad", "--upload-pack=bad", "a..b", ""] {
        assert!(validate_branch(value).is_err());
    }
}
