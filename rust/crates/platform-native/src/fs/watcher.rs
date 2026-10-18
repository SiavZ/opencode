//! Purpose: native filesystem watcher (notify 8) replacing the baseline `@parcel/watcher`
//! subscription (`packages/core/src/filesystem/watcher.ts`): add/change/unlink events, ignore
//! patterns, per-path coalescing, bounded delivery with a single `Overflow` marker.
//! Scope: one notify watcher plus one owned tokio task per `watch` call.
//! Uses:
//! rust/crates/
//! |-- platform-api/src/watcher.rs
//! |-- platform-native/src/fs/error.rs
//! `-- platform-native/src/fs/glob.rs
//! External:
//! |-- notify::{RecommendedWatcher, RecursiveMode}
//! `-- tokio::sync::mpsc
//! Used by:
//! rust/crates/
//! |-- platform-native/src/fs/mod.rs
//! `-- platform-native/tests/fs_watcher.rs
//! Invariants: paths are reported under the canonical root (FSEvents/parcel report real paths);
//! repeated events for one path within `coalesce` merge into one event settled against the
//! filesystem at flush (existing paths are add/change, missing paths unlink, create+delete inside
//! one window is dropped); when the raw or delivery queue is full, events are discarded and
//! exactly one `Overflow` is delivered, with nothing else until the consumer reads it; backend
//! rescan flags and errors also surface as `Overflow`; `close` cancels, drops the
//! notify watcher and joins the task.
//! Contract tests: platform-native/tests/fs_watcher.rs

use super::error::io_error;
use super::glob::compile;
use async_trait::async_trait;
use globset::GlobMatcher;
use notify::event::{EventKind, ModifyKind, RenameMode};
use notify::{RecommendedWatcher, RecursiveMode, Watcher as _};
use opencode_platform_api::watcher::{WatchEvent, WatchHandle, WatchKind, WatchOptions, Watcher};
use opencode_platform_api::{PlatformError, PlatformErrorKind};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Default)]
pub struct NativeWatcher;

impl NativeWatcher {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Watcher for NativeWatcher {
    async fn watch(
        &self,
        root: PathBuf,
        options: WatchOptions,
    ) -> Result<Box<dyn WatchHandle>, PlatformError> {
        let root = tokio::fs::canonicalize(&root)
            .await
            .map_err(|error| io_error("watch", &root, &error))?;
        let capacity = options.capacity.max(1);
        let (raw_tx, raw_rx) = mpsc::channel::<(PathBuf, WatchKind)>(capacity);
        let (out_tx, out_rx) = mpsc::channel::<WatchEvent>(capacity);
        let overflow = Arc::new(AtomicBool::new(false));
        let marker = Arc::new(AtomicBool::new(false));
        let flag = overflow.clone();
        let mut watcher =
            notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
                let Ok(event) = result else {
                    flag.store(true, Ordering::SeqCst);
                    return;
                };
                if event.need_rescan() {
                    flag.store(true, Ordering::SeqCst);
                    return;
                }
                for (path, kind) in classify(&event) {
                    if raw_tx.try_send((path, kind)).is_err() {
                        flag.store(true, Ordering::SeqCst);
                    }
                }
            })
            .map_err(|error| notify_error(&root, error))?;
        let mode = if options.recursive {
            RecursiveMode::Recursive
        } else {
            RecursiveMode::NonRecursive
        };
        watcher
            .watch(&root, mode)
            .map_err(|error| notify_error(&root, error))?;
        let cancel = CancellationToken::new();
        let filter = Filter::new(&root, &options.ignore);
        let task = tokio::spawn(run(Pump {
            raw: raw_rx,
            out: out_tx,
            overflow,
            marker: marker.clone(),
            filter,
            window: options.coalesce,
            cancel: cancel.clone(),
        }));
        Ok(Box::new(NativeWatchHandle {
            events: tokio::sync::Mutex::new(out_rx),
            marker,
            cancel,
            task: tokio::sync::Mutex::new(Some(task)),
            watcher: std::sync::Mutex::new(Some(watcher)),
        }))
    }
}

#[derive(Debug)]
struct NativeWatchHandle {
    events: tokio::sync::Mutex<mpsc::Receiver<WatchEvent>>,
    /// Set while an undelivered `Overflow` sits in the queue; cleared when the consumer reads it.
    marker: Arc<AtomicBool>,
    cancel: CancellationToken,
    task: tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
    watcher: std::sync::Mutex<Option<RecommendedWatcher>>,
}

