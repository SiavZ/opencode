//! Purpose: observe real provider auth plugin callbacks and canonical SQLite credentials.
//! Scope: native JS bridge, controlled loopback OAuth and temporary isolated catalog cache.
//! Uses:
//! rust/crates/
//! |-- integration-adapters/src/plugins/mod.rs
//! |-- runtime/src/provider_services.rs
//! |-- server/src/services/legacy.rs
//! `-- storage-sqlite/src/lib.rs
//! External:
//! |-- async_trait::async_trait
//! |-- serde_json::json
//! `-- tokio::net::TcpListener
//! Used by:
//! `-- none, entrypoint
//! Invariants: no user login, real credential, provider network, global environment mutation or installs.
//! Contract tests: this file

use opencode_integration_adapters::plugins::{Callbacks, Host, Options};
use opencode_integration_api::{IntegrationError, plugin_abi::PluginInputWire};
use opencode_provider_adapters::catalog::ModelsService;
use opencode_runtime::provider_services::ProviderServices;
use opencode_schema::ids::IntegrationID;
use opencode_server::middleware::legacy_routing::RouteContext;
use opencode_server::services::legacy::{LegacyError, LegacySessions};
use opencode_storage_sqlite::SqliteDatabase;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct CallbacksDenied;
#[async_trait::async_trait]
impl Callbacks for CallbacksDenied {
    async fn request(&self, method: &str, _params: Value) -> Result<Value, IntegrationError> {
        Err(IntegrationError::PermissionDenied(method.into()))
    }
    async fn notification(&self, _method: &str, _params: Value) {}
}

fn interpreter() -> std::path::PathBuf {
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|directory| directory.join("bun"))
        .find(|path| path.is_file())
        .expect("installed bun interpreter")
}

fn error_name(error: LegacyError) -> Value {
    match error {
        LegacyError::Response { status: 400, body } => body,
        error => panic!("unexpected auth error: {error}"),
    }
}

#[tokio::test]
async fn code_oauth_validates_then_persists_canonical_identity_and_rotates() {
    let root = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let token_url = format!("http://{}/token", listener.local_addr().unwrap());
    let token_task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = vec![0; 8192];
        let count = socket.read(&mut request).await.unwrap();
        let request = String::from_utf8_lossy(&request[..count]);
        assert!(request.starts_with("POST /token HTTP/1.1"));
        let response = json!({"type":"success","refresh":"fixture-refresh","access":"fixture-access","expires":123456,"accountId":"fixture-account"}).to_string();
        socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", response.len(), response).as_bytes()).await.unwrap();
    });
    let entry = root.path().join("auth.mjs");
    std::fs::write(&entry, r#"export default async (_input, options) => ({ auth: {
      provider: "alias",
      methods: [
        {type:"api", label:"API key"},
        {type:"oauth", label:"Controlled OAuth", prompts:[{type:"text",key:"tenant",message:"Tenant",placeholder:"local",when:{key:"mode",op:"eq",value:"code"},validate:value=>value === "valid" ? undefined : "invalid tenant"}],
         authorize:async inputs=>({url:options.token,method:"code",instructions:"Use the fixture code",callback:async code=>{
           if (code !== "fixture-code") return {type:"failed"};
           return await (await fetch(options.token,{method:"POST",body:code})).json();
         }})},
        {type:"oauth",label:"Automatic",authorize:async()=>({url:options.token,method:"auto",instructions:"Controlled automatic",callback:async()=>({type:"success",key:"fixture-api",metadata:{tenant:"local"}})})}
      ]
    }})"#).unwrap();
    let host = Host::start(Options {
        interpreter: interpreter(),
        bridge_directory: root.path().join("bridge"),
        directory: root.path().to_owned(),
        environment: BTreeMap::from([("HOME".into(), root.path().display().to_string())]),
        zod_fallback: None,
        callbacks: Arc::new(CallbacksDenied),
    })
    .await
    .unwrap();
    let plugins = host
        .load(
            &entry,
            Some(json!({"token":token_url})),
            PluginInputWire {
                directory: root.path().display().to_string(),
                worktree: root.path().display().to_string(),
                project: json!({"id":"fixture"}),
                server_url: "http://127.0.0.1:1".into(),
            },
        )
        .await
        .unwrap();
    let database = Arc::new(SqliteDatabase::open_in_memory().await.unwrap());
    let models = Arc::new(
        ModelsService::new(
            reqwest::Client::new(),
            "http://127.0.0.1:1".into(),
            root.path().join("models.json"),
            "fixture".into(),
        )
        .with_policy(true, None),
    );
    let service = ProviderServices::new(database,models,json!({"providers":{"alias":{"integrationID":"canonical","env":["FIXTURE_KEY"],"models":{"test":{}}}}}),[("FIXTURE_KEY".into(),"fixture-env".into())].into()).with_plugins(plugins);
    let methods = service.auth_methods().await.unwrap();
    assert_eq!(methods["alias"].as_array().unwrap().len(), 3);
    assert!(methods.to_string().find("validate").is_none());
    assert_eq!(methods["alias"][1]["prompts"][0]["when"]["op"], "eq");
    assert!(
        service
            .oauth_authorize("alias", json!({"method":0}))
            .await
            .unwrap()
            .is_null()
    );
    let missing = error_name(
        service
            .oauth_callback("alias", json!({"method":1}))
            .await
            .unwrap_err(),
    );
    assert_eq!(
        missing,
        json!({"name":"ProviderAuthOauthMissing","data":{"providerID":"alias"}})
    );
    let invalid = error_name(
        service
            .oauth_authorize("alias", json!({"method":1,"inputs":{"tenant":"bad"}}))
            .await
            .unwrap_err(),
    );
    assert_eq!(
        invalid,
        json!({"name":"ProviderAuthValidationFailed","data":{"field":"tenant","message":"invalid tenant"}})
    );
    let authorization = service
        .oauth_authorize("alias", json!({"method":1,"inputs":{"tenant":"valid"}}))
        .await
        .unwrap();
    assert_eq!(authorization["method"], "code");
    assert_eq!(authorization["url"], token_url);
    assert_eq!(
        error_name(
            service
                .oauth_callback("alias", json!({"method":1}))
                .await
                .unwrap_err()
        )["name"],
        "ProviderAuthOauthCodeMissing"
    );
    assert_eq!(
        error_name(
            service
                .oauth_callback("alias", json!({"method":1,"code":"bad"}))
                .await
                .unwrap_err()
        )["name"],
        "ProviderAuthOauthCallbackFailed"
    );
    service
        .oauth_callback("alias", json!({"method":1,"code":"fixture-code"}))
        .await
        .unwrap();
    token_task.await.unwrap();
    let credentials = service.auth_list().await.unwrap();
    assert!(credentials.get("alias").is_none());
    assert_eq!(credentials["canonical"]["refresh"], "fixture-refresh");
    assert_eq!(
        credentials["canonical"]["metadata"]["accountId"],
        "fixture-account"
    );
    assert_eq!(service.catalog().await.unwrap().available_models().len(), 1);
    assert!(
        !service.catalog().await.unwrap().available_models()[0]
            .request
            .body
            .contains_key("apiKey")
    );
    service
        .oauth_authorize("alias", json!({"method":2}))
        .await
        .unwrap();
    service
        .oauth_callback("alias", json!({"method":2}))
        .await
        .unwrap();
    assert_eq!(
        service.auth_list().await.unwrap()["canonical"],
        json!({"type":"api","key":"fixture-api","metadata":{"tenant":"local"}})
    );
    service.auth_remove("alias").await.unwrap();
    assert!(
        service
            .auth_list()
            .await
            .unwrap()
            .as_object()
            .unwrap()
            .is_empty()
    );
    let current = service
        .credentials()
        .await
        .unwrap()
        .active_credential(&IntegrationID::new("canonical"))
        .unwrap()
        .unwrap();
    assert_eq!(serde_json::to_value(current).unwrap()["key"], "fixture-env");
    host.close().await;
}

