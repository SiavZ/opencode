//! Purpose: server/directory/session/draft ownership keys and captured-generation ownership.
//! Ports `ServerConnection.key` (`packages/app/src/context/server.tsx:180-235`), route segment
//! encoding (`packages/app/src/utils/session-route.ts`, `packages/core/src/util/encode.ts:1-11`),
//! draft remount identity (`packages/app/src/app.tsx:209-230`) and
//! `createSessionOwnership` (`packages/app/src/pages/session/session-ownership.ts`).
//! Scope: pure values. The browser root keys transport/cache by `ServerKey`; the terminal keys by
//! session. Both share these types, neither erases the other's remount boundary.
//! Uses:
//! rust/crates/
//! `-- none
//! External:
//! `-- base64::engine::general_purpose::URL_SAFE_NO_PAD
//! Used by:
//! rust/crates/
//! |-- view-model/src/actions.rs
//! |-- view-model/src/lib.rs
//! `-- view-model/tests/identity.rs
//! Invariants: route segments are base64url without padding over UTF-8; `require_server_key`
//! rejects segments that do not round-trip; a captured ownership is current only while the
//! generation is unchanged (any session-key transition or disposal invalidates it).
//! Contract tests: view-model/tests/identity.rs

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

/// Identity of one server connection; owns transport, caches and subscriptions.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ServerKey(pub String);

/// Connection descriptor (`ServerConnection.Any`) reduced to what identity needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServerConnection {
    Http { url: String },
    Sidecar,
    SidecarWsl { distro: String },
    Ssh { host: String },
}

impl ServerConnection {
    pub fn key(&self) -> ServerKey {
        ServerKey(match self {
            Self::Http { url } => url.clone(),
            Self::Sidecar => "sidecar".into(),
            Self::SidecarWsl { distro } => format!("wsl:{distro}"),
            Self::Ssh { host } => format!("ssh:{host}"),
        })
    }
}

pub fn encode_segment(value: &str) -> String {
    URL_SAFE_NO_PAD.encode(value.as_bytes())
}

pub fn decode_segment(segment: &str) -> Option<String> {
    let trimmed = segment.trim_end_matches('=');
    let bytes = URL_SAFE_NO_PAD.decode(trimmed).ok()?;
    String::from_utf8(bytes).ok()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvalidServerRoute;

/// Decode a `/server/:serverKey/...` segment, rejecting non-canonical encodings.
pub fn require_server_key(segment: &str) -> Result<ServerKey, InvalidServerRoute> {
    let key = decode_segment(segment)
        .filter(|k| !k.is_empty())
        .ok_or(InvalidServerRoute)?;
    if encode_segment(&key) != segment {
        return Err(InvalidServerRoute);
    }
    Ok(ServerKey(key))
}

pub fn session_href(server: &ServerKey, session_id: &str) -> String {
    format!("/server/{}/session/{session_id}", encode_segment(&server.0))
}

pub fn legacy_session_href(directory: &str, session_id: &str) -> String {
    format!("/{}/session/{session_id}", encode_segment(directory))
}

/// Pick the server for a legacy `/:dir/session/:id` link from persisted tabs
/// (`legacySessionServer`): prefer the active server, else the unique match, else active.
pub fn legacy_session_server(
    tabs: &[(ServerKey, String)],
    session_id: &str,
    active: &ServerKey,
) -> ServerKey {
    let matches: Vec<&ServerKey> = tabs
        .iter()
        .filter(|(_, s)| s == session_id)
        .map(|(k, _)| k)
        .collect();
    if matches.contains(&active) {
        return active.clone();
    }
    match matches.as_slice() {
        [only] => (*only).clone(),
        _ => active.clone(),
    }
}

/// Scope that owns a directory view (layout, file tree, terminals) under one server.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DirectoryKey {
    pub server: ServerKey,
    pub directory: String,
}

/// Scope of a viewed session. Session changes remount only session-scoped state.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SessionKey {
    pub server: ServerKey,
    pub session_id: String,
}

/// Unsent new-session draft; remounts when server or directory change (`${server}\0${dir}`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DraftKey {
    pub server: ServerKey,
    pub directory: String,
    pub draft_id: String,
}

impl DraftKey {
    pub fn remount_key(&self) -> String {
        format!("{}\0{}", self.server.0, self.directory)
    }
}

/// Target a prompt or action was captured for. Late results apply only to this target.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Target {
    Session(SessionKey),
    Draft(DraftKey),
}

impl Target {
    pub fn server(&self) -> &ServerKey {
        match self {
            Self::Session(s) => &s.server,
            Self::Draft(d) => &d.server,
        }
    }
    /// Stable storage key for drafts and per-target UI state.
    pub fn storage_key(&self) -> String {
        match self {
            Self::Session(s) => format!("session:{}:{}", s.server.0, s.session_id),
            Self::Draft(d) => format!("draft:{}:{}:{}", d.server.0, d.directory, d.draft_id),
        }
    }
}

/// Generation counter for the visible session key (`createSessionOwnership`).
#[derive(Clone, Debug)]
pub struct Ownership {
    current: String,
    generation: u64,
}

/// Snapshot taken when an async action starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Captured {
    pub key: String,
    generation: u64,
}

impl Ownership {
    pub fn new(session_key: impl Into<String>) -> Self {
        Self {
            current: session_key.into(),
            generation: 0,
        }
    }

    /// Observe the current session key; a change bumps the generation.
    pub fn transition(&mut self, session_key: &str) {
        if session_key == self.current {
            return;
        }
        self.current = session_key.to_owned();
        self.generation += 1;
    }

    /// Owner disposed (view unmounted): every outstanding capture becomes stale.
    pub fn dispose(&mut self) {
        self.generation += 1;
    }

    pub fn key(&self) -> String {
        format!("{}:{}", self.generation, self.current)
    }

    pub fn capture(&self) -> Captured {
        Captured {
            key: self.key(),
            generation: self.generation,
        }
    }

    pub fn is_current(&self, captured: &Captured) -> bool {
        self.generation == captured.generation
    }

    /// Run `action` only if the capture is still current (`captured.run`).
    pub fn run<T>(&self, captured: &Captured, action: impl FnOnce() -> T) -> Option<T> {
        self.is_current(captured).then(action)
    }
}
