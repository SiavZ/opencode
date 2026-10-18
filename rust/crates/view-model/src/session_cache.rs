//! Purpose: per-server canonical session cache: live event folding, page/live reconciliation
//! during loads, removals, optimistic user roots, status, and bounded LRU eviction with pins.
//! Port of the V2 paths of `packages/app/src/context/server-session.ts:151-184,540-731,883-983`
//! and `packages/app/src/context/global-sync/session-cache.ts` (`pickSessionCacheEvictions`).
//! Scope: synchronous state; the host performs fetches and calls `begin_load`/`apply_page`.
//! Uses:
//! rust/crates/
//! |-- view-model/src/projection/mod.rs
//! |-- view-model/src/reducer/mod.rs
//! |-- view-model/src/source.rs
//! `-- view-model/src/timeline/mod.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! |-- view-model/src/lib.rs
//! `-- view-model/tests/session_cache.rs
//! Invariants: events observed while a load is pending win over the fetched page for the same
//! identity; a removed message is never restored by a stale page; an incomplete latest page keeps
//! cached messages older than its oldest message and drops omitted newer ones; pinned and the
//! kept session are never evicted; at most `limit` sessions stay cached.
//! Contract tests: view-model/tests/session_cache.rs

use std::collections::{HashMap, HashSet};

use crate::projection::{RenderMessage, message_key};
use crate::reducer::V2SessionReducer;
use crate::source::{SessionEvent, SessionMessage};
use crate::timeline::SessionStatus;

pub const SESSION_CACHE_LIMIT: usize = 40;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageMode {
    /// Newest page (initial sync or forced refresh).
    Latest,
    /// History prepend (`loadMore`).
    Older,
}

/// One fetched page in chronological order.
#[derive(Clone, Debug, PartialEq)]
pub struct Page {
    pub source: Vec<SessionMessage>,
    pub cursor: Option<String>,
    pub complete: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SessionEntry {
    pub source: Vec<SessionMessage>,
    pub status: Option<SessionStatus>,
    pub cursor: Option<String>,
    pub complete: bool,
    /// Optimistic user roots not yet confirmed by the source.
    pub optimistic: Vec<RenderMessage>,
    /// Bumped on every visible change, for cheap renderer invalidation.
    pub revision: u64,
}

#[derive(Debug, Default)]
struct LoadState {
    touched: HashSet<String>,
    removed: HashSet<String>,
}

/// Outcome of applying a live event.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Applied {
    pub session_id: String,
    pub touched: Vec<String>,
    /// Promoted input whose admission was missed: fetch that message and call `hydrate`.
    pub hydrate: Option<String>,
}

#[derive(Debug)]
pub struct SessionCache {
    entries: HashMap<String, SessionEntry>,
    /// LRU order, oldest first.
    seen: Vec<String>,
    pinned: HashSet<String>,
    removed: HashMap<String, HashSet<String>>,
    loads: HashMap<String, LoadState>,
    reducer: V2SessionReducer,
    limit: usize,
}

impl Default for SessionCache {
    fn default() -> Self {
        Self::new(SESSION_CACHE_LIMIT)
    }
}

impl SessionCache {
    pub fn new(limit: usize) -> Self {
        Self {
            entries: HashMap::new(),
            seen: vec![],
            pinned: HashSet::new(),
            removed: HashMap::new(),
            loads: HashMap::new(),
            reducer: V2SessionReducer::new(),
            limit,
        }
    }

    pub fn get(&self, session_id: &str) -> Option<&SessionEntry> {
        self.entries.get(session_id)
    }

    pub fn source(&self, session_id: &str) -> &[SessionMessage] {
        self.entries
            .get(session_id)
            .map(|e| e.source.as_slice())
            .unwrap_or(&[])
    }

    /// Pin a viewed session: exempt from eviction until unpinned.
    pub fn pin(&mut self, session_id: &str) {
        self.pinned.insert(session_id.to_owned());
    }

