//! Purpose: Core (V2) `read` tool: Location resolution, external then read approval, file text
//! pages, supported images through the image normalizer, and directory pages.
//! Scope: `packages/core/src/tool/read.ts` execute and model-output mapping.
//! Uses:
//! rust/crates/
//! |-- platform-api/src/fs.rs
//! |-- tool-api/src/context.rs
//! |-- tool-api/src/definition.rs
//! |-- tool-api/src/output.rs
//! |-- tools/src/files/common.rs
//! |-- tools/src/files/read_common.rs
//! |-- tools/src/files/read_files.rs
//! |-- tools/src/files/read_input.rs
//! `-- tools/src/files/schemas.rs
//! External:
//! `-- async_trait::async_trait
//! Used by:
//! rust/crates/
//! `-- tools/src/files/mod.rs
//! Invariants: external_directory approval precedes any stat or content access, read approval
//! precedes content access, only binary/media/image errors keep their message, everything else
//! becomes `Unable to read <path>`.
//! Contract tests: tools/tests/files_read.rs

use super::common::{FileServices, Generation, text_output};
use super::read_common::SUPPORTED_IMAGE_MIMES;
use super::read_files::{self, Content, Encoding, FileRead, Kind, Page, ReadError};
use super::read_input::decode_core;
use super::schemas;
use async_trait::async_trait;
use opencode_platform_api::fs::ResolveKind;
use opencode_tool_api::{Tool, ToolContent, ToolContext, ToolFailure, ToolOutput};
use serde_json::Value;
use std::sync::Arc;
use std::time::SystemTime;

pub const NAME: &str = "read";

/// `Image.normalize` failures.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ImageError {
    #[error("Image resizer unavailable")]
    ResizerUnavailable,
    #[error("Image could not be decoded: {resource}")]
    Decode { resource: String },
    #[error(
        "Image {resource} is {width}x{height} with base64 size {bytes}, exceeding configured limits {max_width}x{max_height}/{max_bytes} bytes"
    )]
    Size {
        resource: String,
        width: u64,
        height: u64,
        bytes: u64,
        max_width: u64,
        max_height: u64,
        max_bytes: u64,
    },
}

/// `Image.Service` port. Config-driven resizing lives with the image adapter owner.
#[async_trait]
pub trait ImageNormalizer: Send + Sync + std::fmt::Debug {
    async fn normalize(&self, resource: &str, content: Content) -> Result<Content, ImageError>;
}

/// Equivalent of the baseline when the photon resizer cannot load: images pass through unchanged.
#[derive(Debug, Default)]
pub struct NoResizer;

#[async_trait]
impl ImageNormalizer for NoResizer {
    async fn normalize(&self, _resource: &str, _content: Content) -> Result<Content, ImageError> {
        Err(ImageError::ResizerUnavailable)
    }
}

#[derive(Debug, Clone)]
pub struct CoreReadTool {
    services: FileServices,
    image: Arc<dyn ImageNormalizer>,
}

impl CoreReadTool {
    pub fn new(services: FileServices, image: Arc<dyn ImageNormalizer>) -> Self {
        Self { services, image }
    }

    async fn run(&self, path: &str, page: Page, context: &ToolContext) -> Result<Value, Failure> {
        let fs = self.services.fs.as_ref();
        let target = self.services.resolve(path, ResolveKind::Directory).await?;
        self.services.authorize_external(&target, context).await?;
        let kind = read_files::inspect(fs, &target.canonical).await?;
        context
            .authorize(
                NAME,
                vec![target.resource.clone()],
                Some(vec!["*".into()]),
                None,
            )
            .await
            .map_err(ToolFailure::from)?;
        if kind == Kind::Directory {
            return Ok(to_value(
                &read_files::list(fs, &target.canonical, page).await?,
            ));
        }
        let content = read_files::read(fs, &target.canonical, &target.resource, page).await?;
        let modified = fs
            .metadata(&target.canonical)
            .await
            .ok()
            .and_then(|info| info.modified);
        self.services.record_read(
            &context.session_id,
            &target.canonical,
            modified.unwrap_or_else(SystemTime::now),
        );
        let content = match content {
            FileRead::Page(page) => return Ok(to_value(&page)),
            FileRead::Content(content) => content,
        };
        if content.encoding == Encoding::Base64 && SUPPORTED_IMAGE_MIMES.contains(&&*content.mime) {
            let normalized = match self
                .image
                .normalize(&target.resource, content.clone())
                .await
            {
                Err(ImageError::ResizerUnavailable) => content,
                other => other?,
            };
            return Ok(to_value(&normalized));
        }
        if content.encoding == Encoding::Base64 {
            return Err(ReadError::Binary {
                resource: target.resource,
            }
            .into());
        }
        Ok(to_value(&content))
    }
}

#[async_trait]
impl Tool for CoreReadTool {
    fn description(&self) -> &str {
        schemas::description(Generation::Core, NAME)
    }

    fn input_schema(&self) -> Value {
        schemas::input_schema(Generation::Core, NAME)
    }

    fn output_schema(&self) -> Option<Value> {
        schemas::output_schema(Generation::Core, NAME)
    }

    async fn execute(
        &self,
        input: Value,
        context: &ToolContext,
    ) -> Result<ToolOutput, ToolFailure> {
        let input = decode_core(&input).map_err(ToolFailure::invalid_input)?;
        let page = Page {
            offset: input.offset,
            limit: input.limit,
        };
        let output =
            self.run(&input.path, page, context)
                .await
                .map_err(|failure| match failure {
                    Failure::Visible(message) => ToolFailure::new(message),
                    Failure::Hidden => ToolFailure::new(format!("Unable to read {}", input.path)),
                })?;
        Ok(model_output(&input.path, output))
    }
}

/// `toModelOutput`: supported base64 images become text plus native media, others stay JSON.
fn model_output(path: &str, structured: Value) -> ToolOutput {
    let image = structured["encoding"] == "base64"
        && SUPPORTED_IMAGE_MIMES
            .iter()
            .any(|mime| structured["mime"] == *mime);
    if !image {
        return ToolOutput::new(structured, vec![]);
    }
    let media = ToolContent::file_base64(
        structured["content"].as_str().unwrap_or_default(),
        structured["mime"].as_str().unwrap_or_default(),
        Some(path.into()),
    );
    let mut output = text_output(structured, "Image read successfully");
    output.content.push(media);
    output
}

fn to_value(value: &impl serde::Serialize) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

/// Baseline error mapping: only some typed errors surface their own message.
enum Failure {
    Visible(String),
    Hidden,
}

impl From<ToolFailure> for Failure {
    fn from(_: ToolFailure) -> Self {
        Self::Hidden
    }
}

impl From<ReadError> for Failure {
    fn from(error: ReadError) -> Self {
        match error {
            ReadError::Binary { .. } | ReadError::MediaIngestLimit { .. } => {
                Self::Visible(error.to_string())
            }
            _ => Self::Hidden,
        }
    }
}

impl From<ImageError> for Failure {
    fn from(error: ImageError) -> Self {
        match error {
            ImageError::ResizerUnavailable => Self::Hidden,
            other => Self::Visible(other.to_string()),
        }
    }
}
