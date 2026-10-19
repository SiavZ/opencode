//! Purpose: turn/row grouping shared by browser and terminal timelines. Port of
//! `packages/app/src/pages/session/timeline/{rows.ts:35-242,timeline-row.ts,summary-diffs.ts,model.ts:95-129}`
//! and `packages/session-ui/src/components/message-part.tsx:607-720` (`groupParts`, `renderable`).
//! Scope: pure functions from projection to keyed rows. Row identity reuse lives in `reconcile.rs`.
//! Uses:
//! rust/crates/
//! |-- view-model/src/projection/mod.rs
//! |-- view-model/src/source.rs
//! |-- view-model/src/timeline/reconcile.rs
//! |-- view-model/src/timeline/row.rs
//! `-- view-model/src/timeline/text.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! |-- view-model/src/lib.rs
//! |-- view-model/src/session_cache.rs
//! |-- view-model/tests/session_cache.rs
//! `-- view-model/tests/timeline.rs
//! Invariants: row keys match `TimelineRow.key` byte for byte; context tools read/glob/grep/list
//! collapse into one `context:${firstPartID}` group; `todowrite` is hidden; running/pending
//! `question` tools are hidden; the thinking row only shows for the active busy turn without error.
//! Contract tests: view-model/tests/timeline.rs

pub mod reconcile;
pub mod row;
pub mod text;

use std::collections::{HashMap, HashSet};

use crate::projection::{
    PartKind, Projection, RenderMessage, RenderPart, Role, SummaryDiff, ToolStatus, message_key,
};
use crate::source::SessionMessage;

pub use reconcile::reuse_rows;
pub use row::{DividerLabel, PartGroup, PartRef, TimelineRow};

pub const CONTEXT_GROUP_TOOLS: [&str; 4] = ["read", "glob", "grep", "list"];
pub const HIDDEN_TOOLS: [&str; 1] = ["todowrite"];

/// Session activity as seen by the timeline (`SessionStatus["type"]`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionStatus {
    Idle,
    Busy,
    Retry,
}

#[derive(Clone, Debug)]
pub struct RowOptions {
    pub show_reasoning: bool,
    pub status: SessionStatus,
    pub inline_comments: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rows {
    pub active_message_id: Option<String>,
    pub rows: Vec<TimelineRow>,
}

/// Should this part produce a row (`renderable` in session-ui).
pub fn renderable(part: &RenderPart, show_reasoning: bool) -> bool {
    match &part.kind {
        PartKind::Tool { tool, status, .. } => {
            if HIDDEN_TOOLS.contains(&tool.as_str()) {
                return false;
            }
            if tool == "question" {
                return !matches!(status, ToolStatus::Pending { .. } | ToolStatus::Running);
            }
            true
        }
        PartKind::Text { text, .. } => !text.trim().is_empty(),
        PartKind::Reasoning { text, .. } => show_reasoning && !text.trim().is_empty(),
        PartKind::Compaction { .. } => true,
        PartKind::File { .. } | PartKind::Agent { .. } => false,
    }
}

fn is_context_tool(part: &RenderPart) -> bool {
    matches!(&part.kind, PartKind::Tool { tool, .. } if CONTEXT_GROUP_TOOLS.contains(&tool.as_str()))
}

/// Collapse adjacent context tools into one group; everything else is its own group.
pub fn group_parts(parts: &[(&str, &RenderPart)]) -> Vec<PartGroup> {
    let mut out = vec![];
    let mut run: Vec<PartRef> = vec![];
    let flush = |run: &mut Vec<PartRef>, out: &mut Vec<PartGroup>| {
        if let Some(first) = run.first() {
            out.push(PartGroup::Context {
                key: format!("context:{}", first.part_id),
                refs: std::mem::take(run),
            });
        }
    };
    for (message_id, part) in parts {
        let r = PartRef {
            message_id: (*message_id).to_owned(),
            part_id: part.id.clone(),
        };
        if is_context_tool(part) {
            run.push(r);
            continue;
        }
        flush(&mut run, &mut out);
        out.push(PartGroup::Part {
            key: format!("part:{}:{}", message_id, part.id),
            part: r,
        });
    }
    flush(&mut run, &mut out);
    out
}

/// Keep the last diff per file, preserving the order of last occurrences.
pub fn unique_summary_diffs(diffs: &[SummaryDiff]) -> Vec<SummaryDiff> {
    let mut seen = HashSet::new();
    let mut out: Vec<SummaryDiff> = diffs
        .iter()
        .rev()
        .filter(|d| seen.insert(d.file.clone()))
        .cloned()
        .collect();
    out.reverse();
    out
}

/// User roots in order; `revert_message_id` hides it and everything after (`selectVisibleUserMessages`).
pub fn visible_user_messages<'a>(
    messages: &'a [RenderMessage],
    revert_message_id: Option<&str>,
) -> Vec<&'a RenderMessage> {
    let users: Vec<_> = messages.iter().filter(|m| m.is_user()).collect();
    let Some(revert) = revert_message_id else {
        return users;
    };
    match users.iter().position(|m| m.id == revert) {
        Some(i) => users[..i].to_vec(),
        None => users,
    }
}

