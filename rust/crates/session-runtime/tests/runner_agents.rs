//! Purpose: port of packages/core/test/agent.test.ts (AgentV2 registry) plus the selection rules
//! of packages/core/src/agent.ts (`default`, `resolve`, `select`) and the replay semantics of
//! packages/core/test/state.test.ts that apply to the agent registry.
//! Scope: in-memory registry, no I/O. The baseline "does not ambiently opt built-in agents into
//! bash" case needs the AgentPlugin built-ins (not owned here) and is not ported.
//! Uses:
//! rust/crates/
//! |-- schema/src/agent.rs
//! `-- session-runtime/src/agents.rs
//! External:
//! `-- none
//! Used by:
//! `-- none, entrypoint
//! Invariants: assertions go through the public `Agents` API only.
//! Contract tests: this file

use std::sync::{Arc, Mutex};

use opencode_schema::agent::{ID, Info, Mode};
use opencode_session_runtime::agents::{Agents, Selection};

fn id(value: &str) -> ID {
    ID::new(value)
}

fn ids(agents: &[Info]) -> Vec<&str> {
    agents.iter().map(|agent| agent.id.as_str()).collect()
}

#[test]
fn starts_without_agents() {
    let agents = Agents::new();
    assert!(agents.all().is_empty());
    assert_eq!(agents.get(&id("build")), None);
}

#[test]
fn materializes_replayable_agent_transforms() {
    let agents = Agents::new();
    agents.transform(|editor| {
        editor.update(id("reviewer"), |info| {
            info.description = Some("Reviews code".into());
            info.mode = Mode::Subagent;
        })
    });
    let reviewer = agents.get(&id("reviewer")).unwrap();
    assert_eq!(reviewer.id, id("reviewer"));
    assert_eq!(reviewer.description.as_deref(), Some("Reviews code"));
    assert_eq!(reviewer.mode, Mode::Subagent);
    assert_eq!(ids(&agents.all()), ["reviewer"]);
}

#[test]
fn rebuilds_state_when_a_transform_is_replaced() {
    let agents = Agents::new();
    let input = Arc::new(Mutex::new(("Old description".to_string(), true)));
    let shared = input.clone();
    agents.transform(move |editor| {
        let (description, hidden) = shared.lock().unwrap().clone();
        editor.update(id("reviewer"), |info| {
            info.description = Some(description);
            info.hidden = hidden;
        })
    });
    assert!(agents.get(&id("reviewer")).unwrap().hidden);

    *input.lock().unwrap() = ("New description".into(), false);
    agents.reload();
    let reviewer = agents.get(&id("reviewer")).unwrap();
    assert_eq!(reviewer.description.as_deref(), Some("New description"));
    assert!(!reviewer.hidden);
}

#[test]
fn removes_a_transform_when_disposed() {
    let agents = Agents::new();
    let registration = agents.transform(|editor| editor.update(id("scoped"), |_| {}));
    assert!(agents.get(&id("scoped")).is_some());

    registration.dispose();
    assert_eq!(agents.get(&id("scoped")), None);
}

#[test]
fn disposes_once_and_rebuilds_remaining_state() {
    let agents = Agents::new();
    agents.transform(|editor| editor.update(id("first"), |_| {}));
    let second = agents.transform(|editor| editor.update(id("second"), |_| {}));
    assert_eq!(ids(&agents.all()), ["first", "second"]);

    second.dispose();
    assert_eq!(ids(&agents.all()), ["first"]);
    second.dispose();
    assert_eq!(ids(&agents.all()), ["first"]);
}

#[test]
fn applies_direct_agent_updates() {
    let agents = Agents::new();
    agents.transform(|editor| {
        editor.update(id("build"), |info| {
            info.mode = Mode::Primary;
            info.hidden = true;
        })
    });
    let build = agents.get(&id("build")).unwrap();
    assert_eq!(
        (build.id, build.mode, build.hidden),
        (id("build"), Mode::Primary, true)
    );
}

#[test]
fn creates_agents_with_runtime_defaults_and_supports_direct_removal() {
    let agents = Agents::new();
    agents.transform(|editor| editor.update(id("custom"), |_| {}));
    assert_eq!(agents.get(&id("custom")), Some(Info::empty(id("custom"))));

    agents.transform(|editor| editor.remove(&id("custom")));
    assert_eq!(agents.get(&id("custom")), None);
}

#[test]
fn update_cannot_rename_an_agent() {
    let agents = Agents::new();
    agents.transform(|editor| editor.update(id("fixed"), |info| info.id = id("renamed")));
    assert_eq!(ids(&agents.all()), ["fixed"]);
}

