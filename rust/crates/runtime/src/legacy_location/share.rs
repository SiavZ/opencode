//! Purpose: real hosted legacy share transport and durable secret storage.
//! Scope: explicit share/unshare calls and initial persisted transcript synchronization.
//! Invariants: remote success precedes local URL mutation; share credentials never enter event payloads.
//! Contract tests: runtime/tests/legacy_operations.rs

use opencode_session_runtime::legacy::ports::{PortError, ShareRemote};
use opencode_session_runtime::legacy::store::LegacyStore;
use opencode_storage_api::records::Secret;
use opencode_storage_api::{DatabaseExt, TxOptions};
use opencode_storage_sqlite::repositories::session_share;
use serde_json::{Value, json};

pub struct Remote {
    store: LegacyStore,
    base: String,
    client: reqwest::Client,
}

impl Remote {
    pub fn new(store: LegacyStore, config: &Value) -> Self {
        Self {
            store,
            base: config
                .get("enterprise")
                .and_then(|value| value.get("url"))
                .and_then(Value::as_str)
                .unwrap_or("https://opncd.ai")
                .trim_end_matches('/')
                .into(),
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait::async_trait]
impl ShareRemote for Remote {
    async fn create(&self, session: &str) -> Result<String, PortError> {
        self.store.get(session).await.map_err(failed)?;
        let response = self
            .client
            .post(format!("{}/api/share", self.base))
            .json(&json!({"sessionID":session}))
            .send()
            .await
            .map_err(failed)?
            .error_for_status()
            .map_err(failed)?
            .json::<Value>()
            .await
            .map_err(failed)?;
        let field = |name: &str| {
            response
                .get(name)
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| PortError::Failed(format!("Invalid share response: missing {name}")))
        };
        let id = field("id")?;
        let url = field("url")?;
        let secret = field("secret")?;
        let (session_key, id_key, secret_key, url_key) =
            (session.to_owned(), id.clone(), secret.clone(), url.clone());
        self.store
            .events()
            .database()
            .transaction(TxOptions::default(), move |sql| {
                session_share::upsert(
                    sql,
                    &session_key,
                    &id_key,
                    &Secret::new(secret_key),
                    &url_key,
                )
            })
            .await
            .map_err(failed)?;
        let mut data =
            vec![json!({"type":"session","data":self.store.get(session).await.map_err(failed)?})];
        for message in self.store.messages(session).await.map_err(failed)? {
            data.push(json!({"type":"message","data":message.info}));
            data.extend(
                message
                    .parts
                    .into_iter()
                    .map(|part| json!({"type":"part","data":part})),
            );
        }
        // Baseline initial synchronization is best effort after the credential row is committed.
        if let Err(error) = self
            .client
            .post(format!("{}/api/share/{id}/sync", self.base))
            .json(&json!({"secret":secret,"data":data}))
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
        {
            tracing::warn!(%error,"initial share sync failed");
        }
        Ok(url)
    }
    async fn remove(&self, session: &str) -> Result<(), PortError> {
        let key = session.to_owned();
        let share = self
            .store
            .events()
            .database()
            .read(move |sql| session_share::get(sql, &key))
            .await
            .map_err(failed)?;
        let Some(share) = share else {
            return Ok(());
        };
        self.client
            .delete(format!("{}/api/share/{}", self.base, share.id))
            .json(&json!({"secret":share.secret.expose()}))
            .send()
            .await
            .map_err(failed)?
            .error_for_status()
            .map_err(failed)?;
        let key = session.to_owned();
        self.store
            .events()
            .database()
            .transaction(TxOptions::default(), move |sql| {
                session_share::remove(sql, &key)
            })
            .await
            .map_err(failed)?;
        Ok(())
    }
}

fn failed(error: impl std::fmt::Display) -> PortError {
    PortError::Failed(error.to_string())
}