    pub fn unpin(&mut self, session_id: &str) {
        self.pinned.remove(session_id);
    }

    fn entry(&mut self, session_id: &str) -> &mut SessionEntry {
        self.remember(session_id);
        self.entries.entry(session_id.to_owned()).or_default()
    }

    /// Mark a session as recently used and evict beyond the limit. Returns evicted ids.
    pub fn remember(&mut self, session_id: &str) -> Vec<String> {
        let stale = pick_evictions(&mut self.seen, session_id, self.limit, &self.pinned);
        for id in &stale {
            self.entries.remove(id);
            self.loads.remove(id);
            self.removed.remove(id);
            self.reducer.clear(id);
        }
        stale
    }

    pub fn apply_event(&mut self, event: &SessionEvent) -> Option<Applied> {
        let session_id = event.data.get("sessionID")?.as_str()?.to_owned();
        let source = self.source(&session_id).to_vec();
        let reduction = self.reducer.reduce(&source, event);
        let status = match event.kind.as_str() {
            "session.execution.started" => Some(SessionStatus::Busy),
            "session.execution.succeeded"
            | "session.execution.failed"
            | "session.execution.interrupted" => Some(SessionStatus::Idle),
            "session.retry.scheduled" => Some(SessionStatus::Retry),
            _ => None,
        };
        if reduction.is_none() && status.is_none() {
            return None;
        }
        let load_touched = reduction
            .as_ref()
            .map(|r| r.touched.clone())
            .unwrap_or_default();
        if let Some(load) = self.loads.get_mut(&session_id) {
            load.touched.extend(load_touched);
        }
        let entry = self.entry(&session_id);
        if let Some(status) = status {
            entry.status = Some(status);
        }
        let mut applied = Applied {
            session_id: session_id.clone(),
            ..Applied::default()
        };
        if let Some(r) = reduction {
            entry.source = r.messages;
            applied.touched = r.touched;
            applied.hydrate = r.missing;
            confirm_optimistic(entry);
        }
        entry.revision += 1;
        Some(applied)
    }

    /// Insert or replace one fetched message (hydration after a missed admission).
    pub fn hydrate(&mut self, session_id: &str, message: SessionMessage) {
        if self
            .removed
            .get(session_id)
            .is_some_and(|r| r.contains(message.id()))
        {
            return;
        }
        let entry = self.entry(session_id);
        entry.source.retain(|m| m.id() != message.id());
        entry.source.push(message);
        entry
            .source
            .sort_by_key(|m| message_key(m.created(), m.id()));
        confirm_optimistic(entry);
        entry.revision += 1;
    }

    /// `message.removed`: drop it and remember the removal against stale pages.
    pub fn remove_message(&mut self, session_id: &str, message_id: &str) {
        self.removed
            .entry(session_id.to_owned())
            .or_default()
            .insert(message_id.to_owned());
        if let Some(load) = self.loads.get_mut(session_id) {
            load.removed.insert(message_id.to_owned());
        }
        if let Some(entry) = self.entries.get_mut(session_id) {
            entry.source.retain(|m| m.id() != message_id);
            entry.revision += 1;
        }
    }

    pub fn set_status(&mut self, session_id: &str, status: SessionStatus) {
        let entry = self.entry(session_id);
        entry.status = Some(status);
        entry.revision += 1;
    }

    pub fn add_optimistic(&mut self, session_id: &str, message: RenderMessage) {
        let entry = self.entry(session_id);
        entry.optimistic.retain(|m| m.id != message.id);
        entry.optimistic.push(message);
        entry.revision += 1;
    }

    pub fn remove_optimistic(&mut self, session_id: &str, message_id: &str) {
        if let Some(entry) = self.entries.get_mut(session_id) {
            entry.optimistic.retain(|m| m.id != message_id);
            entry.revision += 1;
        }
    }