/// `isTimelineReady`: messages loaded and either a user root exists or history is not loading.
pub fn is_timeline_ready(messages: Option<&[RenderMessage]>, loading: bool) -> bool {
    messages.is_some_and(|m| m.iter().any(RenderMessage::is_user) || !loading)
}

struct Turn<'a> {
    user: &'a RenderMessage,
    assistants: Vec<&'a RenderMessage>,
}

/// `Timeline.constructSessionMessageRows`: group the source page into turns, add projected users
/// missing from the page (optimistic or older-page parents) and emit rows per turn.
pub fn construct_rows(
    source: &[SessionMessage],
    projection: &Projection,
    projected_users: &[RenderMessage],
    options: &RowOptions,
) -> Rows {
    let lookup = |id: &str| {
        projection
            .message(id)
            .or_else(|| projected_users.iter().find(|m| m.id == id))
    };
    let mut turns: Vec<Turn> = vec![];
    let mut by_user: HashMap<String, usize> = HashMap::new();
    for message in source {
        let id = message.id();
        let projected = lookup(id);
        if let (SessionMessage::Shell { .. }, Some(user)) =
            (message, projected.filter(|p| p.is_user()))
        {
            let assistant = lookup(&format!("{id}:assistant")).filter(|m| !m.is_user());
            by_user.insert(user.id.clone(), turns.len());
            turns.push(Turn {
                user,
                assistants: assistant.into_iter().collect(),
            });
            continue;
        }
        let Some(projected) = projected else { continue };
        let Some(parent) = projected.parent_id() else {
            if !by_user.contains_key(&projected.id) {
                by_user.insert(projected.id.clone(), turns.len());
                turns.push(Turn {
                    user: projected,
                    assistants: vec![],
                });
            }
            continue;
        };
        if let Some(&i) = by_user.get(parent) {
            turns[i].assistants.push(projected);
            continue;
        }
        let Some(user) = lookup(parent).filter(|u| u.is_user()) else {
            continue;
        };
        by_user.insert(user.id.clone(), turns.len());
        turns.push(Turn {
            user,
            assistants: vec![projected],
        });
    }
    for user in projected_users {
        if by_user.contains_key(&user.id) {
            continue;
        }
        let key = message_key(user.time.created, &user.id);
        let index = turns
            .iter()
            .position(|t| key < message_key(t.user.time.created, &t.user.id));
        let turn = Turn {
            user,
            assistants: vec![],
        };
        match index {
            Some(i) => turns.insert(i, turn),
            None => turns.push(turn),
        }
        by_user = turns
            .iter()
            .enumerate()
            .map(|(i, t)| (t.user.id.clone(), i))
            .collect();
    }
    let active = turns.last().map(|t| t.user.id.clone());
    let rows = turns
        .iter()
        .enumerate()
        .flat_map(|(i, t)| {
            message_rows(
                t,
                projection,
                i,
                options,
                active.as_deref() == Some(&t.user.id),
            )
        })
        .collect();
    Rows {
        active_message_id: active,
        rows,
    }
}

fn assistant_error(m: &RenderMessage) -> Option<&crate::projection::RenderError> {
    match &m.role {
        Role::Assistant { error, .. } => error.as_ref(),
        Role::User => None,
    }
}

