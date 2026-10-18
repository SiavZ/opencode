//! Purpose: diagnostic collection for one client: merged push+pull view with baseline dedupe,
//! pull `textDocument/diagnostic` (static + registered identifiers, related documents),
//! `workspace/diagnostic`, and the document/full wait loops with fresh-push debounce.
//! Ports packages/opencode/src/lsp/client.ts:91-105,270-541,623-639.
//! Scope: methods on `Client`; state lives in client.rs `Shared`.
//! Uses:
//! rust/crates/
//! |-- integration-adapters/src/lsp/client.rs
//! |-- integration-adapters/src/lsp/text.rs
//! `-- integration-api/src/language.rs
//! External:
//! |-- futures::stream::FuturesUnordered
//! `-- serde_json::{Value, json}
//! Used by:
//! rust/crates/
//! |-- integration-adapters/src/lsp/service.rs
//! `-- integration-adapters/tests/lsp_client.rs
//! Invariants: document mode unblocks as soon as one pull batch has items for the file (remaining
//! pulls keep merging in the background, joined at shutdown), full mode waits for all pulls,
//! push waits require matching version or a publish newer than `after`, then debounce 150ms.
//! Contract tests: integration-adapters/tests/lsp_client.rs

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::time::Instant;

use futures::StreamExt;
use futures::stream::FuturesUnordered;
use opencode_integration_api::language::{Diagnostic, DiagnosticsWait};
use serde_json::{Value, json};

use super::client::{Client, parse_diagnostics};
use super::text::{file_uri, uri_to_path};

#[derive(Default)]
struct PullResult {
    handled: bool,
    matched: bool,
    by_file: Vec<(String, Vec<Diagnostic>)>,
}

impl PullResult {
    fn add(&mut self, path: String, items: Vec<Diagnostic>) {
        match self.by_file.iter_mut().find(|(p, _)| *p == path) {
            Some((_, list)) => list.extend(items),
            None => self.by_file.push((path, items)),
        }
    }
    fn has_items(&self, path: &str) -> bool {
        self.by_file.iter().any(|(p, l)| p == path && !l.is_empty())
    }
}

/// client.ts:91-105 `dedupeDiagnostics` on (code, severity, message, source, range).
pub fn dedupe(items: Vec<Diagnostic>) -> Vec<Diagnostic> {
    let mut seen = HashSet::new();
    items
        .into_iter()
        .filter(|d| {
            let key = json!([d.code, d.severity, d.message, d.source, d.range]).to_string();
            seen.insert(key)
        })
        .collect()
}

impl Client {
    /// `client.diagnostics`: merged push then pull per path, deduped.
    pub fn diagnostics(&self) -> BTreeMap<String, Vec<Diagnostic>> {
        let state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        let keys: HashSet<&String> = state.push.keys().chain(state.pull.keys()).collect();
        keys.into_iter()
            .map(|k| {
                let mut all = state.push.get(k).cloned().unwrap_or_default();
                all.extend(state.pull.get(k).cloned().unwrap_or_default());
                (k.clone(), dedupe(all))
            })
            .collect()
    }

