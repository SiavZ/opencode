//! Purpose: legacy `glob` and `grep` tools (packages/opencode/src/tool/{glob,grep}.ts): ask before
//! resolving, external-directory approval, fixed 100-result limit and the legacy text output.
//! Scope: tool contract over `SearchServices`.
//! Uses:
//! rust/crates/
//! |-- platform-api/src/fs.rs
//! |-- platform-api/src/path.rs
//! |-- tool-api/src/lib.rs
//! |-- tools/src/files/common.rs
//! |-- tools/src/search/input.rs
//! |-- tools/src/search/mod.rs
//! `-- tools/src/search/ripgrep.rs
//! External:
//! |-- async_trait::async_trait
//! |-- path::normalize
//! |-- path::relative
//! |-- path::resolve
//! |-- schemas::legacy
//! |-- serde_json::Map::new
//! |-- serde_json::Value
//! |-- serde_json::json
//! `-- tokio::fs::metadata
//! Used by:
//! rust/crates/
//! |-- tools/src/search/mod.rs
//! `-- tools/tests/search_tools.rs
//! Invariants: `truncated` is `count == 100` (exactly 100 results reads as truncated, as in the
//! baseline). Legacy grep never passes `file` to ripgrep: an exact file path searches its whole
//! parent directory, preserved from the baseline (grep.ts:58-66).
//! Contract tests: tools/tests/search_tools.rs

use super::input::{legacy_invalid, object};
use super::ripgrep::{GlobInput, GrepInput};
use super::{SearchServices, schemas};
use crate::files::common::{authorize_legacy_external, legacy_output};
use async_trait::async_trait;
use opencode_platform_api::fs::ResolveKind;
use opencode_platform_api::path;
use opencode_tool_api::{Tool, ToolContext, ToolFailure, ToolOutput};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub const GLOB_DESCRIPTION: &str = include_str!("descriptions/glob.txt");
pub const GREP_DESCRIPTION: &str = include_str!("descriptions/grep.txt");
const LIMIT: usize = 100;

fn failed(error: impl std::fmt::Display) -> ToolFailure {
    ToolFailure::new(error.to_string())
}

#[derive(Debug, Clone)]
pub struct LegacyGlobTool {
    services: SearchServices,
}

impl LegacyGlobTool {
    pub fn new(services: SearchServices) -> Self {
        Self { services }
    }
}

#[async_trait]
impl Tool for LegacyGlobTool {
    fn description(&self) -> &str {
        GLOB_DESCRIPTION
    }
    fn input_schema(&self) -> Value {
        schemas::legacy("glob")
    }
    async fn execute(
        &self,
        input: Value,
        context: &ToolContext,
    ) -> Result<ToolOutput, ToolFailure> {
        let invalid = |detail| legacy_invalid("glob", detail);
        let object = object(&input).map_err(invalid)?;
        let pattern = object.string("pattern").map_err(invalid)?;
        let requested = object.optional_string("path").map_err(invalid)?;
        let mut meta = serde_json::Map::new();
        meta.insert("pattern".into(), json!(pattern));
        if let Some(requested) = &requested {
            meta.insert("path".into(), json!(requested));
        }
        context
            .authorize(
                "glob",
                vec![pattern.clone()],
                Some(vec!["*".into()]),
                Some(meta),
            )
            .await?;
        let directory = &self.services.directory;
        let search = path::resolve(
            directory,
            Path::new(requested.as_deref().unwrap_or(&directory.to_string_lossy())),
        );
        if tokio::fs::metadata(&search)
            .await
            .is_ok_and(|info| info.is_file())
        {
            return Err(failed(format!(
                "glob path must be a directory: {}",
                search.display()
            )));
        }
        authorize_legacy_external(
            directory,
            &self.services.worktree,
            &search,
            ResolveKind::Directory,
            false,
            context,
        )
        .await?;
        let files = self
            .services
            .ripgrep
            .glob(GlobInput {
                cwd: search.clone(),
                pattern,
                limit: LIMIT,
                cancel: Some(context.cancel.clone()),
                ..GlobInput::default()
            })
            .await
            .map_err(failed)?;
        let truncated = files.len() == LIMIT;
        let mut output: Vec<String> = Vec::new();
        if files.is_empty() {
            output.push("No files found".into());
        } else {
            output.extend(files.iter().map(|file| {
                path::resolve(&search, Path::new(&file.path.to_string()))
                    .to_string_lossy()
                    .into_owned()
            }));
            if truncated {
                output.push(String::new());
                output.push(format!("(Results are truncated: showing first {LIMIT} results. Consider using a more specific path or pattern.)"));
            }
        }
        Ok(legacy_output(
            path::relative(&self.services.worktree, &search),
            json!({"count": files.len(), "truncated": truncated}),
            output.join("\n"),
        ))
    }
}

