//! Purpose: cancel hosted request work when its accepted client TCP connection closes.
//! Scope: Axum listener IO lifecycle and per-request cancellation extension.
//! Uses:
//! rust/bins/
//! `-- hosted/src/transport/tests.rs
//! External:
//! |-- axum::{Router, extract::ConnectInfo, serve::Listener}
//! |-- tokio::io::{AsyncRead, AsyncWrite}
//! `-- tokio_util::sync::CancellationToken
//! Used by:
//! rust/bins/
//! `-- hosted/src/main.rs
//! Invariants: only socket EOF/error/drop cancels a connection; HTTP body EOF does not.
//! Contract tests: transport/tests.rs, console/zen/acceptance.rs

use std::io::{self, IoSlice};
use std::net::SocketAddr;
use std::pin::Pin;
use std::task::{Context, Poll};

use axum::Router;
use axum::extract::{ConnectInfo, Request, connect_info::Connected};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::serve::{IncomingStream, Listener};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;

#[cfg(test)]
mod h2_tests;
#[cfg(test)]
mod tests;

/// Uses Axum's native connection serving, including keepalive, streaming and enabled upgrades.
/// Each request receives the connection's plain `CancellationToken` extension.
/// HTTP/1 read half-close follows Axum's default: socket EOF closes a pending exchange.
/// Finishing a framed request body does not signal socket EOF or cancel its request.
pub async fn serve(listener: TcpListener, router: Router) -> io::Result<()> {
    axum::serve(
        Connections(listener),
        router
            .layer(middleware::from_fn(cancel_on_disconnect))
            .into_make_service_with_connect_info::<Connection>(),
    )
    .await
}

struct Connections(TcpListener);

impl Listener for Connections {
    type Io = ConnectionIo;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        let (socket, address) = Listener::accept(&mut self.0).await;
        (
            ConnectionIo {
                socket,
                cancellation: CancellationToken::new(),
            },
            address,
        )
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        self.0.local_addr()
    }
}

struct ConnectionIo {
    socket: TcpStream,
    cancellation: CancellationToken,
}

impl AsyncRead for ConnectionIo {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let before = buffer.filled().len();
        let remaining = buffer.remaining();
        let result = Pin::new(&mut this.socket).poll_read(cx, buffer);
        if matches!(&result, Poll::Ready(Err(_)))
            || (remaining > 0
                && matches!(&result, Poll::Ready(Ok(())))
                && buffer.filled().len() == before)
        {
            this.cancellation.cancel();
        }
        result
    }
}

impl AsyncWrite for ConnectionIo {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        let result = Pin::new(&mut this.socket).poll_write(cx, bytes);
        if matches!(&result, Poll::Ready(Err(_))) {
            this.cancellation.cancel();
        }
        result
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffers: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        let result = Pin::new(&mut this.socket).poll_write_vectored(cx, buffers);
        if matches!(&result, Poll::Ready(Err(_))) {
            this.cancellation.cancel();
        }
        result
    }

    fn is_write_vectored(&self) -> bool {
        self.socket.is_write_vectored()
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let result = Pin::new(&mut this.socket).poll_flush(cx);
        if matches!(&result, Poll::Ready(Err(_))) {
            this.cancellation.cancel();
        }
        result
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let result = Pin::new(&mut this.socket).poll_shutdown(cx);
        if matches!(&result, Poll::Ready(_)) {
            this.cancellation.cancel();
        }
        result
    }
}

impl Drop for ConnectionIo {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

#[derive(Clone)]
struct Connection(CancellationToken);

impl Connected<IncomingStream<'_, Connections>> for Connection {
    fn connect_info(stream: IncomingStream<'_, Connections>) -> Self {
        Self(stream.io().cancellation.clone())
    }
}

async fn cancel_on_disconnect(
    ConnectInfo(Connection(cancellation)): ConnectInfo<Connection>,
    mut request: Request,
    next: Next,
) -> Response {
    request.extensions_mut().insert(cancellation.clone());
    tokio::select! {
        biased;
        _ = cancellation.cancelled() => StatusCode::from_u16(499)
            .expect("valid client closed request status").into_response(),
        response = next.run(request) => response,
    }
}
