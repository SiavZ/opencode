//! Purpose: stabilize timeline row identity across recomputations. Port of
//! `packages/app/src/pages/session/timeline/row-reconciliation.ts` (`reuseTimelineRows`):
//! a context group that grows or loses its first part keeps the key it had before so the
//! renderer does not remount it.
//! Scope: pure function over previous and next row lists.
//! Uses:
//! rust/crates/
//! `-- view-model/src/timeline/row.rs
//! External:
//! `-- str::to_owned
//! Used by:
//! rust/crates/
//! `-- view-model/src/timeline/mod.rs
//! Invariants: each previous context key is claimed by at most one next row; a next row whose own
//! key already existed reserves it; `unchanged` is true only when the result equals `previous`.
//! Contract tests: view-model/tests/timeline.rs

use std::collections::{HashMap, HashSet};

use super::row::{PartGroup, TimelineRow};

#[derive(Clone, Debug, PartialEq)]
pub struct Reused {
    pub rows: Vec<TimelineRow>,
    /// True when the result is identical to `previous` (renderer may skip work).
    pub unchanged: bool,
}

pub fn reuse_rows(previous: &[TimelineRow], rows: Vec<TimelineRow>) -> Reused {
    if previous.is_empty() {
        return Reused {
            rows,
            unchanged: false,
        };
    }
    let prev_keys: HashSet<String> = previous.iter().map(TimelineRow::key).collect();
    // `${user}:${partID}` -> (index, key) of the previous context row containing that part.
    let mut context_by_part: HashMap<String, (usize, String)> = HashMap::new();
    for (index, row) in previous.iter().enumerate() {
        if let TimelineRow::AssistantPart {
            user_message_id,
            group: PartGroup::Context { refs, .. },
            ..
        } = row
        {
            for r in refs {
                context_by_part.insert(
                    format!("{user_message_id}:{}", r.part_id),
                    (index, row.key()),
                );
            }
        }
    }
    let mut reserved: HashMap<String, usize> = HashMap::new();
    for (index, row) in rows.iter().enumerate() {
        if is_context(row) {
            let key = row.key();
            if prev_keys.contains(&key) {
                reserved.entry(key).or_insert(index);
            }
        }
    }
    let mut claimed: HashSet<String> = HashSet::new();
    let next: Vec<TimelineRow> = rows
        .into_iter()
        .enumerate()
        .map(|(index, row)| stabilize(&context_by_part, &reserved, row, index, &mut claimed))
        .collect();
    let unchanged = next.as_slice() == previous;
    Reused {
        rows: next,
        unchanged,
    }
}

fn is_context(row: &TimelineRow) -> bool {
    matches!(
        row,
        TimelineRow::AssistantPart {
            group: PartGroup::Context { .. },
            ..
        }
    )
}

fn stabilize(
    context_by_part: &HashMap<String, (usize, String)>,
    reserved: &HashMap<String, usize>,
    row: TimelineRow,
    index: usize,
    claimed: &mut HashSet<String>,
) -> TimelineRow {
    let TimelineRow::AssistantPart {
        user_message_id,
        group: PartGroup::Context { key, refs },
        previous_assistant_part,
    } = row
    else {
        return row;
    };
    let mut best: Option<&(usize, String)> = None;
    for r in &refs {
        let Some(candidate) = context_by_part.get(&format!("{user_message_id}:{}", r.part_id))
        else {
            continue;
        };
        if claimed.contains(&candidate.1) {
            continue;
        }
        if reserved
            .get(&candidate.1)
            .is_some_and(|owner| *owner != index)
        {
            continue;
        }
        if best.is_none_or(|b| candidate.0 < b.0) {
            best = Some(candidate);
        }
    }
    let key = match best {
        Some((_, row_key)) => {
            claimed.insert(row_key.clone());
            // Row key is `assistant-part:${user}:${groupKey}`; recover the group key.
            let prefix = format!("assistant-part:{user_message_id}:");
            row_key
                .strip_prefix(&prefix)
                .map(str::to_owned)
                .unwrap_or(key)
        }
        None => key,
    };
    TimelineRow::AssistantPart {
        user_message_id,
        group: PartGroup::Context { key, refs },
        previous_assistant_part,
    }
}
