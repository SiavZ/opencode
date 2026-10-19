//! Purpose: present actual shared question requests without a second pending registry.
//! Scope: one visible question form, including multiple answers, custom input and review.
//! Uses: runtime question facade, ui-terminal input and prompt.
//! Used by: terminal.rs.
//! Invariants: forms submit labels in question order; dismissals settle the real waiter.
//! Contract tests: tests/terminal_app question workflows.

use opencode_runtime::{Process, QuestionAnswer, QuestionRequest};
use opencode_ui_terminal::input::{UiEvent, UiKeyCode, UiKeyEventKind};
use opencode_ui_terminal::prompt::{Editor, EditorAction};
use std::path::Path;

pub struct Form {
    request: QuestionRequest,
    page: usize,
    row: usize,
    answers: Vec<QuestionAnswer>,
    custom: Option<Editor>,
}

pub enum Outcome {
    Pending,
    Reply(Vec<QuestionAnswer>),
    Reject,
}

impl Form {
    fn new(request: QuestionRequest) -> Self {
        Self {
            answers: vec![Vec::new(); request.questions.len()],
            request,
            page: 0,
            row: 0,
            custom: None,
        }
    }

    pub fn lines(&self) -> Vec<String> {
        let mut lines = vec!["Question required".into()];
        if self.page == self.request.questions.len() {
            lines.push("Review answers".into());
            lines.extend(self.request.questions.iter().zip(&self.answers).map(
                |(question, answers)| {
                    format!(
                        "{}: {}",
                        question.header,
                        if answers.is_empty() {
                            "Unanswered".into()
                        } else {
                            answers.join(", ")
                        }
                    )
                },
            ));
            lines.push("Enter submit | Left previous | Ctrl-D dismiss".into());
            return lines;
        }
        let question = &self.request.questions[self.page];
        lines.push(format!(
            "{} [{}/{}]",
            question.header,
            self.page + 1,
            self.request.questions.len()
        ));
        lines.push(question.question.clone());
        lines.extend(question.options.iter().enumerate().map(|(index, option)| {
            format!(
                "{} [{}] {}  {}",
                if index == self.row { ">" } else { " " },
                if self.answers[self.page].contains(&option.label) {
                    "x"
                } else {
                    " "
                },
                option.label,
                option.description
            )
        }));
        if question.custom != Some(false) {
            lines.push(format!(
                "{} Other: {}",
                if self.row == question.options.len() {
                    ">"
                } else {
                    " "
                },
                self.custom.as_ref().map(Editor::text).unwrap_or_default()
            ));
        }
        lines.push(
            "Up/Down choose | Space toggle | Enter next | Tab review | Ctrl-D dismiss".into(),
        );
        lines
    }

    pub fn handle(&mut self, event: &UiEvent, width: u16) -> Outcome {
        if let UiEvent::Key(key) = event {
            if key.kind == UiKeyEventKind::Release {
                return Outcome::Pending;
            }
            if key.code == UiKeyCode::Char('d') && key.modifiers.control {
                return Outcome::Reject;
            }
        }
        if let Some(editor) = &mut self.custom {
            if matches!(event, UiEvent::Key(key) if key.code == UiKeyCode::Esc) {
                self.custom = None;
                return Outcome::Pending;
            }
            if let EditorAction::Submit(answer) = editor.handle_at(event, width) {
                self.custom = None;
                if !answer.trim().is_empty() {
                    if self.request.questions[self.page].multiple != Some(true) {
                        self.answers[self.page].clear();
                    }
                    if !self.answers[self.page].contains(&answer) {
                        self.answers[self.page].push(answer);
                    }
                    return self.advance();
                }
            }
            return Outcome::Pending;
        }
        let UiEvent::Key(key) = event else {
            return Outcome::Pending;
        };
        match key.code {
            UiKeyCode::Esc => return Outcome::Reject,
            UiKeyCode::Left | UiKeyCode::BackTab => {
                self.page = self.page.saturating_sub(1);
                self.row = 0;
            }
            UiKeyCode::Right | UiKeyCode::Tab => {
                self.page = (self.page + 1).min(self.request.questions.len());
                self.row = 0;
            }
            UiKeyCode::Enter if self.page == self.request.questions.len() => {
                return Outcome::Reply(self.answers.clone());
            }
            _ if self.page == self.request.questions.len() => {}
            UiKeyCode::Up | UiKeyCode::Down => {
                let question = &self.request.questions[self.page];
                let rows = question.options.len() + usize::from(question.custom != Some(false));
                if rows > 0 {
                    self.row = if key.code == UiKeyCode::Up {
                        (self.row + rows - 1) % rows
                    } else {
                        (self.row + 1) % rows
                    };
                }
            }
            UiKeyCode::Enter | UiKeyCode::Char(' ') => {
                let question = &self.request.questions[self.page];
                if let Some(option) = question.options.get(self.row) {
                    let label = option.label.clone();
                    if question.multiple == Some(true) {
                        if self.answers[self.page].contains(&label) {
                            self.answers[self.page].retain(|answer| answer != &label);
                        } else {
                            self.answers[self.page].push(label);
                        }
                    } else {
                        self.answers[self.page] = vec![label];
                    }
                    if key.code == UiKeyCode::Enter {
                        return self.advance();
                    }
                } else if question.custom != Some(false) {
                    self.custom = Some(Editor::new());
                }
            }
            _ => {}
        }
        Outcome::Pending
    }

    fn advance(&mut self) -> Outcome {
        if self.request.questions.len() == 1 && self.request.questions[0].multiple != Some(true) {
            return Outcome::Reply(self.answers.clone());
        }
        self.page = (self.page + 1).min(self.request.questions.len());
        self.row = 0;
        Outcome::Pending
    }
}

pub async fn poll(
    process: &Process,
    form: &mut Option<Form>,
    directory: &Path,
    session: &str,
) -> anyhow::Result<()> {
    let requests = process.pending_questions(directory.to_path_buf()).await?;
    if form
        .as_ref()
        .is_some_and(|form| !requests.iter().any(|request| request.id == form.request.id))
    {
        *form = None;
    }
    if form.is_none() {
        *form = requests
            .into_iter()
            .find(|request| request.session_id.as_str() == session)
            .map(Form::new);
    }
    Ok(())
}

pub async fn handle(
    process: &Process,
    form: &mut Option<Form>,
    directory: &Path,
    event: &UiEvent,
    width: u16,
) -> anyhow::Result<()> {
    let Some(current) = form else {
        return Ok(());
    };
    match current.handle(event, width) {
        Outcome::Pending => return Ok(()),
        Outcome::Reply(answers) => {
            process
                .reply_question(
                    directory.to_path_buf(),
                    current.request.id.as_str(),
                    answers,
                )
                .await?
        }
        Outcome::Reject => {
            process
                .reject_question(directory.to_path_buf(), current.request.id.as_str())
                .await?
        }
    }
    *form = None;
    Ok(())
}
