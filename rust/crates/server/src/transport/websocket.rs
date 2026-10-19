//! Purpose: PTY WebSocket session (`packages/server/src/handlers/pty.ts` connect and the legacy
//! `httpapi/handlers/pty.ts` connect): replay chunks, the cursor metadata frame, live output,
//! inbound input, close codes and tracked shutdown.
//! Scope: per-connection task.
//! Uses:
//! rust/crates/
//! `-- platform-api/src/pty.rs
//! External:
//! |-- axum::extract::ws::CloseFrame
//! |-- axum::extract::ws::Message
//! |-- axum::extract::ws::WebSocket
//! |-- futures_util::SinkExt
//! |-- futures_util::StreamExt
//! |-- tokio::select
//! |-- tokio::time::timeout
//! `-- tokio_util::sync::CancellationToken
//! Used by:
//! rust/crates/
//! |-- server/src/handlers/streams/pty.rs
//! `-- server/src/transport/mod.rs
//! Invariants: one writer keeps order replay chunks (text, 64Ki UTF-16 units each), then the
//! binary meta frame `0x00 + {"cursor":N}`, then live output, then close 1000 on PTY end. Inbound
//! text and valid UTF-8 binary go to the PTY; invalid UTF-8 is dropped. The attachment is always
//! detached when either side ends. Server shutdown closes with 1001 `server closing`.
//! Contract tests: server/tests/ws_pty.rs

use axum::extract::ws::{CloseFrame, Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use opencode_platform_api::pty::{PtyAttachment, PtyOutput};
use tokio_util::sync::CancellationToken;

/// `PtyProtocol.REPLAY_CHUNK` in UTF-16 code units.
pub const REPLAY_CHUNK: usize = 64 * 1024;

/// `PtyProtocol.metaFrame(cursor)`.
pub fn meta_frame(cursor: u64) -> Vec<u8> {
    let mut out = vec![0u8];
    out.extend_from_slice(format!("{{\"cursor\":{cursor}}}").as_bytes());
    out
}

/// `PtyProtocol.chunks(data)`: slices of at most `REPLAY_CHUNK` UTF-16 code units. A surrogate
/// pair straddling a boundary is kept whole (JS would split it into lone surrogates, which do
/// not survive UTF-8 transport either way).
pub fn chunks(data: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut units = 0;
    for ch in data.chars() {
        let width = ch.len_utf16();
        if units + width > REPLAY_CHUNK && !current.is_empty() {
            out.push(std::mem::take(&mut current));
            units = 0;
        }
        current.push(ch);
        units += width;
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// Close a just-upgraded socket with `code`/`reason` (`closeAccepted`).
pub async fn close_with(mut socket: WebSocket, code: u16, reason: &str) {
    let frame = CloseFrame {
        code,
        reason: reason.into(),
    };
    let _ = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        socket.send(Message::Close(Some(frame))),
    )
    .await;
}

/// Drive an attached PTY over `socket` until either side ends or `closing` fires.
pub async fn run(
    socket: WebSocket,
    attachment: Box<dyn PtyAttachment>,
    closing: CancellationToken,
) {
    let (mut sink, mut source) = socket.split();
    let attachment: std::sync::Arc<dyn PtyAttachment> = std::sync::Arc::from(attachment);
    let replay = chunks(attachment.replay());
    let cursor = attachment.cursor();
    let writer = {
        let attachment = attachment.clone();
        let closing = closing.clone();
        async move {
            for chunk in replay {
                if sink.send(Message::Text(chunk.into())).await.is_err() {
                    return;
                }
            }
            if sink
                .send(Message::Binary(meta_frame(cursor).into()))
                .await
                .is_err()
            {
                return;
            }
            attachment.activate();
            loop {
                let next = tokio::select! {
                    output = attachment.next() => output,
                    _ = closing.cancelled() => {
                        let _ = sink.send(Message::Close(Some(CloseFrame { code: 1001, reason: "server closing".into() }))).await;
                        return;
                    }
                };
                match next {
                    Some(PtyOutput::Data(data)) => {
                        if sink.send(Message::Text(data.into())).await.is_err() {
                            return;
                        }
                    }
                    Some(PtyOutput::End { .. }) | None => {
                        let _ = sink
                            .send(Message::Close(Some(CloseFrame {
                                code: 1000,
                                reason: "".into(),
                            })))
                            .await;
                        return;
                    }
                }
            }
        }
    };
    let reader = {
        let attachment = attachment.clone();
        async move {
            while let Some(Ok(message)) = source.next().await {
                let input = match message {
                    Message::Text(text) => Some(text.to_string()),
                    Message::Binary(bytes) => String::from_utf8(bytes.to_vec()).ok(),
                    Message::Close(_) => break,
                    Message::Ping(_) | Message::Pong(_) => None,
                };
                if let Some(input) = input {
                    let _ = attachment.write(&input);
                }
            }
        }
    };
    // Whichever side finishes first ends the connection (`Effect.race`).
    tokio::select! {
        _ = writer => {}
        _ = reader => {}
    }
    attachment.detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meta_frame_bytes() {
        assert_eq!(meta_frame(42), b"\0{\"cursor\":42}".to_vec());
    }

    #[test]
    fn chunks_split_on_utf16_units() {
        let data = "a".repeat(REPLAY_CHUNK + 1);
        let parts = chunks(&data);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[1], "a");
        assert!(chunks("").is_empty());
    }
}
