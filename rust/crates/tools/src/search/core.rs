//! Purpose: core V2 `glob` and `grep` leaf tools (packages/core/src/tool/{glob,grep}.ts): permission
//! on the pattern, Location-relative search through ripgrep, concise model text.
//! Scope: tool contract over `SearchServices`.
//! Uses:
//! rust/crates/
//! |-- platform-api/src/path.rs
//! |-- schema/src/filesystem.rs
//! |-- schema/src/lib.rs
//! |-- tool-api/src/lib.rs
//! |-- tools/src/search/input.rs
//! |-- tools/src/search/mod.rs
//! `-- tools/src/search/ripgrep.rs
//! External:
//! |-- async_trait::async_trait
//! |-- path::relative
//! |-- path::resolve
//! |-- schemas::core
//! |-- serde_json::Value
//! |-- serde_json::json
//! |-- serde_json::to_value
//! `-- tokio::fs::metadata
//! Used by:
//! rust/crates/
//! |-- tools/src/search/mod.rs
//! `-- tools/tests/search_tools.rs
//! Invariants: baseline semantics are preserved verbatim, including the containment concern: the
//! permission check covers only the pattern, and `path` is resolved afterwards without Location
//! containment, so `../` or absolute paths search outside the Location (see status.md finding).
//! Every failure after decoding, including permission denial, maps to the generic message.
//! Contract tests: tools/tests/search_tools.rs

use super::input::{metadata, object};
use super::ripgrep::{GlobInput, GrepInput};
use super::{SearchServices, schemas};
use async_trait::async_trait;
use opencode_platform_api::path;
use opencode_schema::RelativePath;
use opencode_schema::filesystem::{Entry, Match};
use opencode_tool_api::{Tool, ToolContent, ToolContext, ToolFailure, ToolOutput};
use serde_json::{Value, json};
use std::path::Path;

pub const GLOB_DESCRIPTION: &str = "Find files by glob pattern within the active Location. Returns concise relative file resources. Use a relative path to narrow the search and limit to bound the result count.";
pub const GREP_DESCRIPTION: &str = "Search file contents by regular expression within the active Location or an absolute managed tool-output file. Use a path to narrow the search, include to filter files by glob, and limit to bound the match count. Returns concise file resources, line numbers, and bounded line previews.";

const UNBOUNDED: usize = 9_007_199_254_740_991;

/// `GlobTool.toModelOutput` over absolute paths.
pub fn glob_text(paths: &[String]) -> String {
    if paths.is_empty() {
        return "No files found".into();
    }
    paths.join("\n")
}

/// `GrepTool.toModelOutput` over absolute paths.
pub fn grep_text(matches: &[(String, i64, String)]) -> String {
    if matches.is_empty() {
        return "No files found".into();
    }
    let mut lines = vec![format!("Found {} matches", matches.len())];
    let mut current = "";
    for (file, line, text) in matches {
        if current != file {
            if !current.is_empty() {
                lines.push(String::new());
            }
            current = file;
            lines.push(format!("{file}:"));
        }
        lines.push(format!("  Line {line}: {text}"));
    }
    lines.join("\n")
}

fn rebase(services: &SearchServices, cwd: &Path, entry: &Entry) -> Entry {
    Entry {
        path: RelativePath::new(path::relative(
            &services.directory,
            &cwd.join(entry.path.to_string()),
        )),
        r#type: entry.r#type,
    }
}

fn absolute(services: &SearchServices, relative: &RelativePath) -> String {
    path::resolve(&services.directory, Path::new(&relative.to_string()))
        .to_string_lossy()
        .into_owned()
}

#[derive(Debug, Clone)]
pub struct CoreGlobTool {
    services: SearchServices,
}

impl CoreGlobTool {
    pub fn new(services: SearchServices) -> Self {
        Self { services }
    }

    async fn search(
        &self,
        pattern: &str,
        dir: Option<String>,
        limit: Option<u64>,
        context: &ToolContext,
    ) -> Result<Vec<Entry>, String> {
        let meta = metadata(vec![
            (
                "root",
                Some(json!(dir.clone().unwrap_or_else(|| ".".into()))),
            ),
            ("path", dir.clone().map(Value::from)),
            ("limit", limit.map(Value::from)),
        ]);
        context
            .authorize(
                "glob",
                vec![pattern.into()],
                Some(vec!["*".into()]),
                Some(meta),
            )
            .await
            .map_err(|error| error.to_string())?;
        let cwd = path::resolve(
            &self.services.directory,
            Path::new(dir.as_deref().unwrap_or(".")),
        );
        let found = self
            .services
            .ripgrep
            .glob(GlobInput {
                cwd: cwd.clone(),
                pattern: pattern.into(),
                limit: limit.map_or(UNBOUNDED, |limit| limit as usize),
                cancel: Some(context.cancel.clone()),
                ..GlobInput::default()
            })
            .await
            .map_err(|error| error.to_string())?;
        Ok(found
            .iter()
            .map(|entry| rebase(&self.services, &cwd, entry))
            .collect())
    }
}

