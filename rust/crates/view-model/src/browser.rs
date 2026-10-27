//! Purpose: target-owned browser drafts and exact keyboard submission decisions.
//! Scope: portable browser interaction state, without DOM or transport.
//! Uses:
//! `-- none
//! External:
//! |-- serde::Deserialize
//! `-- serde::Serialize
//! Used by:
//! `-- none
//! Invariants: late failures restore only their captured target and never overwrite a newer draft.
//! Contract tests: view-model/tests/browser.rs

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Attachment {
    pub uri: String,
    pub name: String,
    pub mime: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Draft {
    pub text: String,
    pub attachments: Vec<Attachment>,
}

impl Draft {
    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty() && self.attachments.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Submission {
    pub target: String,
    pub draft: Draft,
    revision: u64,
}

#[derive(Debug, Default)]
pub struct DraftBook {
    drafts: HashMap<String, Draft>,
    revisions: HashMap<String, u64>,
    pending: HashMap<String, u64>,
}

impl DraftBook {
    pub fn get(&self, target: &str) -> Draft {
        self.drafts.get(target).cloned().unwrap_or_default()
    }

    pub fn edit(&mut self, target: &str, draft: Draft) {
        *self.revisions.entry(target.into()).or_default() += 1;
        self.drafts.insert(target.into(), draft);
    }

    pub fn busy(&self, target: &str) -> bool {
        self.pending.contains_key(target)
    }

    pub fn begin(&mut self, target: &str) -> Option<Submission> {
        let draft = self.get(target);
        if draft.is_empty() || self.busy(target) {
            return None;
        }
        let revision = *self.revisions.entry(target.into()).or_default();
        self.pending.insert(target.into(), revision);
        self.drafts.remove(target);
        Some(Submission {
            target: target.into(),
            draft,
            revision,
        })
    }

    pub fn finish(&mut self, submission: &Submission, success: bool) -> bool {
        if self.pending.get(&submission.target) != Some(&submission.revision) {
            return false;
        }
        self.pending.remove(&submission.target);
        if success || self.revisions.get(&submission.target) != Some(&submission.revision) {
            return false;
        }
        self.drafts
            .insert(submission.target.clone(), submission.draft.clone());
        true
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnterAction {
    Newline,
    Compose,
    Submit,
    IgnoreRepeat,
}

pub fn enter_action(shift: bool, composing: bool, key_code: u32, repeat: bool) -> EnterAction {
    if shift {
        return EnterAction::Newline;
    }
    if composing || key_code == 229 {
        return EnterAction::Compose;
    }
    if repeat {
        return EnterAction::IgnoreRepeat;
    }
    EnterAction::Submit
}
