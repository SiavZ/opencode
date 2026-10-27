//! Purpose: own browser session state, generated requests and target-scoped async completion.
//! Scope: server-root state, preserved across session navigation.
//! Uses:
//! `-- none
//! External:
//! `-- none
//! Used by:
//! `-- none
//! Invariants: late responses never mutate another server or another target's draft.
//! Contract tests: ui-web/tests; view-model/tests/browser.rs

use crate::host::network::BrowserTransport;
use crate::host::{network, storage};
use leptos::prelude::*;
use opencode_view_model::browser::{Draft, DraftBook};
use opencode_view_model::identity::{ServerKey, session_href};
use opencode_view_model::session_cache::{Page, PageMode, SessionCache};
use serde_json::{Value, json};

pub struct BrowserState {
    pub server: String,
    pub directory: String,
    pub active: String,
    pub sessions: Vec<Value>,
    pub models: Vec<Value>,
    pub agents: Vec<Value>,
    pub model: String,
    pub agent: String,
    pub cache: SessionCache,
    pub drafts: DraftBook,
    pub error: Option<String>,
    pub loading: bool,
}

impl BrowserState {
    pub fn new(server: String) -> Self {
        Self {
            server,
            directory: storage::get("oc.directory").unwrap_or_default(),
            active: String::new(),
            sessions: vec![],
            models: vec![],
            agents: vec![],
            model: String::new(),
            agent: String::new(),
            cache: SessionCache::default(),
            drafts: DraftBook::default(),
            error: None,
            loading: false,
        }
    }
    pub fn target(&self) -> String {
        format!("{}\0{}\0{}", self.server, self.directory, self.active)
    }
    pub fn transport(&self) -> Result<BrowserTransport, String> {
        url::Url::parse(&self.server)
            .map(|base| BrowserTransport { base })
            .map_err(|error| error.to_string())
    }
}

#[derive(Clone, Copy)]
pub struct Browser {
    pub state: RwSignal<BrowserState>,
    pub stream: StoredValue<Option<crate::host::stream::Subscription>, LocalStorage>,
}

