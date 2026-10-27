//! Purpose: execute generated client requests using real browser fetch.
//! Scope: browser HTTP transport, preserving declared wire errors.
//! Uses:
//! `-- none
//! External:
//! `-- none
//! Used by:
//! `-- none
//! Invariants: every response is decoded by its generated operation, never treated as success by status alone.
//! Contract tests: ui-web/tests/network.rs

use opencode_client::operation::{AsyncTransport, Response};
use opencode_client::transport::RequestDescriptor;
use opencode_client::{ClientError, ClientErrorReason};
use serde_json::{Value, json};
use url::Url;

#[derive(Clone, Debug)]
pub struct BrowserTransport {
    pub base: Url,
}

impl AsyncTransport for BrowserTransport {
    async fn execute(&self, request: RequestDescriptor) -> Result<Response, ClientError> {
        let url = request
            .url(&self.base)
            .map_err(|error| ClientError::with_cause(ClientErrorReason::Transport, error))?;
        let mut builder = gloo_net::http::RequestBuilder::new(url.as_str()).method(
            request
                .method
                .parse()
                .map_err(|_| ClientError::new(ClientErrorReason::Transport))?,
        );
        for (key, value) in &request.headers {
            builder = builder.header(key, value);
        }
        let request = match request.body {
            Some(body) => builder.json(&body).map_err(network_error)?,
            None => builder.build().map_err(network_error)?,
        };
        let response = request.send().await.map_err(network_error)?;
        Ok(Response {
            status: response.status(),
            content_type: response.headers().get("content-type"),
            body: response.binary().await.map_err(network_error)?,
        })
    }
}

fn network_error(error: gloo_net::Error) -> ClientError {
    ClientError::with_cause(
        ClientErrorReason::Transport,
        std::io::Error::other(error.to_string()),
    )
}

// Inputs and outputs are checked against the generated registry before crossing the UI boundary.
macro_rules! operation {
    ($name:ident, $group:ident, $operation:ident) => {
        pub async fn $name(transport: &BrowserTransport, value: Value) -> Result<Value, String> {
            use opencode_client::current::$group::$operation;
            let input = serde_json::from_value(value).map_err(|error| error.to_string())?;
            let result = $operation::execute_async(transport, &input)
                .await
                .map_err(|error| format!("{error:?}"))?;
            serde_json::to_value(result).map_err(|error| error.to_string())
        }
    };
}
operation!(sessions, sessions, list);
operation!(create, sessions, create);
operation!(messages, messages, list);
operation!(prompt, sessions, prompt);
operation!(interrupt, sessions, interrupt);
operation!(models, models, list);
operation!(agents, agents, list);
operation!(permissions, permissions, list_requests);
operation!(reply_permission, permissions, permission_reply);

pub fn session_query(session: &str, cursor: Option<&str>) -> Value {
    let mut query = json!({"sessionID": session, "limit": 50, "order": "desc"});
    if let Some(cursor) = cursor {
        query["cursor"] = cursor.into();
    }
    query
}
