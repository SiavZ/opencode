//! Purpose: captured-target action descriptions and the prompt submit/rollback state machine.
//! Port of the decision logic in `packages/app/src/components/prompt-input/submit.ts:58-208`
//! (`sendFollowupDraft`): slash-command detection, optimistic busy + optimistic user message,
//! cancellation by the `before` hook, and rollback (idle + remove optimistic + restore draft) on
//! failure. Describes what the host must do; it never executes I/O.
//! Scope: pure values consumed by the browser composer and the terminal prompt.
//! Uses:
//! rust/crates/
//! `-- view-model/src/identity.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! |-- view-model/src/lib.rs
//! `-- view-model/tests/actions.rs
//! Invariants: every effect names the captured `Target`, never "the visible session"; rollback
//! restores the draft captured at submit time; a command (`/name args`) is only recognised when
//! `name` is in the known command list and never creates an optimistic message.
//! Contract tests: view-model/tests/actions.rs

use crate::identity::Target;

/// Draft content captured when the user submitted.
#[derive(Clone, Debug, PartialEq)]
pub struct DraftSnapshot {
    pub text: String,
    pub agent: String,
    pub model_id: String,
    pub provider_id: String,
    pub variant: Option<String>,
    /// Opaque attachment identifiers (images/files) owned by the host.
    pub attachments: Vec<String>,
}

/// What the submission will send.
#[derive(Clone, Debug, PartialEq)]
pub enum Submission {
    Command { name: String, arguments: String },
    Prompt { message_id: String },
}

/// Host-visible effect, always scoped to the captured target.
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    SetBusy(Target),
    SetIdle(Target),
    AddOptimistic {
        target: Target,
        message_id: String,
    },
    RemoveOptimistic {
        target: Target,
        message_id: String,
    },
    ClearDraft(Target),
    RestoreDraft {
        target: Target,
        draft: DraftSnapshot,
    },
    Send {
        target: Target,
        submission: Submission,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Started,
    Sent,
    Cancelled,
    Failed,
}

/// One in-flight submission. Drive it with `begin`, then `before_resolved`, then `finished`.
#[derive(Clone, Debug)]
pub struct PromptSubmit {
    pub target: Target,
    pub draft: DraftSnapshot,
    pub submission: Submission,
    pub optimistic_busy: bool,
    pub phase: Phase,
}

/// Recognise `/name rest` when `name` is a known command (`input.sync.data.command`).
pub fn parse_command(text: &str, commands: &[&str]) -> Option<(String, String)> {
    let mut split = text.splitn(2, ' ');
    let head = split.next()?;
    let name = head.strip_prefix('/')?;
    if name.is_empty() || !commands.contains(&name) {
        return None;
    }
    // Baseline joins `tail` back with single spaces: identical to the remainder after one split.
    Some((name.to_owned(), split.next().unwrap_or("").to_owned()))
}

impl PromptSubmit {
    /// Start a submission. `message_id` is the pre-allocated ascending message id.
    pub fn begin(
        target: Target,
        draft: DraftSnapshot,
        commands: &[&str],
        message_id: String,
        optimistic_busy: bool,
    ) -> (Self, Vec<Effect>) {
        let submission = match parse_command(&draft.text, commands) {
            Some((name, arguments)) => Submission::Command { name, arguments },
            None => Submission::Prompt { message_id },
        };
        let mut effects = vec![];
        if optimistic_busy {
            effects.push(Effect::SetBusy(target.clone()));
        }
        if let Submission::Prompt { message_id } = &submission {
            effects.push(Effect::AddOptimistic {
                target: target.clone(),
                message_id: message_id.clone(),
            });
        }
        effects.push(Effect::ClearDraft(target.clone()));
        let this = Self {
            target,
            draft,
            submission,
            optimistic_busy,
            phase: Phase::Started,
        };
        (this, effects)
    }

    /// The `before` hook resolved. `false` cancels: undo optimistic state and restore the draft.
    pub fn before_resolved(&mut self, proceed: bool) -> Vec<Effect> {
        if self.phase != Phase::Started {
            return vec![];
        }
        if proceed {
            return vec![Effect::Send {
                target: self.target.clone(),
                submission: self.submission.clone(),
            }];
        }
        self.phase = Phase::Cancelled;
        self.rollback()
    }

    /// The send completed. Failures roll back to the captured target, not the visible one.
    pub fn finished(&mut self, ok: bool) -> Vec<Effect> {
        if self.phase != Phase::Started {
            return vec![];
        }
        if ok {
            self.phase = Phase::Sent;
            return vec![];
        }
        self.phase = Phase::Failed;
        self.rollback()
    }

    fn rollback(&self) -> Vec<Effect> {
        let mut effects = vec![];
        if self.optimistic_busy {
            effects.push(Effect::SetIdle(self.target.clone()));
        }
        if let Submission::Prompt { message_id } = &self.submission {
            effects.push(Effect::RemoveOptimistic {
                target: self.target.clone(),
                message_id: message_id.clone(),
            });
        }
        effects.push(Effect::RestoreDraft {
            target: self.target.clone(),
            draft: self.draft.clone(),
        });
        effects
    }
}