#[async_trait]
impl WatchHandle for NativeWatchHandle {
    async fn next(&self) -> Option<WatchEvent> {
        let event = self.events.lock().await.recv().await;
        if event == Some(WatchEvent::Overflow) {
            self.marker.store(false, Ordering::SeqCst);
        }
        event
    }

    async fn close(&self) {
        self.cancel.cancel();
        let watcher = self
            .watcher
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .take();
        drop(watcher);
        let task = self.task.lock().await.take();
        if let Some(task) = task {
            let _ = task.await;
        }
    }
}

impl Drop for NativeWatchHandle {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

fn notify_error(root: &Path, error: notify::Error) -> PlatformError {
    if let notify::ErrorKind::Io(io) = &error.kind {
        return io_error("watch", root, io);
    }
    let kind = match error.kind {
        notify::ErrorKind::PathNotFound => PlatformErrorKind::NotFound,
        notify::ErrorKind::MaxFilesWatch => PlatformErrorKind::Io,
        _ => PlatformErrorKind::Io,
    };
    PlatformError::new(kind, "watch", root.to_string_lossy(), error.to_string())
}

/// Map a notify event to baseline kinds. Renames without a side report whether the path exists.
fn classify(event: &notify::Event) -> Vec<(PathBuf, WatchKind)> {
    let fixed = match event.kind {
        EventKind::Create(_) => Some(WatchKind::Add),
        EventKind::Remove(_) => Some(WatchKind::Unlink),
        EventKind::Modify(ModifyKind::Name(RenameMode::From)) => Some(WatchKind::Unlink),
        EventKind::Modify(ModifyKind::Name(RenameMode::To)) => Some(WatchKind::Add),
        EventKind::Modify(ModifyKind::Name(RenameMode::Both)) => {
            let mut out = Vec::new();
            if let [from, to, ..] = event.paths.as_slice() {
                out.push((from.clone(), WatchKind::Unlink));
                out.push((to.clone(), WatchKind::Add));
            }
            return out;
        }
        EventKind::Modify(ModifyKind::Name(_)) => None,
        EventKind::Modify(ModifyKind::Metadata(_)) => return Vec::new(),
        EventKind::Modify(_) => Some(WatchKind::Change),
        EventKind::Access(_) | EventKind::Other | EventKind::Any => return Vec::new(),
    };
    event
        .paths
        .iter()
        .map(|path| {
            let kind = fixed.unwrap_or_else(|| {
                if path.symlink_metadata().is_ok() {
                    WatchKind::Add
                } else {
                    WatchKind::Unlink
                }
            });
            (path.clone(), kind)
        })
        .collect()
}

/// Parcel-style ignore list: plain entries are paths (relative to the root unless absolute) that
/// ignore themselves and everything below; entries with glob syntax match the relative path.
#[derive(Debug)]
struct Filter {
    root: PathBuf,
    prefixes: Vec<PathBuf>,
    globs: Vec<GlobMatcher>,
}

impl Filter {
    fn new(root: &Path, ignore: &[String]) -> Self {
        let mut prefixes = Vec::new();
        let mut globs = Vec::new();
        for pattern in ignore {
            if pattern.contains(['*', '?', '[', '{']) {
                globs.extend(compile(pattern));
                continue;
            }
            let path = Path::new(pattern);
            prefixes.push(if path.is_absolute() {
                path.to_path_buf()
            } else {
                root.join(path)
            });
        }
        Self {
            root: root.to_path_buf(),
            prefixes,
            globs,
        }
    }

