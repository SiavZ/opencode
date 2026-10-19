//! Purpose: shared portable services and approval boundaries for both file-tool generations.
//! Scope: Location-scoped tool dependencies, no direct native I/O.
//! Uses:
//! rust/crates/
//! |-- platform-api/src/fs.rs
//! |-- platform-api/src/path.rs
//! `-- tool-api/src/lib.rs
//! External:
//! |-- path::contains
//! |-- path::relative
//! |-- path::resolve
//! |-- path::slash
//! |-- serde_json::Map
//! |-- serde_json::Value
//! `-- serde_json::json
//! Used by:
//! rust/crates/
//! |-- tools/src/files/edit.rs
//! |-- tools/src/files/edit_legacy.rs
//! |-- tools/src/files/mod.rs
//! |-- tools/src/files/read_core.rs
//! |-- tools/src/files/read_legacy.rs
//! |-- tools/src/files/schemas.rs
//! |-- tools/src/files/write.rs
//! |-- tools/src/search/legacy.rs
//! |-- tools/src/shell/legacy.rs
//! `-- tools/tests/files_common.rs
//! Invariants: helpers perform no content reads, external approval precedes operation approval.
//! Contract tests: tools/tests/files_common.rs

use opencode_platform_api::fs::{
    FileMutation, FileSystem, LocationMutation, ResolveInput, ResolveKind, Target,
};
use opencode_platform_api::path;
use opencode_tool_api::{ToolContent, ToolContext, ToolFailure, ToolOutput};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Generation {
    Core,
    Legacy,
}

#[derive(Debug, Clone)]
pub struct FileServices {
    pub fs: Arc<dyn FileSystem>,
    pub mutation: Arc<dyn FileMutation>,
    pub location: Arc<dyn LocationMutation>,
    pub directory: PathBuf,
    pub worktree: PathBuf,
    read_stamps: Arc<Mutex<HashMap<(String, PathBuf), SystemTime>>>,
}

impl FileServices {
    pub fn new(
        fs: Arc<dyn FileSystem>,
        mutation: Arc<dyn FileMutation>,
        location: Arc<dyn LocationMutation>,
        directory: PathBuf,
        worktree: PathBuf,
    ) -> Self {
        Self {
            fs,
            mutation,
            location,
            directory,
            worktree,
            read_stamps: Arc::default(),
        }
    }

    pub async fn resolve(&self, input: &str, kind: ResolveKind) -> Result<Target, ToolFailure> {
        self.location
            .resolve(ResolveInput {
                path: input.into(),
                kind: Some(kind),
            })
            .await
            .map_err(|error| ToolFailure::new(error.to_string()))
    }

    /// Legacy tools resolve lexically and allow external relative paths; they do not use V2's
    /// relative escape rejection. Approval resources remain worktree-relative.
    pub fn legacy_path(&self, input: &str) -> PathBuf {
        path::resolve(&self.directory, Path::new(input))
    }

    pub fn title(&self, target: &Path) -> String {
        path::relative(&self.worktree, target)
    }

    pub async fn authorize_external(
        &self,
        target: &Target,
        context: &ToolContext,
    ) -> Result<(), ToolFailure> {
        authorize_external(target, context).await
    }

    pub async fn authorize(
        &self,
        target: &Target,
        action: &str,
        context: &ToolContext,
        metadata: Option<Map<String, Value>>,
    ) -> Result<(), ToolFailure> {
        authorize_target(target, action, context, metadata).await
    }

    /// Legacy external-directory contract, including bypass and lexical worktree containment.
    pub async fn authorize_legacy_external(
        &self,
        target: &Path,
        kind: ResolveKind,
        bypass: bool,
        context: &ToolContext,
    ) -> Result<bool, ToolFailure> {
        authorize_legacy_external(
            &self.directory,
            &self.worktree,
            target,
            kind,
            bypass,
            context,
        )
        .await
    }

    /// Advisory observations are session-scoped. Core edit correctness uses conditional mutation,
    /// not these stamps. Baseline current legacy edit does not enforce read-before-edit.
    pub fn record_read(&self, session: &str, target: &Path, time: SystemTime) {
        self.read_stamps
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert((session.into(), target.into()), time);
    }

    pub fn read_stamp(&self, session: &str, target: &Path) -> Option<SystemTime> {
        self.read_stamps
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(&(session.into(), target.into()))
            .copied()
    }
}

pub async fn authorize_legacy_external(
    directory: &Path,
    worktree: &Path,
    target: &Path,
    kind: ResolveKind,
    bypass: bool,
    context: &ToolContext,
) -> Result<bool, ToolFailure> {
    // Non-git projects use worktree "/", which would contain every absolute path
    // (packages/opencode/src/project/instance-context.ts containsPath).
    let in_worktree = worktree != Path::new("/") && path::contains(worktree, target);
    if bypass || path::contains(directory, target) || in_worktree {
        return Ok(false);
    }
    let directory = if kind == ResolveKind::Directory {
        target
    } else {
        target.parent().unwrap_or(target)
    };
    let resource = path::slash(&directory.join("*").to_string_lossy());
    let metadata = serde_json::json!({"filepath": target.to_string_lossy(), "parentDir": directory.to_string_lossy()});
    context
        .authorize(
            "external_directory",
            vec![resource.clone()],
            Some(vec![resource]),
            metadata.as_object().cloned(),
        )
        .await?;
    Ok(true)
}

pub async fn authorize_external(target: &Target, context: &ToolContext) -> Result<(), ToolFailure> {
    if let Some(external) = &target.external_directory {
        let (action, resources, save) = external.permission();
        context
            .authorize(action, resources, Some(save), None)
            .await?;
    }
    Ok(())
}

pub async fn authorize_target(
    target: &Target,
    action: &str,
    context: &ToolContext,
    metadata: Option<Map<String, Value>>,
) -> Result<(), ToolFailure> {
    authorize_external(target, context).await?;
    context
        .authorize(
            action,
            vec![target.resource.clone()],
            Some(vec!["*".into()]),
            metadata,
        )
        .await?;
    Ok(())
}

pub fn text_output(structured: Value, text: impl Into<String>) -> ToolOutput {
    ToolOutput::new(structured, vec![ToolContent::text(text)])
}

pub fn legacy_output(title: String, metadata: Value, output: String) -> ToolOutput {
    text_output(
        serde_json::json!({"title": title, "metadata": metadata, "output": output}),
        output,
    )
}
