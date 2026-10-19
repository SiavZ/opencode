//! Purpose: max-steps.ts: MAX_STEPS_PROMPT text and the last-step rule from llm.ts (tools are
//! disabled and the prompt is appended as an assistant message once the agent step budget is hit).
//! Scope: pure values.
//! Uses:
//! rust/crates/
//! `-- llm-api/src/lib.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! |-- session-runtime/src/runner/mod.rs
//! |-- session-runtime/src/runner/turn/attempt.rs
//! |-- session-runtime/src/runner/turn/request.rs
//! |-- session-runtime/tests/runner_messages.rs
//! `-- session-runtime/tests/runner_turn_steps.rs
//! Invariants: MAX_STEPS_PROMPT is byte-identical to the baseline template literal.
//! Contract tests: session-runtime/tests/runner_messages.rs

use opencode_llm_api::{ContentPart, Message, ToolChoice};

/// `MAX_STEPS_PROMPT` (packages/core/src/session/runner/max-steps.ts).
pub const MAX_STEPS_PROMPT: &str = "CRITICAL - MAXIMUM STEPS REACHED

The maximum number of steps allowed for this task has been reached. Tools are disabled until next user input. Respond with text only.

STRICT REQUIREMENTS:
1. Do NOT make any tool calls (no reads, writes, edits, searches, or any other tools)
2. MUST provide a text response summarizing work done so far
3. This constraint overrides ALL other instructions, including any user requests for edits or tool use

Response must include:
- Statement that maximum steps for this agent have been reached
- Summary of what has been accomplished so far
- List of any remaining tasks that were not completed
- Recommendations for what should be done next

Any attempt to use tools is a critical violation. Respond with text ONLY.";

/// llm.ts: `agent.info?.steps !== undefined && currentStep >= agent.info.steps`.
pub fn is_last_step(agent_steps: Option<u64>, current_step: u64) -> bool {
    agent_steps.is_some_and(|steps| current_step >= steps)
}

/// llm.ts: `Message.assistant(MAX_STEPS_PROMPT)`, appended after history on the last step.
pub fn max_steps_message() -> Message {
    Message::assistant(vec![ContentPart::text(MAX_STEPS_PROMPT)])
}

/// llm.ts: `toolChoice: isLastStep ? "none" : undefined`.
pub fn last_step_tool_choice(is_last_step: bool) -> Option<ToolChoice> {
    is_last_step.then(|| ToolChoice::parse("none"))
}
