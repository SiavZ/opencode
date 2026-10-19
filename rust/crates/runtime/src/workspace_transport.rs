//! Purpose: real remote workspace HTTP history and response-owned SSE transport.
//! Scope: process HTTP client; each returned stream owns its response and has no detached task.
//! Uses:
//! rust/crates/
//! |-- locations/src/error.rs
//! `-- locations/src/workspace/sync.rs
//! External:
//! |-- async_trait::async_trait
//! |-- futures::{StreamExt, stream::BoxStream}
//! `-- reqwest::{Client, header}
//! Used by:
//! rust/crates/
//! |-- runtime/src/lib.rs
//! |-- runtime/src/process.rs
//! `-- runtime/tests/workspace_transport.rs
//! Invariants: endpoint URLs are already routed by the supervisor; dropping the stream drops HTTP.
//! Non-2xx messages and history error bodies retain the TypeScript SyncHttp contract.
//! Contract tests: runtime/tests/workspace_transport.rs

use std::collections::BTreeMap;

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use opencode_locations::error::{LocationError, Result};
use opencode_locations::workspace::sync::{HistoryEvent, SyncTransport};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

#[derive(Clone, Debug)]
pub struct WorkspaceTransport {
    client: reqwest::Client,
}

impl WorkspaceTransport {
    pub fn new(client: reqwest::Client) -> Self {
        Self { client }
    }
}

#[async_trait]
impl SyncTransport for WorkspaceTransport {
    async fn connect(
        &self,
        url: &str,
        headers: &[(String, String)],
    ) -> Result<BoxStream<'static, Result<Vec<u8>>>> {
        let response = self
            .client
            .get(url)
            .headers(request_headers(headers)?)
            .header(reqwest::header::ACCEPT, "text/event-stream")
            .send()
            .await
            .map_err(|error| LocationError::Io(error.to_string()))?;
        let status = response.status();
        if !status.is_success() {
            return Err(LocationError::SyncHttp {
                message: format!("Workspace sync HTTP failure: {}", status.as_u16()),
                status: status.as_u16(),
                body: None,
            });
        }
        Ok(response
            .bytes_stream()
            .map(|chunk| {
                chunk
                    .map(|bytes| bytes.to_vec())
                    .map_err(|error| LocationError::Io(error.to_string()))
            })
            .boxed())
    }

    async fn history(
        &self,
        url: &str,
        headers: &[(String, String)],
        state: &BTreeMap<String, i64>,
    ) -> Result<Vec<HistoryEvent>> {
        let response = self
            .client
            .post(url)
            .headers(request_headers(headers)?)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .json(state)
            .send()
            .await
            .map_err(|error| LocationError::Io(error.to_string()))?;
        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .map_err(|error| LocationError::Io(error.to_string()))?;
            return Err(LocationError::SyncHttp {
                message: format!("Workspace history HTTP failure: {} {body}", status.as_u16()),
                status: status.as_u16(),
                body: Some(body),
            });
        }
        response.json().await.map_err(|error| {
            if error.is_decode() {
                return LocationError::Validation(error.to_string());
            }
            LocationError::Io(error.to_string())
        })
    }
}

fn request_headers(headers: &[(String, String)]) -> Result<HeaderMap> {
    headers
        .iter()
        .try_fold(HeaderMap::new(), |mut out, (name, value)| {
            out.append(
                HeaderName::from_bytes(name.as_bytes())
                    .map_err(|error| LocationError::Validation(error.to_string()))?,
                HeaderValue::from_str(value)
                    .map_err(|error| LocationError::Validation(error.to_string()))?,
            );
            Ok(out)
        })
}
