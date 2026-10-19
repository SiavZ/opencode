//! Purpose: `AgentV2` registry port of packages/core/src/agent.ts plus the replayable transform
//! state of packages/core/src/state.ts: agents are materialized by replaying registered transforms
//! onto an empty draft, and the default agent is selected from that materialized state.
//! Scope: Location-scoped in-memory registry, synchronous, thread-safe.
//! Uses:
//! rust/crates/
//! |-- schema/src/agent.rs
//! `-- session-api/src/ids.rs
//! External:
//! `-- indexmap::IndexMap
//! Used by:
//! rust/crates/
//! |-- runtime/src/location_agents.rs
//! |-- runtime/src/location_tools.rs
//! |-- session-runtime/src/lib.rs
//! |-- session-runtime/src/runner/ports.rs
//! |-- session-runtime/src/runner/turn.rs
//! |-- session-runtime/tests/runner_agents.rs
//! `-- session-runtime/tests/support/provider.rs
//! Invariants: agents keep insertion order (removal keeps the order of the rest), `update` creates
//! `Info::empty(id)` when missing and forces `info.id = id` after the callback, a reload replays
//! every active transform in registration order onto a fresh state and commits atomically,
//! disposing a registration is idempotent. Default selection: configured selectable, then `build`
//! selectable, then the first selectable in insertion order (selectable = not subagent, not hidden).
//! Contract tests: session-runtime/tests/runner_agents.rs

use std::sync::{Arc, Mutex, PoisonError, RwLock};

use indexmap::IndexMap;
use opencode_schema::agent::{ID, Info, Mode};
use opencode_session_api::ids::default_agent_id;

/// `AgentV2.Selection`: the requested or default agent ID plus its info when it exists.
#[derive(Debug, Clone, PartialEq)]
pub struct Selection {
    pub id: ID,
    pub info: Option<Info>,
}

/// Materialized registry state (`Data` in the baseline).
#[derive(Debug, Clone, Default)]
struct Data {
    agents: IndexMap<ID, Info>,
    default: Option<ID>,
}

/// `AgentV2.Draft`: the mutable view a transform edits during a reload.
pub struct Draft<'a> {
    data: &'a mut Data,
}

impl Draft<'_> {
    pub fn list(&self) -> Vec<Info> {
        self.data.agents.values().cloned().collect()
    }

    pub fn get(&self, id: &ID) -> Option<&Info> {
        self.data.agents.get(id)
    }

    pub fn default(&mut self, id: Option<ID>) {
        self.data.default = id;
    }

    /// Edits the agent, creating `Info::empty(id)` first when it does not exist. The callback
    /// cannot rename the agent: `id` is restored afterwards.
    pub fn update(&mut self, id: ID, f: impl FnOnce(&mut Info)) {
        let current = self
            .data
            .agents
            .entry(id.clone())
            .or_insert_with(|| Info::empty(id.clone()));
        f(current);
        current.id = id;
    }

    pub fn remove(&mut self, id: &ID) {
        self.data.agents.shift_remove(id);
    }
}

type TransformFn = dyn Fn(&mut Draft<'_>) + Send + Sync;

struct Entry {
    key: u64,
    run: Arc<TransformFn>,
}

#[derive(Default)]
struct Transforms {
    next_key: u64,
    active: Vec<Entry>,
}

#[derive(Default)]
struct Inner {
    // Holding this lock serializes materialization like the baseline semaphore.
    transforms: Mutex<Transforms>,
    state: RwLock<Arc<Data>>,
}

impl Inner {
    fn materialize(&self, transforms: &Transforms) {
        let mut next = Data::default();
        let mut draft = Draft { data: &mut next };
        transforms
            .active
            .iter()
            .for_each(|entry| (entry.run)(&mut draft));
        *self.state.write().unwrap_or_else(PoisonError::into_inner) = Arc::new(next);
    }

    fn snapshot(&self) -> Arc<Data> {
        self.state
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// Handle returned by [`Agents::transform`]. Dropping it keeps the transform active (the
/// baseline removes it when the owning Scope closes, which maps to [`Registration::dispose`]).
#[derive(Clone)]
pub struct Registration {
    inner: Arc<Inner>,
    key: u64,
}

impl Registration {
    /// Removes the transform and rebuilds the remaining state. Calling it again is a no-op.
    pub fn dispose(&self) {
        let mut transforms = self
            .inner
            .transforms
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let before = transforms.active.len();
        transforms.active.retain(|entry| entry.key != self.key);
        if transforms.active.len() == before {
            return;
        }
        self.inner.materialize(&transforms);
    }
}

/// `AgentV2.Service`. Cheap to clone, clones share state.
///
/// Transforms run synchronously while the registry lock is held, so a transform must not call
/// back into the same `Agents`.
#[derive(Clone, Default)]
pub struct Agents {
    inner: Arc<Inner>,
}

impl Agents {
    pub fn new() -> Self {
        Self {
            inner: Arc::default(),
        }
    }

    /// Registers a replayable transform and immediately reloads.
    pub fn transform(
        &self,
        transform: impl Fn(&mut Draft<'_>) + Send + Sync + 'static,
    ) -> Registration {
        let mut transforms = self
            .inner
            .transforms
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let key = transforms.next_key;
        transforms.next_key += 1;
        transforms.active.push(Entry {
            key,
            run: Arc::new(transform),
        });
        self.inner.materialize(&transforms);
        Registration {
            inner: self.inner.clone(),
            key,
        }
    }

    /// Replays every active transform onto a fresh state.
    pub fn reload(&self) {
        let transforms = self
            .inner
            .transforms
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        self.inner.materialize(&transforms);
    }

    pub fn get(&self, id: &ID) -> Option<Info> {
        self.inner.snapshot().agents.get(id).cloned()
    }

    pub fn default(&self) -> Option<Info> {
        selected_default(&self.inner.snapshot()).cloned()
    }

    /// Explicit IDs are looked up raw (hidden and subagent agents included), otherwise the default.
    pub fn resolve(&self, id: Option<&str>) -> Option<Info> {
        let data = self.inner.snapshot();
        match id {
            Some(id) => data.agents.get(&ID::new(id)).cloned(),
            None => selected_default(&data).cloned(),
        }
    }

    /// Explicit IDs keep the requested ID even when the agent is missing. Without one the default
    /// is selected, falling back to the `build` ID when no agent is selectable.
    pub fn select(&self, id: Option<&str>) -> Selection {
        let data = self.inner.snapshot();
        if let Some(id) = id {
            let id = ID::new(id);
            let info = data.agents.get(&id).cloned();
            return Selection { id, info };
        }
        let info = selected_default(&data).cloned();
        Selection {
            id: info
                .as_ref()
                .map_or_else(default_agent_id, |info| info.id.clone()),
            info,
        }
    }

    pub fn all(&self) -> Vec<Info> {
        self.inner.snapshot().agents.values().cloned().collect()
    }
}

fn selectable(agent: Option<&Info>) -> Option<&Info> {
    agent.filter(|agent| agent.mode != Mode::Subagent && !agent.hidden)
}

fn selected_default(data: &Data) -> Option<&Info> {
    data.default
        .as_ref()
        .and_then(|id| selectable(data.agents.get(id)))
        .or_else(|| selectable(data.agents.get(&default_agent_id())))
        .or_else(|| {
            data.agents
                .values()
                .find(|agent| selectable(Some(agent)).is_some())
        })
}
