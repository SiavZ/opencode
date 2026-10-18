//! Purpose: system context algebra (`packages/core/src/system-context/index.ts`): keyed typed
//! sources closed into an opaque `SystemContext`, ordered `combine` with duplicate-key rejection,
//! `initialize`, `reconcile` and `replace` over one observation, durable `Snapshot` values.
//! Scope: pure algebra plus async source observation. No storage.
//! Uses:
//! rust/crates/
//! |-- context/src/source/error.rs
//! `-- context/src/source/snapshot.rs
//! External:
//! |-- futures::future::try_join_all
//! |-- serde::{de::DeserializeOwned, Serialize}
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! |-- context/src/builtins.rs
//! |-- context/src/epoch.rs
//! |-- context/src/lib.rs
//! |-- context/src/registry.rs
//! `-- context/tests/system_context.rs
//! Invariants: unavailable is not removal (refresh keeps the admitted snapshot, replacement and
//! initialization block), each source is loaded once per observation, update text is rendered only
//! when the result is `Updated`, removals render in sorted key order, empty renderings fail.
//! Contract tests: context/tests/system_context.rs

use std::fmt;
use std::future::Future;
use std::sync::Arc;

use futures::future::try_join_all;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

mod error;
mod snapshot;

pub use error::{BoxError, BoxFuture, ContextError};
pub use snapshot::{
    Generation, InvalidKey, Key, ReconcileResult, ReplacementResult, Snapshot, SourceSnapshot,
};

/// One observation of a source: the value, or `unavailable` (temporary failure, not removal).
#[derive(Debug, Clone, PartialEq)]
pub enum Observation<A> {
    Available(A),
    Unavailable,
}

type Render<A> = Arc<dyn Fn(&A) -> String + Send + Sync>;
type RenderUpdate<A> = Arc<dyn Fn(&A, &A) -> String + Send + Sync>;
type Load<A> = Arc<dyn Fn() -> BoxFuture<Result<Observation<A>, ContextError>> + Send + Sync>;

/// One typed source before its value type is hidden by [`make`]. The serde representation of `A`
/// is the durable codec, `PartialEq` is the equivalence.
pub struct Source<A> {
    pub key: Key,
    pub load: Load<A>,
    pub baseline: Render<A>,
    pub update: RenderUpdate<A>,
    pub removed: Option<Render<A>>,
}

impl<A> Source<A> {
    /// Builder for the common case.
    pub fn new<L, Fut>(
        key: Key,
        load: L,
        baseline: impl Fn(&A) -> String + Send + Sync + 'static,
        update: impl Fn(&A, &A) -> String + Send + Sync + 'static,
    ) -> Self
    where
        L: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Observation<A>, ContextError>> + Send + 'static,
    {
        Self {
            key,
            load: Arc::new(move || Box::pin(load())),
            baseline: Arc::new(baseline),
            update: Arc::new(update),
            removed: None,
        }
    }

    pub fn with_removed(mut self, removed: impl Fn(&A) -> String + Send + Sync + 'static) -> Self {
        self.removed = Some(Arc::new(removed));
        self
    }
}

struct Rendered {
    text: String,
    snapshot: SourceSnapshot,
}

enum Compared<'a> {
    Incompatible,
    Unchanged,
    Updated(Box<dyn FnOnce() -> Result<Rendered, ContextError> + 'a>),
}

trait Loaded: Send {
    fn baseline(&self) -> Result<Rendered, ContextError>;
    fn compare(&self, previous: &Value) -> Compared<'_>;
}

struct LoadedValue<A> {
    key: Key,
    value: A,
    baseline: Render<A>,
    update: RenderUpdate<A>,
    removed: Option<Render<A>>,
}

impl<A: Serialize + DeserializeOwned + PartialEq + Send> LoadedValue<A> {
    fn snapshot(&self) -> Result<SourceSnapshot, ContextError> {
        let value =
            serde_json::to_value(&self.value).map_err(|e| ContextError::Source(Box::new(e)))?;
        let removed = match &self.removed {
            Some(render) => Some(require_text(&self.key, "removal", render(&self.value))?),
            None => None,
        };
        Ok(SourceSnapshot { value, removed })
    }
}