    /// `waitForDiagnostics({path, version, mode, after})`.
    pub async fn wait_for_diagnostics(
        &self,
        file: &str,
        version: i64,
        mode: DiagnosticsWait,
        after: Instant,
    ) {
        let path = self.normalize(file);
        let (limit, full) = match mode {
            DiagnosticsWait::Document => (self.timeouts.document_wait, false),
            DiagnosticsWait::Full => (self.timeouts.full_wait, true),
        };
        let deadline = after + limit;
        let mut registrations = self.shared.registration.subscribe();
        let push = self.wait_fresh_push(&path, version, after, deadline);
        tokio::pin!(push);
        loop {
            if Instant::now() >= deadline {
                return;
            }
            let (handled, matched) = if full {
                self.pull_full(&path).await
            } else {
                self.pull_document(&path).await
            };
            if matched || (full && handled) {
                return;
            }
            registrations.borrow_and_update();
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return;
            }
            tokio::select! {
                // A resolved push wait (fresh push or its own timeout) ends the wait.
                _ = &mut push => return,
                changed = tokio::time::timeout(remaining, registrations.changed()) => {
                    if !matches!(changed, Ok(Ok(()))) { return; }
                }
            }
        }
    }

    async fn wait_fresh_push(
        &self,
        path: &str,
        version: i64,
        after: Instant,
        deadline: Instant,
    ) -> bool {
        let mut rx = self.shared.pushes.subscribe();
        let mut fire: Option<Instant> = self.push_ready(path, version, after);
        loop {
            let sleep_until = match fire {
                Some(at) => at.min(deadline),
                None => deadline,
            };
            tokio::select! {
                _ = tokio::time::sleep_until(sleep_until.into()) => {
                    return fire.is_some_and(|at| at <= deadline && Instant::now() >= at);
                }
                msg = rx.recv() => match msg {
                    Ok(p) if p == path => {
                        if let Some(at) = self.push_ready(path, version, after) { fire = Some(at); }
                    }
                    Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(_) => return false,
                }
            }
        }
    }

    /// client.ts:479-486 `schedule`: returns when the debounce timer should fire.
    fn push_ready(&self, path: &str, version: i64, after: Instant) -> Option<Instant> {
        let state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        let (at, hit_version) = *state.published.get(path)?;
        if hit_version.is_some_and(|v| v != version) {
            return None;
        }
        if at < after && hit_version != Some(version) {
            return None;
        }
        Some(at + self.timeouts.debounce)
    }

    fn pull_identifiers(&self, workspace: bool) -> (bool, Vec<String>) {
        let state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        let regs: Vec<_> = state
            .registrations
            .iter()
            .filter(|(_, r)| r.workspace == workspace)
            .collect();
        let mut ids: Vec<String> = Vec::new();
        for id in regs.iter().filter_map(|(_, r)| r.identifier.clone()) {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        let supported = if workspace {
            !regs.is_empty()
        } else {
            self.static_pull || !regs.is_empty()
        };
        (supported, ids)
    }

    async fn pull_document(&self, path: &str) -> (bool, bool) {
        let (supported, ids) = self.pull_identifiers(false);
        if !supported {
            return (false, false);
        }
        let mut pending: FuturesUnordered<_> = std::iter::once(None)
            .chain(ids.into_iter().map(Some))
            .map(|id| self.document_report(path.to_string(), id))
            .collect();
        let mut results = Vec::new();
        let mut merged = (false, false);
        while let Some(result) = pending.next().await {
            results.push(result);
            merged = self.merge(path, &results);
            if results.iter().any(|r| r.has_items(path)) {
                break;
            }
        }
        if !pending.is_empty() {
            self.finish_in_background(path.to_string(), results, pending);
        }
        merged
    }

    /// Keep merging slower identifier pulls after the caller unblocked (client.ts:412-415).
    fn finish_in_background<F>(
        &self,
        path: String,
        mut results: Vec<PullResult>,
        mut pending: FuturesUnordered<F>,
    ) where
        F: std::future::Future<Output = PullResult> + Send + 'static,
    {
        let shared = self.shared.clone();
        let mut tasks = self.background.lock().unwrap_or_else(|e| e.into_inner());
        tasks.spawn(async move {
            while let Some(result) = pending.next().await {
                results.push(result);
                merge_into(&shared, &path, &results);
            }
        });
    }

    async fn pull_full(&self, path: &str) -> (bool, bool) {
        let (doc_supported, doc_ids) = self.pull_identifiers(false);
        let (ws_supported, ws_ids) = self.pull_identifiers(true);
        if !doc_supported && !ws_supported {
            return (false, false);
        }
        let mut futures: Vec<futures::future::BoxFuture<'static, PullResult>> = Vec::new();
        if doc_supported {
            futures.push(Box::pin(self.document_report(path.to_string(), None)));
        }
        for id in doc_ids {
            futures.push(Box::pin(self.document_report(path.to_string(), Some(id))));
        }
        if ws_supported {
            futures.push(Box::pin(self.workspace_report(path.to_string(), None)));
        }
        for id in ws_ids {
            futures.push(Box::pin(self.workspace_report(path.to_string(), Some(id))));
        }
        let results = futures::future::join_all(futures).await;
        self.merge(path, &results)
    }

    fn merge(&self, path: &str, results: &[PullResult]) -> (bool, bool) {
        merge_into(&self.shared, path, results)
    }

    fn document_report(
        &self,
        path: String,
        identifier: Option<String>,
    ) -> impl std::future::Future<Output = PullResult> + Send + 'static {
        let connection = self.connection.clone();
        let limit = self.timeouts.diagnostics_request;
        async move {
            let mut params = json!({"textDocument": {"uri": file_uri(Path::new(&path))}});
            if let Some(id) = identifier {
                params["identifier"] = Value::String(id);
            }
            let report = connection
                .request("textDocument/diagnostic", params, Some(limit))
                .await;
            let Some(report) = report.ok().filter(|r| !r.is_null()) else {
                return PullResult::default();
            };
            let mut out = PullResult::default();
            if let Some(items) = report.get("items").filter(|v| v.is_array()) {
                out.add(path.clone(), parse_diagnostics(Some(items)));
                out.handled = true;
                out.matched = true;
            }
            for (uri, related) in report
                .get("relatedDocuments")
                .and_then(Value::as_object)
                .into_iter()
                .flatten()
            {
                let (Some(rel), Some(items)) = (
                    uri_to_path(uri),
                    related.get("items").filter(|v| v.is_array()),
                ) else {
                    continue;
                };
                out.matched |= rel == path;
                out.add(rel, parse_diagnostics(Some(items)));
                out.handled = true;
            }
            out
        }
    }

    fn workspace_report(
        &self,
        path: String,
        identifier: Option<String>,
    ) -> impl std::future::Future<Output = PullResult> + Send + 'static {
        let connection = self.connection.clone();
        let limit = self.timeouts.diagnostics_request;
        async move {
            let mut params = json!({"previousResultIds": []});
            if let Some(id) = identifier {
                params["identifier"] = Value::String(id);
            }
            let report = connection
                .request("workspace/diagnostic", params, Some(limit))
                .await;
            let Some(report) = report.ok().filter(|r| !r.is_null()) else {
                return PullResult::default();
            };
            let mut out = PullResult {
                handled: true,
                ..PullResult::default()
            };
            for item in report
                .get("items")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let rel = item
                    .get("uri")
                    .and_then(Value::as_str)
                    .and_then(uri_to_path);
                let (Some(rel), Some(items)) = (rel, item.get("items").filter(|v| v.is_array()))
                else {
                    continue;
                };
                out.matched |= rel == path;
                out.add(rel, parse_diagnostics(Some(items)));
            }
            out
        }
    }
}

/// client.ts:272-291 `mergeResults`.
fn merge_into(shared: &super::client::Shared, path: &str, results: &[PullResult]) -> (bool, bool) {
    let handled = results.iter().any(|r| r.handled);
    let matched = results.iter().any(|r| r.matched);
    if !handled {
        return (false, false);
    }
    let mut merged: HashMap<String, Vec<Diagnostic>> = HashMap::new();
    for (target, items) in results.iter().flat_map(|r| r.by_file.iter()) {
        merged
            .entry(target.clone())
            .or_default()
            .extend(items.iter().cloned());
    }
    if matched {
        merged.entry(path.to_string()).or_default();
    }
    let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
    for (target, items) in merged {
        state.pull.insert(target, dedupe(items));
    }
    (handled, matched)
}
