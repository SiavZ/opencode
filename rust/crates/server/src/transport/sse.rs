//! Purpose: SSE response bodies: Effect-compatible frame encoding over a merged stream of
//! payload frames and heartbeats, with the baseline stream headers.
//! Scope: per-connection stream.
//! Uses:
//! rust/crates/
//! `-- protocol/src/sse.rs
//! External:
//! |-- axum::body::Body
//! |-- axum::body::Bytes
//! |-- axum::http::HeaderName
//! |-- axum::http::HeaderValue
//! |-- axum::http::StatusCode
//! |-- axum::http::header
//! |-- axum::response::Response
//! |-- futures_util::future::ready
//! |-- futures_util::stream::BoxStream
//! |-- futures_util::stream::StreamExt
//! |-- futures_util::stream::self
//! |-- header::CONTENT_TYPE
//! |-- stream::empty
//! |-- stream::iter
//! |-- stream::once
//! |-- stream::select
//! |-- stream::unfold
//! |-- tokio::test
//! |-- tokio::time::Instant::now
//! `-- tokio::time::sleep_until
//! Used by:
//! rust/crates/
//! |-- server/src/handlers/streams.rs
//! `-- server/src/transport/mod.rs
//! Invariants: the body ends when the source ends (`haltStrategy: "left"`), heartbeats never
//! keep a finished stream alive. `Stream.tick` semantics: the first tick fires immediately; legacy
//! streams drop it. Headers: `content-type: text/event-stream` then the protocol `HEADERS`.
//! Contract tests: server/tests/sse_events.rs

use std::convert::Infallible;
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::http::{HeaderName, HeaderValue, StatusCode, header};
use axum::response::Response;
use futures_util::stream::{self, BoxStream, StreamExt};
use opencode_protocol::sse::{CONTENT_TYPE, Frame, HEADERS};

/// One encoded chunk of an SSE body.
pub type Chunk = String;

/// `Stream.tick(interval)` mapped to `item`, dropping `drop` initial ticks.
pub fn ticks(
    interval: Duration,
    drop: usize,
    item: impl Fn() -> Chunk + Send + 'static,
) -> BoxStream<'static, Chunk> {
    let start = tokio::time::Instant::now();
    stream::unfold(0u64, move |n| async move {
        // Tick `n` fires at `n * interval` (first tick immediate).
        tokio::time::sleep_until(start + interval * n as u32).await;
        Some(((), n + 1))
    })
    .skip(drop)
    .map(move |_| item())
    .boxed()
}

/// Merge `source` with `heartbeat`, ending when `source` ends.
pub fn halt_left(
    source: BoxStream<'static, Chunk>,
    heartbeat: BoxStream<'static, Chunk>,
) -> BoxStream<'static, Chunk> {
    // Tag items so the merged stream can stop at the source's end marker.
    let left = source.map(Some).chain(stream::once(async { None }));
    let right = heartbeat.map(Some);
    stream::select(left, right)
        .take_while(|item| futures_util::future::ready(item.is_some()))
        .filter_map(futures_util::future::ready)
        .boxed()
}

/// Fallible frame stream: an `Err` aborts the body, matching an Effect stream failure inside
/// `HttpServerResponse.stream` (the connection is torn down without a terminal chunk).
pub type FallibleChunks = BoxStream<'static, Result<Chunk, std::io::Error>>;

/// `halt_left` for a fallible source.
pub fn halt_left_fallible(
    source: FallibleChunks,
    heartbeat: BoxStream<'static, Chunk>,
) -> FallibleChunks {
    let left = source.map(Some).chain(stream::once(async { None }));
    let right = heartbeat.map(|chunk| Some(Ok(chunk)));
    stream::select(left, right)
        .take_while(|item| futures_util::future::ready(item.is_some()))
        .filter_map(futures_util::future::ready)
        .boxed()
}

/// `response` for a fallible body.
pub fn fallible_response(body: FallibleChunks) -> Response {
    let mut response = response(stream::empty().boxed());
    *response.body_mut() = Body::from_stream(body.map(|item| item.map(Bytes::from)));
    response
}

/// JSON data frame (`event: message`, no id).
pub fn data(json: &str) -> Chunk {
    Frame::message(json).encode()
}

/// 200 `text/event-stream` response with the baseline headers.
pub fn response(body: BoxStream<'static, Chunk>) -> Response {
    let body = Body::from_stream(body.map(|chunk| Ok::<_, Infallible>(Bytes::from(chunk))));
    let mut response = Response::new(body);
    *response.status_mut() = StatusCode::OK;
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(CONTENT_TYPE));
    for (name, value) in HEADERS {
        headers.insert(
            HeaderName::from_static(lower(name)),
            HeaderValue::from_static(value),
        );
    }
    response
}

fn lower(name: &'static str) -> &'static str {
    match name {
        "Cache-Control" => "cache-control",
        "X-Accel-Buffering" => "x-accel-buffering",
        "X-Content-Type-Options" => "x-content-type-options",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn halts_when_source_ends_even_with_heartbeat() {
        let source = stream::iter(vec!["a".to_string(), "b".to_string()]).boxed();
        let hb = ticks(Duration::from_secs(15), 0, || ": heartbeat\n\n".to_string());
        let out: Vec<_> = halt_left(source, hb).collect().await;
        assert!(out.contains(&"a".to_string()) && out.contains(&"b".to_string()));
    }

    #[tokio::test(start_paused = true)]
    async fn legacy_ticks_drop_first() {
        let start = tokio::time::Instant::now();
        let mut hb = ticks(Duration::from_secs(10), 1, || "hb".to_string());
        hb.next().await;
        assert_eq!(start.elapsed(), Duration::from_secs(10));
    }
}