#[derive(Debug, Clone)]
pub struct LegacyGrepTool {
    services: SearchServices,
}

impl LegacyGrepTool {
    pub fn new(services: SearchServices) -> Self {
        Self { services }
    }
}

/// `FSUtil.resolve`: realpath when the target exists, otherwise the lexical path.
fn real(target: &Path) -> PathBuf {
    std::fs::canonicalize(target).unwrap_or_else(|_| target.to_path_buf())
}

#[async_trait]
impl Tool for LegacyGrepTool {
    fn description(&self) -> &str {
        GREP_DESCRIPTION
    }
    fn input_schema(&self) -> Value {
        schemas::legacy("grep")
    }
    async fn execute(
        &self,
        input: Value,
        context: &ToolContext,
    ) -> Result<ToolOutput, ToolFailure> {
        let invalid = |detail| legacy_invalid("grep", detail);
        let object = object(&input).map_err(invalid)?;
        let pattern = object.string("pattern").map_err(invalid)?;
        let requested = object.optional_string("path").map_err(invalid)?;
        let include = object.optional_string("include").map_err(invalid)?;
        if pattern.is_empty() {
            return Err(failed("pattern is required"));
        }
        let mut meta = serde_json::Map::new();
        meta.insert("pattern".into(), json!(pattern));
        if let Some(requested) = &requested {
            meta.insert("path".into(), json!(requested));
        }
        if let Some(include) = &include {
            meta.insert("include".into(), json!(include));
        }
        context
            .authorize(
                "grep",
                vec![pattern.clone()],
                Some(vec!["*".into()]),
                Some(meta),
            )
            .await?;
        let directory = &self.services.directory;
        let requested = match requested {
            Some(requested) if Path::new(&requested).is_absolute() => PathBuf::from(requested),
            Some(requested) => path::normalize(&directory.join(requested)),
            None => directory.clone(),
        };
        let requested_is_dir = tokio::fs::metadata(&requested)
            .await
            .is_ok_and(|info| info.is_dir());
        let kind = if requested_is_dir {
            ResolveKind::Directory
        } else {
            ResolveKind::File
        };
        authorize_legacy_external(
            directory,
            &self.services.worktree,
            &requested,
            kind,
            false,
            context,
        )
        .await?;
        let search = real(&requested);
        let parent = |target: &Path| {
            target
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or(target.to_path_buf())
        };
        let cwd = if tokio::fs::metadata(&search)
            .await
            .is_ok_and(|info| info.is_dir())
        {
            search.clone()
        } else {
            parent(&search)
        };
        let result = self
            .services
            .ripgrep
            .grep(GrepInput {
                cwd,
                pattern: pattern.clone(),
                file: None,
                include,
                limit: LIMIT,
                cancel: Some(context.cancel.clone()),
            })
            .await
            .map_err(failed)?;
        if result.is_empty() {
            return Ok(legacy_output(
                pattern,
                json!({"matches": 0, "truncated": false}),
                "No files found".into(),
            ));
        }
        let base = if requested_is_dir {
            requested.clone()
        } else {
            parent(&requested)
        };
        let truncated = result.len() == LIMIT;
        let mut output = vec![format!(
            "Found {} matches{}",
            result.len(),
            if truncated {
                " (more matches available)"
            } else {
                ""
            }
        )];
        let mut current = String::new();
        for found in &result {
            let file = path::resolve(&base, Path::new(&found.entry.path.to_string()))
                .to_string_lossy()
                .into_owned();
            if current != file {
                if !current.is_empty() {
                    output.push(String::new());
                }
                output.push(format!("{file}:"));
                current = file;
            }
            output.push(format!("  Line {}: {}", found.line.get(), found.text));
        }
        if truncated {
            output.push(String::new());
            output.push(
                "(Results truncated. Consider using a more specific path or pattern.)".into(),
            );
        }
        Ok(legacy_output(
            pattern,
            json!({"matches": result.len(), "truncated": truncated}),
            output.join("\n"),
        ))
    }
}