    fn ignored(&self, path: &Path) -> bool {
        if self.prefixes.iter().any(|prefix| path.starts_with(prefix)) {
            return true;
        }
        let Ok(relative) = path.strip_prefix(&self.root) else {
            return false;
        };
        let relative = relative.to_string_lossy().replace('\\', "/");
        self.globs.iter().any(|glob| glob.is_match(&relative))
    }
}

struct Pump {
    raw: mpsc::Receiver<(PathBuf, WatchKind)>,
    out: mpsc::Sender<WatchEvent>,
    overflow: Arc<AtomicBool>,
    marker: Arc<AtomicBool>,
    filter: Filter,
    window: std::time::Duration,
    cancel: CancellationToken,
}

struct Pending {
    kind: Option<WatchKind>,
    first: WatchKind,
    deadline: Instant,
    order: u64,
}

/// In-window coalescing for precise backends. `None` means the pair cancels out.
fn merge(previous: Option<WatchKind>, next: WatchKind) -> Option<WatchKind> {
    match (previous, next) {
        (None, next) => Some(next),
        (Some(WatchKind::Add), WatchKind::Change) => Some(WatchKind::Add),
        (Some(WatchKind::Add), WatchKind::Unlink) => None,
        (Some(WatchKind::Unlink), WatchKind::Add | WatchKind::Change) => Some(WatchKind::Change),
        (Some(_), next) => Some(next),
    }
}

/// Bound for the announced-path set; past it the set resets and removals are always reported.
const ANNOUNCED_LIMIT: usize = 65_536;

/// Settle the coalesced kind against the filesystem at flush time, like parcel's FSEvents
/// backend: FSEvents merges sticky create/modify/remove flags per path, so raw order is
/// unreliable. An existing path is never reported as unlinked. A missing path is an unlink,
/// except when its first raw event in this window was a create and it was never announced
/// (created and removed inside the window).
fn settle(path: &Path, item: &Pending, announced: &HashSet<PathBuf>) -> Option<WatchKind> {
    if path.symlink_metadata().is_ok() {
        return match item.kind {
            None => Some(WatchKind::Add),
            Some(WatchKind::Unlink) => Some(WatchKind::Change),
            Some(kind) => Some(kind),
        };
    }
    if item.first == WatchKind::Add && !announced.contains(path) {
        return None;
    }
    Some(WatchKind::Unlink)
}

async fn run(mut pump: Pump) {
    let mut pending: HashMap<PathBuf, Pending> = HashMap::new();
    let mut announced: HashSet<PathBuf> = HashSet::new();
    let mut order = 0u64;
    let mut overflowed = false;
    loop {
        if pump.overflow.swap(false, Ordering::SeqCst) {
            // One Overflow per episode: while a marker is still unread, keep discarding.
            overflowed = !pump.marker.load(Ordering::SeqCst);
            pending.clear();
            while pump.raw.try_recv().is_ok() {}
        }
        if overflowed {
            match pump.out.try_send(WatchEvent::Overflow) {
                Ok(()) => {
                    overflowed = false;
                    pump.marker.store(true, Ordering::SeqCst);
                }
                Err(mpsc::error::TrySendError::Closed(_)) => return,
                Err(mpsc::error::TrySendError::Full(_)) => {}
            }
        }
        let now = Instant::now();
        let mut due: Vec<(PathBuf, Pending)> = Vec::new();
        let discarding = overflowed || pump.marker.load(Ordering::SeqCst);
        if discarding {
            pending.clear();
        } else {
            let ready: Vec<PathBuf> = pending
                .iter()
                .filter(|(_, item)| item.deadline <= now)
                .map(|(path, _)| path.clone())
                .collect();
            due = ready
                .into_iter()
                .filter_map(|path| pending.remove(&path).map(|item| (path, item)))
                .collect();
            due.sort_by_key(|(_, item)| item.order);
        }
        for (path, item) in due {
            let Some(kind) = settle(&path, &item, &announced) else {
                continue;
            };
            if kind == WatchKind::Unlink {
                announced.remove(&path);
            } else {
                if announced.len() >= ANNOUNCED_LIMIT {
                    announced.clear();
                }
                announced.insert(path.clone());
            }
            match pump.out.try_send(WatchEvent::Changed { path, kind }) {
                Ok(()) => {}
                Err(mpsc::error::TrySendError::Closed(_)) => return,
                Err(mpsc::error::TrySendError::Full(_)) => {
                    pump.overflow.store(true, Ordering::SeqCst);
                    break;
                }
            }
        }
        if pump.overflow.load(Ordering::SeqCst) {
            continue;
        }
        let next_deadline = pending.values().map(|item| item.deadline).min();
        // While an Overflow is undeliverable or unread, poll for consumer progress.
        let wake = if discarding {
            Some(Instant::now() + std::time::Duration::from_millis(10))
        } else {
            next_deadline
        };
        tokio::select! {
            () = pump.cancel.cancelled() => return,
            received = pump.raw.recv() => {
                let Some((path, kind)) = received else { return };
                if overflowed || pump.marker.load(Ordering::SeqCst) || pump.filter.ignored(&path) {
                    continue;
                }
                order += 1;
                let window = pump.window;
                let entry = pending.entry(path).or_insert_with(|| Pending {
                    kind: None,
                    first: kind,
                    deadline: Instant::now() + window,
                    order,
                });
                entry.kind = merge(entry.kind, kind);
            }
            () = sleep_until(wake) => {}
        }
    }
}

async fn sleep_until(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}