#[test]
fn later_transforms_see_and_edit_earlier_state_in_order() {
    let agents = Agents::new();
    agents.transform(|editor| {
        editor.update(id("a"), |info| info.system = Some("one".into()));
        editor.update(id("b"), |_| {});
    });
    agents.transform(|editor| {
        let seen = editor.list().len();
        assert_eq!(editor.get(&id("a")).unwrap().system.as_deref(), Some("one"));
        editor.update(id("a"), |info| info.description = Some(format!("{seen}")));
    });
    let a = agents.get(&id("a")).unwrap();
    assert_eq!(
        (a.system.as_deref(), a.description.as_deref()),
        (Some("one"), Some("2"))
    );
    assert_eq!(ids(&agents.all()), ["a", "b"]);
}

#[test]
fn removal_keeps_the_order_of_remaining_agents_and_readd_appends() {
    let agents = Agents::new();
    agents.transform(|editor| {
        ["a", "b", "c"]
            .into_iter()
            .for_each(|name| editor.update(id(name), |_| {}));
        editor.remove(&id("a"));
        editor.update(id("a"), |_| {});
    });
    assert_eq!(ids(&agents.all()), ["b", "c", "a"]);
}

#[test]
fn default_prefers_configured_selectable_agent() {
    let agents = Agents::new();
    agents.transform(|editor| {
        editor.update(id("build"), |info| info.mode = Mode::Primary);
        editor.update(id("reviewer"), |info| info.mode = Mode::Primary);
        editor.default(Some(id("reviewer")));
    });
    assert_eq!(agents.default().unwrap().id, id("reviewer"));
    assert_eq!(agents.resolve(None).unwrap().id, id("reviewer"));
    assert_eq!(agents.select(None).id, id("reviewer"));
}

#[test]
fn default_skips_unselectable_configured_agent_then_uses_build() {
    for (configured, hidden) in [("sub", false), ("ghost", true), ("missing", false)] {
        let agents = Agents::new();
        agents.transform(move |editor| {
            editor.update(id("first"), |_| {});
            editor.update(id("sub"), |info| info.mode = Mode::Subagent);
            editor.update(id("ghost"), |info| info.hidden = hidden);
            editor.update(id("build"), |_| {});
            editor.default(Some(id(configured)));
        });
        assert_eq!(agents.default().unwrap().id, id("build"), "{configured}");
    }
}

#[test]
fn default_falls_back_to_first_selectable_in_insertion_order() {
    let agents = Agents::new();
    agents.transform(|editor| {
        editor.update(id("sub"), |info| info.mode = Mode::Subagent);
        editor.update(id("build"), |info| info.hidden = true);
        editor.update(id("plan"), |info| info.mode = Mode::Primary);
        editor.update(id("other"), |_| {});
    });
    assert_eq!(agents.default().unwrap().id, id("plan"));
}

#[test]
fn select_without_selectable_agents_falls_back_to_build_id() {
    let agents = Agents::new();
    assert_eq!(
        agents.select(None),
        Selection {
            id: id("build"),
            info: None
        }
    );
    agents.transform(|editor| editor.update(id("sub"), |info| info.mode = Mode::Subagent));
    assert_eq!(agents.default(), None);
    assert_eq!(
        agents.select(None),
        Selection {
            id: id("build"),
            info: None
        }
    );
}

#[test]
fn explicit_ids_resolve_raw_even_when_unselectable_or_missing() {
    let agents = Agents::new();
    agents.transform(|editor| {
        editor.update(id("build"), |_| {});
        editor.update(id("sub"), |info| info.mode = Mode::Subagent);
        editor.update(id("ghost"), |info| info.hidden = true);
    });
    assert_eq!(agents.resolve(Some("sub")).unwrap().id, id("sub"));
    assert_eq!(agents.resolve(Some("ghost")).unwrap().id, id("ghost"));
    assert_eq!(agents.resolve(Some("missing")), None);

    let sub = agents.select(Some("sub"));
    assert_eq!(
        (sub.id, sub.info.map(|info| info.mode)),
        (id("sub"), Some(Mode::Subagent))
    );
    assert_eq!(
        agents.select(Some("missing")),
        Selection {
            id: id("missing"),
            info: None
        }
    );
}

#[test]
fn default_can_be_cleared_by_a_later_transform() {
    let agents = Agents::new();
    agents.transform(|editor| {
        editor.update(id("build"), |_| {});
        editor.update(id("reviewer"), |_| {});
        editor.default(Some(id("reviewer")));
    });
    let clear = agents.transform(|editor| editor.default(None));
    assert_eq!(agents.default().unwrap().id, id("build"));
    clear.dispose();
    assert_eq!(agents.default().unwrap().id, id("reviewer"));
}

#[test]
fn clones_share_state() {
    let agents = Agents::new();
    let other = agents.clone();
    agents.transform(|editor| editor.update(id("build"), |_| {}));
    assert_eq!(ids(&other.all()), ["build"]);
}
