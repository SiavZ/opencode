//! Purpose: plugin hook contract: the legacy `Hooks` trigger names from packages/plugin, their
//! JSON input/output envelopes, Rust-owned ordering policy, custom tool registrations and the
//! `PluginInstance` trait that the compatibility host and Rust-native plugins implement.
//! Scope: values, the hook-ordering function and async traits. No process code.
//! Uses:
//! rust/crates/
//! `-- integration-api/src/error.rs
//! External:
//! |-- async_trait::async_trait
//! |-- serde::{Deserialize, Serialize}
//! `-- serde_json::{Map, Value}
//! Used by:
//! rust/crates/
//! `-- integration-api/src/lib.rs
//! Invariants: `trigger` visits hooks in plugin load order, sequentially, each hook receives the
//! output mutated by the previous one (packages/opencode/src/plugin/index.ts:284-297). A hook that
//! errors aborts the trigger with the error (the baseline Effect.promise defect propagates).
//! Contract tests: integration-api/src/plugin.rs (tests module)

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::IntegrationError;

/// Every `(input, output) => Promise<void>` hook in `Hooks` (packages/plugin/src/index.ts:222-334).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TriggerName {
    #[serde(rename = "chat.message")]
    ChatMessage,
    #[serde(rename = "chat.params")]
    ChatParams,
    #[serde(rename = "chat.headers")]
    ChatHeaders,
    /// Declared but no baseline trigger site exists. Never treated as a policy override.
    #[serde(rename = "permission.ask")]
    PermissionAsk,
    #[serde(rename = "command.execute.before")]
    CommandExecuteBefore,
    #[serde(rename = "tool.execute.before")]
    ToolExecuteBefore,
    #[serde(rename = "shell.env")]
    ShellEnv,
    #[serde(rename = "tool.execute.after")]
    ToolExecuteAfter,
    #[serde(rename = "experimental.chat.messages.transform")]
    ChatMessagesTransform,
    #[serde(rename = "experimental.chat.system.transform")]
    ChatSystemTransform,
    #[serde(rename = "experimental.provider.small_model")]
    ProviderSmallModel,
    #[serde(rename = "experimental.session.compacting")]
    SessionCompacting,
    #[serde(rename = "experimental.compaction.autocontinue")]
    CompactionAutocontinue,
    #[serde(rename = "experimental.text.complete")]
    TextComplete,
    #[serde(rename = "tool.definition")]
    ToolDefinition,
}

impl TriggerName {
    pub const ALL: [TriggerName; 15] = [
        Self::ChatMessage,
        Self::ChatParams,
        Self::ChatHeaders,
        Self::PermissionAsk,
        Self::CommandExecuteBefore,
        Self::ToolExecuteBefore,
        Self::ShellEnv,
        Self::ToolExecuteAfter,
        Self::ChatMessagesTransform,
        Self::ChatSystemTransform,
        Self::ProviderSmallModel,
        Self::SessionCompacting,
        Self::CompactionAutocontinue,
        Self::TextComplete,
        Self::ToolDefinition,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::ChatMessage => "chat.message",
            Self::ChatParams => "chat.params",
            Self::ChatHeaders => "chat.headers",
            Self::PermissionAsk => "permission.ask",
            Self::CommandExecuteBefore => "command.execute.before",
            Self::ToolExecuteBefore => "tool.execute.before",
            Self::ShellEnv => "shell.env",
            Self::ToolExecuteAfter => "tool.execute.after",
            Self::ChatMessagesTransform => "experimental.chat.messages.transform",
            Self::ChatSystemTransform => "experimental.chat.system.transform",
            Self::ProviderSmallModel => "experimental.provider.small_model",
            Self::SessionCompacting => "experimental.session.compacting",
            Self::CompactionAutocontinue => "experimental.compaction.autocontinue",
            Self::TextComplete => "experimental.text.complete",
            Self::ToolDefinition => "tool.definition",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.as_str() == name)
    }
}

/// Non-trigger hook members a plugin may expose.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookCapabilities {
    /// Trigger hooks implemented by this plugin, in declaration order.
    pub triggers: Vec<TriggerName>,
    pub event: bool,
    pub config: bool,
    pub dispose: bool,
    /// Custom tool ids from `hooks.tool`.
    pub tools: Vec<String>,
    /// `auth.provider` when an auth hook exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_provider: Option<String>,
    /// `provider.id` when a provider hook exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
}

/// A registered custom tool (`ToolDefinition` from packages/plugin/src/tool.ts) with its JSON
/// Schema already derived from the zod args by the host.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CustomTool {
    pub id: String,
    pub description: String,
    pub parameters: Value,
}

