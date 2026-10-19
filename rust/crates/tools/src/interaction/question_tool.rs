//! Purpose: question builtin backed by the shared pending question domain.
//! Scope: TS core question tool permission, input, output and cancellation contract.
//! Uses: schema/question, tool-api, tools/interaction/questions.
//! Used by: runtime/location_tools.
//! Invariants: authorization precedes admission; cancellation drops pending ownership.
//! Contract tests: tools/tests/questions.rs.

use super::questions::Questions;
use async_trait::async_trait;
use opencode_schema::ids::SessionID;
use opencode_schema::question::{Info, Prompt};
use opencode_tool_api::{Tool, ToolContent, ToolContext, ToolFailure, ToolOutput};
use serde_json::{Value, json};
use std::sync::Arc;

pub struct QuestionTool(pub Arc<Questions>);

#[async_trait]
impl Tool for QuestionTool {
    fn description(&self) -> &str {
        include_str!("question.txt").trim_end()
    }

    fn input_schema(&self) -> Value {
        json!({"type":"object","properties":{"questions":{"type":"array","items":{"type":"object","properties":{"question":{"type":"string"},"header":{"type":"string"},"options":{"type":"array","items":{"type":"object","properties":{"label":{"type":"string"},"description":{"type":"string"}},"required":["label","description"]}},"multiple":{"type":"boolean"}},"required":["question","header","options"]}}},"required":["questions"]})
    }

    fn output_schema(&self) -> Option<Value> {
        Some(
            json!({"type":"object","properties":{"answers":{"type":"array","items":{"type":"array","items":{"type":"string"}}}},"required":["answers"]}),
        )
    }

    async fn execute(
        &self,
        input: Value,
        context: &ToolContext,
    ) -> Result<ToolOutput, ToolFailure> {
        run(&self.0, input, context, false).await
    }
}

pub struct LegacyQuestionTool(pub Arc<Questions>);

#[async_trait]
impl Tool for LegacyQuestionTool {
    fn description(&self) -> &str {
        include_str!("question.txt")
    }
    fn input_schema(&self) -> Value {
        QuestionTool(self.0.clone()).input_schema()
    }
    async fn execute(
        &self,
        input: Value,
        context: &ToolContext,
    ) -> Result<ToolOutput, ToolFailure> {
        run(&self.0, input, context, true).await
    }
}

async fn run(
    service: &Questions,
    input: Value,
    context: &ToolContext,
    legacy: bool,
) -> Result<ToolOutput, ToolFailure> {
    let questions: Vec<Prompt> = serde_json::from_value(
        input
            .get("questions")
            .cloned()
            .ok_or_else(|| ToolFailure::invalid_input("questions is required"))?,
    )
    .map_err(|error| ToolFailure::invalid_input(error.to_string()))?;
    if !legacy {
        context
            .authorize("question", vec!["*".into()], None, None)
            .await
            .map_err(|_| ToolFailure::new("Permission denied: question"))?;
    }
    let session_id =
        SessionID::new(&context.session_id).map_err(|error| ToolFailure::new(error.to_string()))?;
    let request = questions
        .iter()
        .map(|question| Info {
            question: question.question.clone(),
            header: question.header.clone(),
            options: question.options.clone(),
            multiple: question.multiple,
            custom: None,
        })
        .collect();
    let answers = tokio::select! {
        _ = context.cancel.cancelled() => return Err(ToolFailure::new("Question canceled")),
        answers = service.ask(session_id, request, Some(opencode_schema::question::Tool { message_id: context.assistant_message_id.clone(), call_id: context.tool_call_id.clone() })) => answers.map_err(|error| ToolFailure::new(error.to_string()))?,
    };
    let formatted = questions
        .iter()
        .enumerate()
        .map(|(index, question)| {
            format!(
                "\"{}\"=\"{}\"",
                question.question,
                answers
                    .get(index)
                    .filter(|answer| !answer.is_empty())
                    .map(|answer| answer.join(", "))
                    .unwrap_or_else(|| "Unanswered".into())
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    let text = format!(
        "User has answered your questions: {formatted}. You can now continue with the user's answers in mind."
    );
    let structured = if legacy {
        json!({"title":format!("Asked {} question{}", questions.len(), if questions.len() > 1 { "s" } else { "" }),"output":text,"metadata":{"answers":answers}})
    } else {
        json!({"answers":answers})
    };
    Ok(ToolOutput::new(structured, vec![ToolContent::text(text)]))
}