impl Browser {
    pub fn error(self, error: String) {
        self.state.update(|state| state.error = Some(error));
    }
    pub fn draft(self) -> Draft {
        self.state.with(|state| state.drafts.get(&state.target()))
    }
    pub fn edit(self, draft: Draft) {
        self.state.update(|state| {
            let target = state.target();
            persist(&target, &draft);
            state.drafts.edit(&target, draft);
        });
    }
    pub fn restore(self) {
        self.state.update(|state| {
            let target = state.target();
            if let Some(value) = storage::get(&format!("oc.draft:{target}"))
                && let Ok(draft) = serde_json::from_str(&value)
            {
                state.drafts.edit(&target, draft);
            }
        });
    }
    pub fn refresh(self) {
        let capture = self.state.with(|state| {
            (
                state.server.clone(),
                state.directory.clone(),
                state.transport(),
            )
        });
        wasm_bindgen_futures::spawn_local(async move {
            let result = async {
                let transport = capture.2?;
                let sessions = network::sessions(
                    &transport,
                    json!({"directory":capture.1,"limit":100,"order":"desc"}),
                )
                .await?;
                let models =
                    network::models(&transport, json!({"location":{"directory":capture.1}}))
                        .await?;
                let agents =
                    network::agents(&transport, json!({"location":{"directory":capture.1}}))
                        .await?;
                Ok::<_, String>((sessions, models, agents))
            }
            .await;
            self.state.update(|state| {
                if state.server != capture.0 {
                    return;
                }
                match result {
                    Ok((sessions, models, agents)) => {
                        state.sessions = items(&sessions);
                        state.models = items(&models);
                        state.agents = items(&agents);
                        if state.model.is_empty()
                            && let Some(model) = state.models.first()
                        {
                            state.model =
                                format!("{}/{}", text(model, "providerID"), text(model, "id"));
                        }
                        if state.agent.is_empty()
                            && let Some(agent) = state.agents.first()
                        {
                            state.agent = text(agent, "name");
                        }
                    }
                    Err(error) => state.error = Some(error),
                }
            });
        });
    }
    pub fn select(self, id: String, push: bool) {
        self.stream.update_value(|stream| *stream = None);
        self.state.update(|state| {
            state.cache.unpin(&state.active);
            state.active = id.clone();
            state.error = None;
            if let Some(session) = state.sessions.iter().find(|session| session["id"] == id)
                && let Some(directory) = session
                    .pointer("/location/directory")
                    .and_then(Value::as_str)
            {
                state.directory = directory.into();
            }
            state.cache.pin(&id);
            if push
                && let Some(window) = web_sys::window()
                && let Ok(history) = window.history()
            {
                let href = if id.is_empty() {
                    "/new-session".to_owned()
                } else {
                    session_href(&ServerKey(state.server.clone()), &id)
                };
                let _ = history.push_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&href));
            }
        });
        self.restore();
        if id.is_empty() {
            return;
        }
        match crate::host::stream::Subscription::start(self, &id) {
            Ok(subscription) => self
                .stream
                .update_value(|stream| *stream = Some(subscription)),
            Err(error) => self.error(error),
        }
        self.load(None);
    }
    pub fn load(self, cursor: Option<String>) {
        let (server, id, transport) = self.state.with(|state| {
            (
                state.server.clone(),
                state.active.clone(),
                state.transport(),
            )
        });
        self.state.update(|state| {
            state.cache.begin_load(&id);
            state.loading = true;
        });
        wasm_bindgen_futures::spawn_local(async move {
            let result = async {
                network::messages(&transport?, network::session_query(&id, cursor.as_deref())).await
            }
            .await;
            self.state.update(|state| {
                if state.server != server {
                    return;
                }
                state.loading = false;
                match result {
                    Ok(value) => {
                        let source = serde_json::from_value::<
                            Vec<opencode_view_model::SessionMessage>,
                        >(value["data"].clone());
                        match source {
                            Ok(mut source) => {
                                source.sort_by_key(|message| {
                                    (message.created(), message.id().to_owned())
                                });
                                let next = value
                                    .pointer("/cursor/next")
                                    .and_then(Value::as_str)
                                    .map(str::to_owned);
                                state.cache.apply_page(
                                    &id,
                                    if cursor.is_some() {
                                        PageMode::Older
                                    } else {
                                        PageMode::Latest
                                    },
                                    Page {
                                        source,
                                        complete: next.is_none(),
                                        cursor: next,
                                    },
                                );
                            }
                            Err(error) => {
                                state.cache.cancel_load(&id);
                                state.error = Some(error.to_string());
                            }
                        }
                    }
                    Err(error) => {
                        state.cache.cancel_load(&id);
                        state.error = Some(error);
                    }
                }
            });
        });
    }
    pub fn submit(self) {
        let capture = self
            .state
            .try_update(|state| {
                let target = state.target();
                if state.agent.is_empty() || state.model.is_empty() {
                    state.error = Some("Select a model and agent before sending.".into());
                    return None;
                }
                let submission = state.drafts.begin(&target)?;
                persist(&target, &Draft::default());
                Some((
                    state.server.clone(),
                    state.active.clone(),
                    state.directory.clone(),
                    state.agent.clone(),
                    state.model.clone(),
                    state.transport(),
                    submission,
                ))
            })
            .flatten();
        let Some((server, id, directory, agent, model, transport, submission)) = capture else {
            return;
        };
        wasm_bindgen_futures::spawn_local(async move {
            let result = async {
                let transport = transport?;
                let id = if id.is_empty() {
                    let (provider, model) = model.split_once('/').ok_or("Invalid selected model")?;
                    let session = network::create(&transport,json!({"agent":agent,"model":{"providerID":provider,"id":model},"location":{"directory":directory}})).await?;
                    session["id"].as_str().ok_or("Session creation returned no identity")?.to_owned()
                } else { id };
                let files = submission.draft.attachments.iter().map(|file| json!({"uri":file.uri,"name":file.name})).collect::<Vec<_>>();
                network::prompt(&transport, json!({"sessionID":id,"prompt":{"text":submission.draft.text,"files":files}})).await?;
                Ok::<_,String>(id)
            }.await;
            let success = result.is_ok();
            self.state.update(|state| {
                if state.server != server {
                    return;
                }
                if state.drafts.finish(&submission, success) {
                    persist(&submission.target, &submission.draft);
                }
                if let Err(error) = &result {
                    state.error = Some(error.clone());
                }
            });
            if let Ok(id) = result
                && self
                    .state
                    .with(|state| state.server == server && state.target() == submission.target)
            {
                self.select(id, true);
                self.refresh();
            }
        });
    }
    pub fn interrupt(self) {
        let (id, transport) = self
            .state
            .with(|state| (state.active.clone(), state.transport()));
        wasm_bindgen_futures::spawn_local(async move {
            let result =
                async { network::interrupt(&transport?, json!({"sessionID":id})).await }.await;
            if let Err(error) = result {
                self.error(error);
            }
        });
    }
}

pub fn items(value: &Value) -> Vec<Value> {
    value
        .as_array()
        .or_else(|| value.get("data").and_then(Value::as_array))
        .cloned()
        .unwrap_or_default()
}
pub fn text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}
fn persist(target: &str, draft: &Draft) {
    if let Ok(value) = serde_json::to_string(draft) {
        storage::set(&format!("oc.draft:{target}"), &value);
    }
}