fn message_rows(
    turn: &Turn,
    projection: &Projection,
    index: usize,
    o: &RowOptions,
    active: bool,
) -> Vec<TimelineRow> {
    let uid = turn.user.id.clone();
    let user_parts = projection.parts_of(&uid);
    let comments = user_parts
        .iter()
        .filter(|p| matches!(&p.kind, PartKind::Text { text, synthetic: true } if text::parse_comment_note(text).is_some()))
        .count();
    let compaction = user_parts
        .iter()
        .any(|p| matches!(p.kind, PartKind::Compaction { .. }));
    let interrupted_at = turn
        .assistants
        .iter()
        .position(|m| assistant_error(m).is_some_and(|e| e.aborted));
    let error = turn
        .assistants
        .last()
        .and_then(|m| assistant_error(m))
        .filter(|e| !e.aborted);

    let refs: Vec<(usize, &str, &RenderPart)> = turn
        .assistants
        .iter()
        .enumerate()
        .flat_map(|(mi, m)| {
            projection
                .parts_of(&m.id)
                .iter()
                .filter(|p| renderable(p, o.show_reasoning))
                .map(move |p| (mi, m.id.as_str(), p))
        })
        .collect();
    let groups = |pred: &dyn Fn(usize) -> bool| -> Vec<PartGroup> {
        let selected: Vec<(&str, &RenderPart)> = refs
            .iter()
            .filter(|r| pred(r.0))
            .map(|r| (r.1, r.2))
            .collect();
        group_parts(&selected)
    };
    // `None` stands for the interrupted divider.
    let items: Vec<Option<PartGroup>> = match interrupted_at {
        Some(at) if !compaction => {
            let mut v: Vec<_> = groups(&|i| i <= at).into_iter().map(Some).collect();
            v.push(None);
            v.extend(groups(&|i| i > at).into_iter().map(Some));
            v
        }
        _ => groups(&|_| true).into_iter().map(Some).collect(),
    };

    let mut rows = vec![];
    if index > 0 {
        rows.push(TimelineRow::TurnGap {
            user_message_id: uid.clone(),
        });
    }
    if comments > 0 && !o.inline_comments {
        rows.push(TimelineRow::CommentStrip {
            user_message_id: uid.clone(),
        });
    }
    rows.push(TimelineRow::UserMessage {
        user_message_id: uid.clone(),
        anchor: o.inline_comments || comments == 0,
    });
    if compaction {
        rows.push(TimelineRow::TurnDivider {
            user_message_id: uid.clone(),
            label: DividerLabel::Compaction,
        });
    }
    let mut group_index = 0;
    for item in items {
        let Some(group) = item else {
            rows.push(TimelineRow::TurnDivider {
                user_message_id: uid.clone(),
                label: DividerLabel::Interrupted,
            });
            continue;
        };
        rows.push(TimelineRow::AssistantPart {
            user_message_id: uid.clone(),
            group,
            previous_assistant_part: group_index > 0,
        });
        group_index += 1;
    }
    if active
        && o.status == SessionStatus::Busy
        && error.is_none()
        && (!o.show_reasoning || refs.is_empty())
    {
        let heading = turn
            .assistants
            .iter()
            .flat_map(|m| projection.parts_of(&m.id))
            .find_map(|p| match &p.kind {
                PartKind::Reasoning { text, .. } if !text.is_empty() => {
                    text::reasoning_heading(text)
                }
                _ => None,
            });
        rows.push(TimelineRow::Thinking {
            user_message_id: uid.clone(),
            reasoning_heading: heading,
        });
    }
    if active && o.status == SessionStatus::Retry {
        rows.push(TimelineRow::Retry {
            user_message_id: uid.clone(),
        });
    }
    let diffs = unique_summary_diffs(&turn.user.summary_diffs);
    if !diffs.is_empty() && (o.status == SessionStatus::Idle || !active) {
        rows.push(TimelineRow::DiffSummary {
            user_message_id: uid.clone(),
            diffs,
        });
    }
    if let Some(e) = error {
        rows.push(TimelineRow::Error {
            user_message_id: uid,
            text: text::unwrap_error_message(&e.message),
        });
    }
    rows
}
