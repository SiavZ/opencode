//! Purpose: prove cross-connection issuer code consumption and refresh rotation under contention.
//! Scope: fixture SQLite database and joined native threads.
//! Uses:
//! rust/crates/
//! `-- hosted-adapters/src/console/auth/issuer_runtime.rs
//! External:
//! |-- std::sync::Barrier
//! `-- tempfile::tempdir
//! Used by:
//! rust/crates/
//! `-- hosted-adapters/src/console/auth/issuer_runtime/tests.rs
//! Invariants: only one take succeeds; concurrent refresh callers receive one successor.
//! Contract tests: this file

use super::super::*;

#[tokio::test]
async fn provider_selection_and_implicit_tokens_keep_fragment_contract() {
    let server = super::ProviderServer::start().await;
    let directory = tempfile::tempdir().unwrap();
    let issuer = super::runtime(&directory.path().join("issuer.sqlite"), &server.base);
    let admission = super::Admission(std::sync::atomic::AtomicUsize::new(0));
    let mut query = super::authorize("github", false);
    query.remove("provider");
    query.insert("response_type".into(), "token".into());
    let selection = issuer.authorize(&query).await.unwrap();
    assert!(
        selection
            .html
            .as_deref()
            .unwrap()
            .contains("/github/authorize")
    );
    let browser = selection
        .cookie
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .strip_prefix("__Host-issuer_browser=")
        .unwrap()
        .to_string();
    let provider = issuer.start_provider("github", &browser).await.unwrap();
    let destination = url::Url::parse(provider.location.as_deref().unwrap()).unwrap();
    let params: std::collections::HashMap<String, String> =
        destination.query_pairs().into_owned().collect();
    let callback = issuer
        .callback(
            "github",
            &std::collections::HashMap::from([
                ("state".into(), params["state"].clone()),
                ("code".into(), "github-code".into()),
            ]),
            &browser,
            &admission,
        )
        .await
        .unwrap();
    let destination = url::Url::parse(callback.location.as_deref().unwrap()).unwrap();
    let fragment: std::collections::HashMap<String, String> =
        url::form_urlencoded::parse(destination.fragment().unwrap().as_bytes())
            .into_owned()
            .collect();
    assert_eq!(fragment["state"], "client-state");
    assert_eq!(
        issuer.userinfo(&fragment["access_token"]).unwrap()["accountID"],
        "acc_fixture"
    );
    assert!(fragment["refresh_token"].starts_with("acc_fixture:"));
    assert!(destination.query_pairs().all(|(name, _)| name != "code"));
    server.stop().await;
}

#[test]
fn durable_concurrent_consumption_and_refresh_rotation() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("issuer.sqlite");
    let store = SqliteStore::open(&path).unwrap();
    store
        .put("one-time", &json!({"value":true}), now() + 60)
        .unwrap();
    let barrier = Arc::new(std::sync::Barrier::new(4));
    let threads: Vec<_> = (0..4)
        .map(|_| {
            let connection = SqliteStore::open(&path).unwrap();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                connection.take("one-time", now()).unwrap().is_some()
            })
        })
        .collect();
    assert_eq!(
        threads
            .into_iter()
            .map(|thread| usize::from(thread.join().unwrap()))
            .sum::<usize>(),
        1
    );
    let grant = Grant {
        subject: "acc_concurrent".into(),
        properties: json!({"accountID":"acc_concurrent","email":"fixture@anoma.ly","newAccount":false}),
        client_id: "app".into(),
        redirect_uri: "http://localhost/callback".into(),
        challenge: None,
        next: random().unwrap(),
        used: None,
    };
    let key = refresh_key(&grant.subject, "initial");
    store
        .put(
            &key,
            &serde_json::to_value(&grant).unwrap(),
            now() + REFRESH_TTL,
        )
        .unwrap();
    let barrier = Arc::new(std::sync::Barrier::new(4));
    let threads: Vec<_> = (0..4)
        .map(|_| {
            let connection = SqliteStore::open(&path).unwrap();
            let barrier = barrier.clone();
            let key = key.clone();
            std::thread::spawn(move || {
                barrier.wait();
                connection.refresh(&key, now()).unwrap().next
            })
        })
        .collect();
    let successors: Vec<_> = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect();
    assert!(successors.iter().all(|successor| successor == &grant.next));
    assert!(
        store
            .get(&refresh_key(&grant.subject, &grant.next), now())
            .unwrap()
            .is_some()
    );
    assert!(store.refresh(&key, now() + 61).is_err());
}
