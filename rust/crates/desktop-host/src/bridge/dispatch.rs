//! Purpose: capability dispatcher trait, sender/window-scoped authorization and bounded payload checks.
//! Scope: repository transaction.
//! Uses:
//! rust/crates/
//! `-- desktop-host/src/bridge/catalog.rs
//! External:
//! |-- serde_json::Value
//! |-- serde_json::to_string
//! `-- thiserror::Error
//! Used by:
//! rust/crates/
//! |-- desktop-host/src/bridge.rs
//! `-- desktop-host/tests/bridge_dispatch.rs
//! Invariants: dispatcher enforces payload size and origin before decoding.
//! Contract tests: desktop-host/tests/bridge_dispatch.rs

use crate::bridge::catalog::IpcChannel;
use serde_json::Value;
use thiserror::Error;

/// Context about the sender of an IPC message.
#[derive(Debug, Clone)]
pub struct SenderContext {
    pub window_id: Option<String>,
    pub origin: String,
}

/// Errors that can occur during IPC dispatch.
#[derive(Debug, Error)]
pub enum DispatchError {
    #[error("Channel not found: {0}")]
    NotFound(String),

    #[error("Unauthorized: {0}")]
    Unauthorized(String),

    #[error("Payload too large: {size} bytes exceeds {max} bytes")]
    PayloadTooLarge { size: usize, max: usize },

    #[error("Invalid arguments for channel {channel}: {reason}")]
    InvalidArgs { channel: String, reason: String },

    #[error("Unsupported channel or operation: {0}")]
    Unsupported(String),

    #[error("Operation failed: {0}")]
    Failed(String),
}

/// Default maximum payload size in bytes (10 MB).
pub const DEFAULT_MAX_PAYLOAD_SIZE: usize = 10 * 1024 * 1024;

/// Trait for dispatching IPC messages to capabilities.
///
/// Implementors (e.g., the shell or Tauri adapter) provide the actual
/// authorization logic and payload validation for each channel.
pub trait Dispatcher {
    /// Decode and authorize an incoming IPC request.
    ///
    /// # Arguments
    /// * `channel` - The string name of the IPC channel.
    /// * `args` - The raw JSON arguments payload.
    /// * `ctx` - The sender context (window ID, origin).
    ///
    /// # Returns
    /// The typed `IpcChannel` and validated `Value` arguments, or a `DispatchError`.
    fn decode(
        &self,
        channel: &str,
        args: &Value,
        ctx: &SenderContext,
    ) -> Result<(IpcChannel, Value), DispatchError> {
        let ipc_channel = IpcChannel::from_name(channel)
            .ok_or_else(|| DispatchError::NotFound(channel.to_string()))?;

        if ipc_channel == IpcChannel::Unsupported {
            return Err(DispatchError::Unsupported(channel.to_string()));
        }

        if !self.is_authorized(ipc_channel, ctx) {
            return Err(DispatchError::Unauthorized(format!(
                "origin '{}' not allowed for channel '{}'",
                ctx.origin, channel
            )));
        }

        self.validate_payload_size(args)?;

        Ok((ipc_channel, args.clone()))
    }

    /// Check if the sender context is authorized for the given channel.
    fn is_authorized(&self, channel: IpcChannel, ctx: &SenderContext) -> bool;

    /// Validate payload size bounds.
    fn validate_payload_size(&self, args: &Value) -> Result<(), DispatchError> {
        let size = serde_json::to_string(args).map(|s| s.len()).unwrap_or(0);
        let max = self.max_payload_size();
        if size > max {
            return Err(DispatchError::PayloadTooLarge { size, max });
        }
        Ok(())
    }

    /// Maximum allowed payload size in bytes.
    fn max_payload_size(&self) -> usize {
        DEFAULT_MAX_PAYLOAD_SIZE
    }
}