#[async_trait]
impl Tool for CoreGlobTool {
    fn description(&self) -> &str {
        GLOB_DESCRIPTION
    }
    fn input_schema(&self) -> Value {
        schemas::core("glob", "input")
    }
    fn output_schema(&self) -> Option<Value> {
        Some(schemas::core("glob", "output"))
    }
    async fn execute(
        &self,
        input: Value,
        context: &ToolContext,
    ) -> Result<ToolOutput, ToolFailure> {
        let object = object(&input).map_err(ToolFailure::invalid_input)?;
        let pattern = object
            .string("pattern")
            .map_err(ToolFailure::invalid_input)?;
        let dir = object
            .optional_string("path")
            .map_err(ToolFailure::invalid_input)?;
        let limit = object
            .optional_positive("limit", None)
            .map_err(ToolFailure::invalid_input)?;
        let entries = self
            .search(&pattern, dir, limit, context)
            .await
            .map_err(|_| ToolFailure::new(format!("Unable to find files matching {pattern}")))?;
        let paths: Vec<String> = entries
            .iter()
            .map(|entry| absolute(&self.services, &entry.path))
            .collect();
        Ok(ToolOutput::new(
            serde_json::to_value(&entries).unwrap_or_default(),
            vec![ToolContent::text(glob_text(&paths))],
        ))
    }
}

#[derive(Debug, Clone)]
pub struct CoreGrepTool {
    services: SearchServices,
}

impl CoreGrepTool {
    pub fn new(services: SearchServices) -> Self {
        Self { services }
    }

    async fn search(
        &self,
        pattern: &str,
        dir: Option<String>,
        include: Option<String>,
        limit: Option<u64>,
        context: &ToolContext,
    ) -> Result<Vec<Match>, String> {
        let meta = metadata(vec![
            ("root", Some(json!("."))),
            ("path", dir.clone().map(Value::from)),
            ("include", include.clone().map(Value::from)),
            ("limit", limit.map(Value::from)),
        ]);
        context
            .authorize(
                "grep",
                vec![pattern.into()],
                Some(vec!["*".into()]),
                Some(meta),
            )
            .await
            .map_err(|error| error.to_string())?;
        let target = path::resolve(
            &self.services.directory,
            Path::new(dir.as_deref().unwrap_or(".")),
        );
        let info = tokio::fs::metadata(&target).await.ok();
        let is_dir = info.as_ref().is_some_and(|info| info.is_dir());
        let is_file = info.as_ref().is_some_and(|info| info.is_file());
        let cwd = if is_dir {
            target.clone()
        } else {
            target
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or(target.clone())
        };
        let file = is_file
            .then(|| {
                target
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .flatten();
        let found = self
            .services
            .ripgrep
            .grep(GrepInput {
                cwd: cwd.clone(),
                pattern: pattern.into(),
                file,
                include,
                limit: limit.map_or(UNBOUNDED, |limit| limit as usize),
                cancel: Some(context.cancel.clone()),
            })
            .await
            .map_err(|error| error.to_string())?;
        Ok(found
            .into_iter()
            .map(|mut found| {
                found.entry = rebase(&self.services, &cwd, &found.entry);
                found
            })
            .collect())
    }
}

#[async_trait]
impl Tool for CoreGrepTool {
    fn description(&self) -> &str {
        GREP_DESCRIPTION
    }
    fn input_schema(&self) -> Value {
        schemas::core("grep", "input")
    }
    fn output_schema(&self) -> Option<Value> {
        Some(schemas::core("grep", "output"))
    }
    async fn execute(
        &self,
        input: Value,
        context: &ToolContext,
    ) -> Result<ToolOutput, ToolFailure> {
        let object = object(&input).map_err(ToolFailure::invalid_input)?;
        let pattern = object
            .string("pattern")
            .map_err(ToolFailure::invalid_input)?;
        let dir = object
            .optional_string("path")
            .map_err(ToolFailure::invalid_input)?;
        let include = object
            .optional_string("include")
            .map_err(ToolFailure::invalid_input)?;
        let limit = object
            .optional_positive("limit", None)
            .map_err(ToolFailure::invalid_input)?;
        let matches = self
            .search(&pattern, dir, include, limit, context)
            .await
            .map_err(|_| ToolFailure::new(format!("Unable to grep for {pattern}")))?;
        let rows: Vec<(String, i64, String)> = matches
            .iter()
            .map(|found| {
                (
                    absolute(&self.services, &found.entry.path),
                    found.line.get(),
                    found.text.clone(),
                )
            })
            .collect();
        Ok(ToolOutput::new(
            serde_json::to_value(&matches).unwrap_or_default(),
            vec![ToolContent::text(grep_text(&rows))],
        ))
    }
}
