//! Purpose: interactive builtins and shared question lifecycle.
//! Scope: domain service plus question Tool implementation.
//! Uses: schema, tool-api, futures.
//! Used by: runtime location composition and native adapters.
//! Invariants: all consumers share one Questions instance per Location.
//! Contract tests: tools/tests/questions.rs.

pub mod question_tool;
pub mod questions;
pub use question_tool::{LegacyQuestionTool, QuestionTool};
pub use questions::{QuestionError, QuestionEvents, Questions};