/// `ToolContext` passed to a custom tool execution, minus callbacks (bridged as host RPC).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolContext {
    #[serde(rename = "sessionID")]
    pub session_id: String,
    #[serde(rename = "messageID")]
    pub message_id: String,
    pub agent: String,
    pub directory: String,
    pub worktree: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolAttachment {
    #[serde(rename = "type")]
    pub kind: String,
    pub mime: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
}

/// `ToolResult`: a bare string or a structured result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ToolResult {
    Text(String),
    Structured {
        #[serde(skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        output: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        metadata: Option<Map<String, Value>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        attachments: Option<Vec<ToolAttachment>>,
    },
}

/// Stable plugin identity in load order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginId(pub String);

/// One loaded plugin instance. Implemented by the out-of-process host client and by Rust-native
/// first-party plugins.
#[async_trait]
pub trait PluginInstance: Send + Sync {
    fn id(&self) -> &PluginId;
    fn capabilities(&self) -> &HookCapabilities;
    /// Run one trigger hook. Returns the (possibly mutated) output.
    async fn trigger(
        &self,
        name: TriggerName,
        input: &Value,
        output: Value,
    ) -> Result<Value, IntegrationError>;
    async fn event(&self, event: &Value) -> Result<(), IntegrationError>;
    async fn config(&self, config: &Value) -> Result<(), IntegrationError>;
    async fn execute_tool(
        &self,
        tool: &str,
        args: Value,
        ctx: ToolContext,
    ) -> Result<ToolResult, IntegrationError>;
    async fn dispose(&self) -> Result<(), IntegrationError>;
}

/// Rust-owned ordering policy: run `name` over `plugins` in load order, threading `output`.
pub async fn trigger(
    plugins: &[std::sync::Arc<dyn PluginInstance>],
    name: TriggerName,
    input: &Value,
    mut output: Value,
) -> Result<Value, IntegrationError> {
    for plugin in plugins {
        if !plugin.capabilities().triggers.contains(&name) {
            continue;
        }
        output = plugin.trigger(name, input, output).await?;
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Arc;

    struct Append(PluginId, HookCapabilities, &'static str);

    #[async_trait]
    impl PluginInstance for Append {
        fn id(&self) -> &PluginId {
            &self.0
        }
        fn capabilities(&self) -> &HookCapabilities {
            &self.1
        }
        async fn trigger(
            &self,
            _: TriggerName,
            _: &Value,
            mut output: Value,
        ) -> Result<Value, IntegrationError> {
            output["system"].as_array_mut().unwrap().push(json!(self.2));
            Ok(output)
        }
        async fn event(&self, _: &Value) -> Result<(), IntegrationError> {
            Ok(())
        }
        async fn config(&self, _: &Value) -> Result<(), IntegrationError> {
            Ok(())
        }
        async fn execute_tool(
            &self,
            _: &str,
            _: Value,
            _: ToolContext,
        ) -> Result<ToolResult, IntegrationError> {
            Err(IntegrationError::NotFound {
                kind: "tool",
                name: "x".into(),
            })
        }
        async fn dispose(&self) -> Result<(), IntegrationError> {
            Ok(())
        }
    }

    #[test]
    fn names_roundtrip() {
        for t in TriggerName::ALL {
            assert_eq!(TriggerName::parse(t.as_str()), Some(t));
            assert_eq!(serde_json::to_value(t).unwrap(), json!(t.as_str()));
        }
    }

    #[test]
    fn trigger_threads_output_in_load_order() {
        let caps = |t: Vec<TriggerName>| HookCapabilities {
            triggers: t,
            ..Default::default()
        };
        let plugins: Vec<Arc<dyn PluginInstance>> = vec![
            Arc::new(Append(
                PluginId("a".into()),
                caps(vec![TriggerName::ChatSystemTransform]),
                "a",
            )),
            Arc::new(Append(PluginId("b".into()), caps(vec![]), "b")),
            Arc::new(Append(
                PluginId("c".into()),
                caps(vec![TriggerName::ChatSystemTransform]),
                "c",
            )),
        ];
        let out = futures_lite_block_on(trigger(
            &plugins,
            TriggerName::ChatSystemTransform,
            &json!({}),
            json!({"system": []}),
        ))
        .unwrap();
        assert_eq!(out, json!({"system": ["a", "c"]}));
        let r: ToolResult = serde_json::from_value(json!("x")).unwrap();
        assert_eq!(r, ToolResult::Text("x".into()));
    }

    fn futures_lite_block_on<F: std::future::Future>(f: F) -> F::Output {
        use std::task::{Context, Poll, Waker};
        let mut f = std::pin::pin!(f);
        let mut cx = Context::from_waker(Waker::noop());
        loop {
            if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
                return v;
            }
        }
    }
}