impl<A: Serialize + DeserializeOwned + PartialEq + Send> Loaded for LoadedValue<A> {
    fn baseline(&self) -> Result<Rendered, ContextError> {
        Ok(Rendered {
            text: require_text(&self.key, "baseline", (self.baseline)(&self.value))?,
            snapshot: self.snapshot()?,
        })
    }

    fn compare(&self, previous: &Value) -> Compared<'_> {
        let Ok(decoded) = A::deserialize(previous) else {
            return Compared::Incompatible;
        };
        if decoded == self.value {
            return Compared::Unchanged;
        }
        Compared::Updated(Box::new(move || {
            Ok(Rendered {
                text: require_text(&self.key, "update", (self.update)(&decoded, &self.value))?,
                snapshot: self.snapshot()?,
            })
        }))
    }
}

type PackedLoad =
    Arc<dyn Fn() -> BoxFuture<Result<Option<Box<dyn Loaded>>, ContextError>> + Send + Sync>;

#[derive(Clone)]
struct PackedSource {
    key: Key,
    load: PackedLoad,
}

/// Opaque carrier for composable system context sources.
#[derive(Clone, Default)]
pub struct SystemContext {
    sources: Vec<PackedSource>,
}

impl fmt::Debug for SystemContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list()
            .entries(self.sources.iter().map(|s| s.key.as_str()))
            .finish()
    }
}

impl SystemContext {
    /// The identity context.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Source keys in order.
    pub fn keys(&self) -> Vec<Key> {
        self.sources.iter().map(|s| s.key.clone()).collect()
    }
}

/// Closes a typed source into a context that composes with differently typed sources.
pub fn make<A>(source: Source<A>) -> SystemContext
where
    A: Serialize + DeserializeOwned + PartialEq + Send + Sync + 'static,
{
    let key = source.key.clone();
    let load: PackedLoad = Arc::new(move || {
        let fut = (source.load)();
        let (key, baseline, update, removed) = (
            source.key.clone(),
            source.baseline.clone(),
            source.update.clone(),
            source.removed.clone(),
        );
        Box::pin(async move {
            Ok(match fut.await? {
                Observation::Unavailable => None,
                Observation::Available(value) => Some(Box::new(LoadedValue {
                    key,
                    value,
                    baseline,
                    update,
                    removed,
                }) as Box<dyn Loaded>),
            })
        })
    });
    SystemContext {
        sources: vec![PackedSource { key, load }],
    }
}

/// Combines contexts in order and rejects duplicate source keys immediately.
pub fn combine(
    values: impl IntoIterator<Item = SystemContext>,
) -> Result<SystemContext, ContextError> {
    let sources: Vec<PackedSource> = values.into_iter().flat_map(|v| v.sources).collect();
    let mut seen = std::collections::HashSet::new();
    if let Some(dup) = sources.iter().find(|s| !seen.insert(s.key.clone())) {
        return Err(ContextError::DuplicateKey {
            key: dup.key.clone(),
        });
    }
    Ok(SystemContext { sources })
}

enum Entry {
    Available(Key, Box<dyn Loaded>),
    Unavailable(Key),
}

impl Entry {
    fn key(&self) -> &Key {
        match self {
            Entry::Available(key, _) | Entry::Unavailable(key) => key,
        }
    }
}

async fn observe(value: &SystemContext) -> Result<Vec<Entry>, ContextError> {
    let loads = value.sources.iter().map(|s| (s.load)());
    try_join_all(loads)
        .await?
        .into_iter()
        .zip(&value.sources)
        .map(|(result, s)| {
            Ok(match result {
                Some(loaded) => Entry::Available(s.key.clone(), loaded),
                None => Entry::Unavailable(s.key.clone()),
            })
        })
        .collect()
}

/// Creates the immutable baseline and durable snapshot for a new generation.
pub async fn initialize(value: &SystemContext) -> Result<Generation, ContextError> {
    let entries = observe(value).await?;
    let keys: Vec<Key> = entries
        .iter()
        .filter(|e| matches!(e, Entry::Unavailable(_)))
        .map(|e| e.key().clone())
        .collect();
    if !keys.is_empty() {
        return Err(ContextError::InitializationBlocked { keys });
    }
    initialize_observation(&entries)
}