    /// Start tracking events that race with an in-flight fetch.
    pub fn begin_load(&mut self, session_id: &str) {
        self.loads
            .insert(session_id.to_owned(), LoadState::default());
    }

    /// Abandon a load (all retries failed). Live state is kept as is.
    pub fn cancel_load(&mut self, session_id: &str) {
        self.loads.remove(session_id);
    }

    /// Merge a fetched page with live state (`applyMessagePage` + `reconcileFetched`).
    pub fn apply_page(&mut self, session_id: &str, mode: PageMode, page: Page) {
        let load = self.loads.remove(session_id).unwrap_or_default();
        let removed = self.removed.get(session_id).cloned().unwrap_or_default();
        let entry = self.entry(session_id);
        let live: HashMap<String, SessionMessage> = entry
            .source
            .iter()
            .map(|m| (m.id().to_owned(), m.clone()))
            .collect();
        let incoming: HashSet<&str> = page.source.iter().map(SessionMessage::id).collect();
        let boundary = page
            .source
            .iter()
            .map(|m| message_key(m.created(), m.id()))
            .min();
        let keep_unfetched = |m: &SessionMessage| match mode {
            PageMode::Older => true,
            PageMode::Latest if page.complete => false,
            PageMode::Latest => boundary
                .as_ref()
                .is_some_and(|b| message_key(m.created(), m.id()) < *b),
        };
        let current: Vec<SessionMessage> = entry
            .source
            .iter()
            .filter(|m| {
                !incoming.contains(m.id()) && (keep_unfetched(m) || load.touched.contains(m.id()))
            })
            .cloned()
            .collect();
        let mut merged = match mode {
            PageMode::Older => [page.source.clone(), current].concat(),
            PageMode::Latest => [current, page.source.clone()].concat(),
        };
        // Events observed during the request are the freshest state for those identities.
        for m in merged.iter_mut() {
            if load.touched.contains(m.id())
                && let Some(l) = live.get(m.id())
            {
                *m = l.clone();
            }
        }
        merged.retain(|m| !removed.contains(m.id()) && !load.removed.contains(m.id()));
        // Live messages retained beside the page must stay chronological (`compareMessages`).
        merged.sort_by_cached_key(|m| message_key(m.created(), m.id()));
        entry.source = merged;
        if mode == PageMode::Latest || page.cursor.is_some() || page.complete {
            entry.cursor = page.cursor;
            entry.complete = page.complete;
        }
        confirm_optimistic(entry);
        entry.revision += 1;
    }
}

fn confirm_optimistic(entry: &mut SessionEntry) {
    let ids: HashSet<&str> = entry.source.iter().map(SessionMessage::id).collect();
    entry.optimistic.retain(|m| !ids.contains(m.id.as_str()));
}

/// `pickSessionCacheEvictions`: move `keep` to most-recent, evict oldest unpinned beyond `limit`.
pub fn pick_evictions(
    seen: &mut Vec<String>,
    keep: &str,
    limit: usize,
    preserve: &HashSet<String>,
) -> Vec<String> {
    seen.retain(|id| id != keep);
    seen.push(keep.to_owned());
    let mut stale = vec![];
    for id in seen.iter() {
        if seen.len() - stale.len() <= limit {
            break;
        }
        if id == keep || preserve.contains(id) {
            continue;
        }
        stale.push(id.clone());
    }
    seen.retain(|id| !stale.contains(id));
    stale
}

/// Older pages must continue while the oldest turn boundary is an assistant (`needsOlderTurnRoot`).
pub fn needs_older_turn_root(source: &[SessionMessage]) -> bool {
    let boundary = source.iter().find(|m| match m {
        SessionMessage::User { .. }
        | SessionMessage::Shell { .. }
        | SessionMessage::Assistant(_) => true,
        SessionMessage::Synthetic { description, .. } => {
            description.as_deref().is_some_and(|d| !d.trim().is_empty())
        }
        _ => false,
    });
    matches!(boundary, Some(SessionMessage::Assistant(_)))
}