#[tokio::test]
async fn current_catalog_and_credential_operations_use_actual_database() {
    let root = tempfile::tempdir().unwrap();
    let database = Arc::new(SqliteDatabase::open_in_memory().await.unwrap());
    let models = Arc::new(
        ModelsService::new(
            reqwest::Client::new(),
            "http://127.0.0.1:1".into(),
            root.path().join("models.json"),
            "fixture".into(),
        )
        .with_policy(true, None),
    );
    let service = ProviderServices::new(
        database.clone(),
        models,
        json!({"providers":{"local":{"models":{"configured":{}}}}}),
        Default::default(),
    );
    let context = RouteContext {
        directory: root.path().display().to_string(),
        workspace_id: None,
    };
    let empty = serde_json::Map::new();
    let models = service
        .handle("v2.model.list", &context, &empty, &empty, None)
        .await
        .unwrap();
    assert_eq!(models[0]["id"], "configured");
    let unknown = service
        .handle(
            "v2.provider.get",
            &context,
            json!({"providerID":"absent"}).as_object().unwrap(),
            &empty,
            None,
        )
        .await
        .unwrap_err();
    assert!(matches!(unknown, LegacyError::Response { status: 404, .. }));
    service
        .auth_set("local", json!({"type":"api","key":"fixture-first"}))
        .await
        .unwrap();
    use opencode_storage_api::DatabaseExt;
    let id = database
        .read(|sql| {
            opencode_storage_sqlite::repositories::credentials::all(sql)
                .map(|records| records[0].id.as_str().to_owned())
        })
        .await
        .unwrap();
    service
        .handle(
            "v2.credential.update",
            &context,
            json!({"credentialID":id}).as_object().unwrap(),
            &empty,
            Some(json!({"label":"renamed"})),
        )
        .await
        .unwrap();
    let label = database
        .read(|sql| {
            opencode_storage_sqlite::repositories::credentials::all(sql)
                .map(|records| records[0].label.clone())
        })
        .await
        .unwrap();
    assert_eq!(label, "renamed");
    service
        .handle(
            "v2.credential.remove",
            &context,
            json!({"credentialID":id}).as_object().unwrap(),
            &empty,
            None,
        )
        .await
        .unwrap();
    assert!(
        service
            .auth_list()
            .await
            .unwrap()
            .as_object()
            .unwrap()
            .is_empty()
    );
}
