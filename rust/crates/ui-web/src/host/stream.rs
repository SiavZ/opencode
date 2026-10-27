//! Purpose: own and dispose a real browser EventSource feeding the canonical reducer.
//! Scope: one viewed session subscription with native reconnect and Last-Event-ID.
//! Uses:
//! `-- none
//! External:
//! `-- none
//! Used by:
//! `-- none
//! Invariants: callbacks apply only to their captured server; dropping closes the stream before callbacks are released.
//! Contract tests: browser acceptance and view-model/tests/reducer.rs
use crate::state::Browser;
use leptos::prelude::*;
use opencode_client::current::sessions::events;
use wasm_bindgen::{JsCast, closure::Closure};

pub struct Subscription {
    source: web_sys::EventSource,
    message: Closure<dyn FnMut(web_sys::MessageEvent)>,
    error: Closure<dyn FnMut(web_sys::Event)>,
}

impl Subscription {
    pub fn start(browser: Browser, id: &str) -> Result<Self, String> {
        let transport = browser.state.with(|state| state.transport())?;
        let server = transport.base.to_string();
        let request = events::request(
            &serde_json::from_value(serde_json::json!({"sessionID":id}))
                .map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
        let url = request
            .url(&transport.base)
            .map_err(|error| error.to_string())?;
        let source = web_sys::EventSource::new(url.as_str())
            .map_err(|error| format!("Cannot open session events: {error:?}"))?;
        let message = Closure::new(move |event: web_sys::MessageEvent| {
            let Some(data) = event.data().as_string() else {
                return;
            };
            let parsed = events::decode_frame(&data)
                .map_err(|error| error.to_string())
                .and_then(|event| serde_json::to_value(event).map_err(|error| error.to_string()))
                .and_then(|event| {
                    serde_json::from_value::<opencode_view_model::SessionEvent>(event)
                        .map_err(|error| error.to_string())
                });
            browser.state.update(|state| {
                if state.server.trim_end_matches('/') != server.trim_end_matches('/') {
                    return;
                }
                match parsed {
                    Ok(event) => {
                        state.cache.apply_event(&event);
                    }
                    Err(error) => state.error = Some(format!("Session event decode: {error}")),
                }
            });
        });
        let error = Closure::new(move |_: web_sys::Event| {
            // EventSource itself retries with its last event ID. Do not clear the transcript.
            browser.error("Session event connection interrupted; reconnecting.".into());
        });
        source.set_onmessage(Some(message.as_ref().unchecked_ref()));
        source.set_onerror(Some(error.as_ref().unchecked_ref()));
        Ok(Self {
            source,
            message,
            error,
        })
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        self.source.close();
        self.source.set_onmessage(None);
        self.source.set_onerror(None);
        let _ = (&self.message, &self.error);
    }
}
