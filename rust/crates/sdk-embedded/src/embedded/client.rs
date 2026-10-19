//! Purpose: dispatch client descriptors through the real embedded router without a socket.
//! Scope: in-process HTTP request, JSON response and declared/client failure separation.
//! Uses:
//! rust/crates/
//! |-- client/src/errors.rs
//! |-- client/src/operation.rs
//! |-- client/src/transport/mod.rs
//! `-- sdk-embedded/src/embedded.rs
//! External:
//! |-- axum::Router
//! |-- axum::body::Body
//! |-- axum::http::Request
//! |-- axum::http::Response
//! |-- axum::http::header
//! |-- header::CONTENT_TYPE
//! |-- header::HeaderName
//! |-- header::HeaderValue::from_static
//! |-- http_body_util::BodyExt
//! |-- serde_json::Value
//! |-- serde_json::to_vec
//! |-- str::to_owned
//! |-- tokio::select
//! |-- tower::ServiceExt
//! `-- url::Url
//! Used by:
//! rust/crates/
//! `-- sdk-embedded/src/embedded.rs
//! Invariants: no network transport; host disposal cancels requests and rejects later calls.
//! Contract tests: sdk-embedded/tests/embedded.rs

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, Response, header};
use http_body_util::BodyExt;
use opencode_client::errors::{ApiError, ClientError, ClientErrorReason};
use opencode_client::transport::RequestDescriptor;
use serde_json::Value;
use tower::ServiceExt;
use url::Url;

use super::{EMBEDDED_BASE_URL, Lifetime};

#[derive(Clone)]
pub struct EmbeddedClient {
    router: Router,
    lifetime: Arc<Lifetime>,
}

impl EmbeddedClient {
    pub(super) fn new(router: Router, lifetime: Arc<Lifetime>) -> Self {
        Self { router, lifetime }
    }

    /// Raw HTTP boundary, including streaming bodies. Dropping the returned body cancels it.
    pub async fn execute(
        &self,
        descriptor: &RequestDescriptor,
    ) -> Result<Response<Body>, ClientError> {
        if self.lifetime.cancel.is_cancelled() {
            return Err(closed());
        }
        let url = descriptor
            .url(&Url::parse(EMBEDDED_BASE_URL).expect("constant embedded URL"))
            .map_err(|error| ClientError::with_cause(ClientErrorReason::Transport, error))?;
        let mut request =
            Request::new(match &descriptor.body {
                Some(body) => Body::from(serde_json::to_vec(body).map_err(|error| {
                    ClientError::with_cause(ClientErrorReason::Transport, error)
                })?),
                None => Body::empty(),
            });
        *request.method_mut() = descriptor
            .method
            .parse()
            .map_err(|error| ClientError::with_cause(ClientErrorReason::Transport, error))?;
        *request.uri_mut() = url
            .as_str()
            .parse()
            .map_err(|error| ClientError::with_cause(ClientErrorReason::Transport, error))?;
        for (name, value) in &descriptor.headers {
            request.headers_mut().insert(
                name.parse::<header::HeaderName>().map_err(|error| {
                    ClientError::with_cause(ClientErrorReason::Transport, error)
                })?,
                value.parse().map_err(|error| {
                    ClientError::with_cause(ClientErrorReason::Transport, error)
                })?,
            );
        }
        if descriptor.body.is_some() && !request.headers().contains_key(header::CONTENT_TYPE) {
            request.headers_mut().insert(
                header::CONTENT_TYPE,
                header::HeaderValue::from_static("application/json"),
            );
        }
        tokio::select! {
            biased;
            _ = self.lifetime.cancel.cancelled() => Err(closed()),
            result = self.router.clone().oneshot(request) => result
                .map(|response| response.map(|body| {
                    Body::from_stream(futures_util::StreamExt::take_until(
                        body.into_data_stream(),
                        self.lifetime.cancel.clone().cancelled_owned(),
                    ))
                }))
                .map_err(|error| ClientError::with_cause(ClientErrorReason::Transport, error)),
        }
    }

    /// Collect an HTTP response for a generated operation's typed `decode` function.
    pub async fn send(
        &self,
        descriptor: &RequestDescriptor,
    ) -> Result<opencode_client::operation::Response, ClientError> {
        self.collect(self.execute(descriptor).await?).await
    }

    /// Generated-client JSON semantics, retaining declared wire errors separately.
    pub async fn request(&self, descriptor: RequestDescriptor) -> Result<Value, ApiError<Value>> {
        let response = self.execute(&descriptor).await?;
        let status = response.status().as_u16();
        if status != descriptor.success_status && !descriptor.declared_statuses.contains(&status) {
            return Err(ClientError::unexpected_status(status).into());
        }
        if status == descriptor.success_status && descriptor.empty {
            return Ok(Value::Null);
        }
        opencode_client::operation::decode(
            self.collect(response).await?,
            descriptor.success_status,
            descriptor.declared_statuses,
            descriptor.empty,
            false,
        )
    }

    async fn collect(
        &self,
        response: Response<Body>,
    ) -> Result<opencode_client::operation::Response, ClientError> {
        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let body = tokio::select! {
            biased;
            _ = self.lifetime.cancel.cancelled() => return Err(closed()),
            body = response.into_body().collect() => body
                .map_err(|error| ClientError::with_cause(ClientErrorReason::Transport, error))?,
        };
        Ok(opencode_client::operation::Response {
            status,
            content_type,
            body: body.to_bytes().to_vec(),
        })
    }

    /// Join disposal after explicit host shutdown or Drop. Requires a live tokio runtime.
    pub async fn wait_closed(&self) {
        self.lifetime.cancel.cancelled().await;
        self.lifetime.cleanup.wait().await;
    }
}

impl opencode_client::operation::AsyncTransport for EmbeddedClient {
    async fn execute(
        &self,
        descriptor: RequestDescriptor,
    ) -> Result<opencode_client::operation::Response, ClientError> {
        self.send(&descriptor).await
    }
}

fn closed() -> ClientError {
    ClientError::with_cause(
        ClientErrorReason::Transport,
        std::io::Error::other("embedded host is closed"),
    )
}