fn initialize_observation(entries: &[Entry]) -> Result<Generation, ContextError> {
    let mut texts = Vec::new();
    let mut snapshot = Snapshot::new();
    for entry in entries {
        let Entry::Available(key, loaded) = entry else {
            continue;
        };
        let rendered = loaded.baseline()?;
        texts.push(rendered.text);
        snapshot.insert(key.clone(), rendered.snapshot);
    }
    Ok(Generation {
        baseline: texts.join("\n\n"),
        snapshot,
    })
}

/// Reconciles current source values with one active generation.
pub async fn reconcile(
    value: &SystemContext,
    previous: &Snapshot,
) -> Result<ReconcileResult, ContextError> {
    let entries = observe(value).await?;
    if let Some(result) = reconcile_observation(&entries, previous)? {
        return Ok(result);
    }
    Ok(match replace_observation(&entries, previous)? {
        ReplacementResult::ReplacementReady(g) => ReconcileResult::ReplacementReady(g),
        ReplacementResult::ReplacementBlocked => ReconcileResult::ReplacementBlocked,
    })
}

/// `None` means replacement is required.
fn reconcile_observation(
    entries: &[Entry],
    previous: &Snapshot,
) -> Result<Option<ReconcileResult>, ContextError> {
    let keys: std::collections::HashSet<&Key> = entries.iter().map(Entry::key).collect();
    let mut comparisons = Vec::with_capacity(entries.len());
    for entry in entries {
        let compared = match (entry, previous.get(entry.key())) {
            (Entry::Available(_, loaded), Some(stored)) => Some(loaded.compare(&stored.value)),
            _ => None,
        };
        if matches!(compared, Some(Compared::Incompatible)) {
            return Ok(None);
        }
        comparisons.push(compared);
    }
    let mut gone: Vec<&Key> = previous.keys().filter(|k| !keys.contains(k)).collect();
    gone.sort();
    if gone.iter().any(|k| previous[*k].removed.is_none()) {
        return Ok(None);
    }

    let mut snapshot = Snapshot::new();
    let mut updates = Vec::new();
    for (entry, compared) in entries.iter().zip(comparisons) {
        let stored = previous.get(entry.key());
        match (entry, stored, compared) {
            (Entry::Unavailable(key), Some(stored), _) => {
                snapshot.insert(key.clone(), stored.clone());
            }
            (Entry::Unavailable(_), None, _) => {}
            (Entry::Available(key, loaded), None, _) => {
                let rendered = loaded.baseline()?;
                updates.push(rendered.text);
                snapshot.insert(key.clone(), rendered.snapshot);
            }
            (Entry::Available(key, _), Some(stored), Some(Compared::Unchanged)) => {
                snapshot.insert(key.clone(), stored.clone());
            }
            (Entry::Available(key, _), Some(_), Some(Compared::Updated(render))) => {
                let rendered = render()?;
                updates.push(rendered.text);
                snapshot.insert(key.clone(), rendered.snapshot);
            }
            (Entry::Available(key, _), Some(_), _) => {
                unreachable!("missing comparison for system context source {key}")
            }
        }
    }
    updates.extend(gone.iter().filter_map(|k| previous[*k].removed.clone()));
    if updates.is_empty() {
        return Ok(Some(ReconcileResult::Unchanged));
    }
    Ok(Some(ReconcileResult::Updated {
        text: updates.join("\n\n"),
        snapshot,
    }))
}

/// Creates a complete replacement generation or blocks while admitted context is unavailable.
pub async fn replace(
    value: &SystemContext,
    previous: &Snapshot,
) -> Result<ReplacementResult, ContextError> {
    let entries = observe(value).await?;
    replace_observation(&entries, previous)
}

fn replace_observation(
    entries: &[Entry],
    previous: &Snapshot,
) -> Result<ReplacementResult, ContextError> {
    let blocked = entries
        .iter()
        .any(|e| matches!(e, Entry::Unavailable(key) if previous.contains_key(key)));
    if blocked {
        return Ok(ReplacementResult::ReplacementBlocked);
    }
    Ok(ReplacementResult::ReplacementReady(initialize_observation(
        entries,
    )?))
}

fn require_text(key: &Key, kind: &'static str, text: String) -> Result<String, ContextError> {
    if text.is_empty() {
        return Err(ContextError::EmptyRendering {
            key: key.clone(),
            kind,
        });
    }
    Ok(text)
}
