//! Purpose: legacy file and search HTTP operations over native filesystem and ripgrep.
//! Scope: explicit Location directory; symlink and lexical containment checked before reads.
//! Uses:
//! rust/crates/
//! |-- platform-native/src/fs/native.rs
//! |-- runtime/src/global_services.rs
//! `-- tools/src/search/ripgrep.rs
//! External:
//! |-- base64::Engine
//! `-- ignore::gitignore::GitignoreBuilder
//! Used by:
//! rust/crates/
//! `-- runtime/src/global_services.rs
//! Invariants: missing reads are empty text, binary reads retain bytes, listing excludes symlinks.
//! Contract tests: runtime/tests/global_services.rs

use super::{GlobalServices, LegacyError, Map, RouteContext, Value, internal, json, required};
use base64::Engine;
use opencode_platform_api::fs::{FileSystem, FileType};
use opencode_platform_native::fs::NativeFileSystem;
use opencode_platform_native::process::NativeProcessRunner;
use opencode_tools::search::ripgrep::{GlobInput, GrepInput};
use opencode_tools::search::{NoInstaller, Ripgrep, RipgrepBinary};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

impl GlobalServices {
    fn ripgrep(&self) -> Ripgrep {
        Ripgrep::new(
            Arc::new(NativeProcessRunner::new()),
            Arc::new(RipgrepBinary::new(
                self.options.paths.bin.clone(),
                Some(
                    self.options
                        .environment
                        .var("PATH")
                        .unwrap_or("")
                        .to_owned(),
                ),
                Arc::new(NoInstaller),
            )),
        )
    }

    pub(super) async fn files(
        &self,
        operation: &str,
        context: &RouteContext,
        query: &Map<String, Value>,
    ) -> Result<Value, LegacyError> {
        match operation {
            // The baseline producers intentionally return empty arrays for these two operations.
            "file.status" | "find.symbols" => Ok(json!([])),
            "file.read" => {
                let path = target(Path::new(&context.directory), required(query, "path")?).await?;
                if !NativeFileSystem.exists_safe(&path).await {
                    return Ok(json!({"type":"text","content":""}));
                }
                let bytes = NativeFileSystem.read(&path).await.map_err(internal)?;
                if !bytes.contains(&0)
                    && let Ok(text) = std::str::from_utf8(&bytes)
                {
                    return Ok(
                        json!({"type":"text","content":text.trim_start_matches('\u{feff}').trim()}),
                    );
                }
                Ok(
                    json!({"type":"binary","content":base64::engine::general_purpose::STANDARD.encode(bytes),"encoding":"base64","mimeType":opencode_tools::files::read_common::mime_type(&path.to_string_lossy())}),
                )
            }
            "file.list" => {
                let directory = Path::new(&context.directory);
                let path = target(directory, required(query, "path")?).await?;
                let project = self
                    .projects
                    .from_directory(&context.directory)
                    .await
                    .map_err(internal)?;
                let root = Path::new(&project.project.worktree);
                let mut builder = ignore::gitignore::GitignoreBuilder::new(root);
                for name in [".gitignore", ".ignore"] {
                    builder.add(root.join(name));
                }
                let ignored = builder.build().map_err(internal)?;
                let mut entries = NativeFileSystem.read_dir(&path).await.map_err(internal)?.into_iter().filter_map(|(name, kind)| {
                    let kind = match kind { FileType::File => "file", FileType::Directory => "directory", _ => return None };
                    let absolute = path.join(name);
                    let relative = absolute.strip_prefix(directory).ok()?.to_string_lossy().into_owned();
                    let relative = if kind == "directory" { format!("{relative}{}", std::path::MAIN_SEPARATOR) } else { relative };
                    Some(json!({"name":Path::new(&relative).file_name()?.to_string_lossy(),"path":relative,"absolute":absolute,"type":kind,"ignored":ignored.matched(&absolute, kind == "directory").is_ignore()}))
                }).collect::<Vec<_>>();
                entries.sort_by(|a, b| {
                    let directory = |item: &Value| item["type"] == "directory";
                    directory(b)
                        .cmp(&directory(a))
                        .then_with(|| a["path"].as_str().cmp(&b["path"].as_str()))
                });
                Ok(Value::Array(entries))
            }
            "find.text" => {
                let matches = self
                    .ripgrep()
                    .grep(GrepInput {
                        cwd: context.directory.clone().into(),
                        pattern: required(query, "pattern")?.into(),
                        limit: 10,
                        ..GrepInput::default()
                    })
                    .await
                    .map_err(internal)?;
                Ok(Value::Array(matches.into_iter().map(|item| json!({"path":{"text":item.entry.path},"lines":{"text":item.text},"line_number":item.line,"absolute_offset":item.offset,"submatches":item.submatches.into_iter().map(|item|json!({"match":{"text":item.text},"start":item.start,"end":item.end})).collect::<Vec<_>>()})).collect()))
            }
            "find.files" => {
                let needle = required(query, "query")?.to_lowercase();
                let limit = query.get("limit").and_then(Value::as_u64).unwrap_or(10) as usize;
                let kind = query.get("type").and_then(Value::as_str).or_else(|| {
                    (query.get("dirs").and_then(Value::as_str) == Some("false")).then_some("file")
                });
                let files = self
                    .ripgrep()
                    .find(GlobInput {
                        cwd: context.directory.clone().into(),
                        pattern: "*".into(),
                        limit: 100_000,
                        ..GlobInput::default()
                    })
                    .await
                    .map_err(internal)?;
                let mut paths = Vec::new();
                for file in files {
                    let path = file.path.as_str();
                    if kind != Some("directory") {
                        paths.push(path.to_owned());
                    }
                    if kind != Some("file") {
                        for parent in Path::new(path)
                            .ancestors()
                            .skip(1)
                            .filter(|path| !path.as_os_str().is_empty())
                        {
                            paths.push(format!(
                                "{}{}",
                                parent.display(),
                                std::path::MAIN_SEPARATOR
                            ));
                        }
                    }
                }
                paths.sort();
                paths.dedup();
                let mut found = paths
                    .into_iter()
                    .filter_map(|path| {
                        fuzzy(&needle, &path.to_lowercase()).map(|score| (score, path))
                    })
                    .collect::<Vec<_>>();
                found.sort_by(|a, b| {
                    a.0.cmp(&b.0)
                        .then(a.1.len().cmp(&b.1.len()))
                        .then(a.1.cmp(&b.1))
                });
                Ok(json!(
                    found
                        .into_iter()
                        .take(limit)
                        .map(|(_, path)| path)
                        .collect::<Vec<_>>()
                ))
            }
            _ => Err(LegacyError::Unavailable(format!(
                "Unknown filesystem operation: {operation}"
            ))),
        }
    }
}

pub(super) async fn target(directory: &Path, input: &str) -> Result<PathBuf, LegacyError> {
    let root = tokio::fs::canonicalize(directory).await.map_err(internal)?;
    let input = Path::new(input);
    let mut absolute = if input.is_absolute() {
        PathBuf::new()
    } else {
        root.clone()
    };
    for component in input.components() {
        match component {
            Component::ParentDir => {
                absolute.pop();
            }
            Component::CurDir => {}
            component => absolute.push(component.as_os_str()),
        }
    }
    if !absolute.starts_with(&root) {
        return Err(internal("Path escapes the location"));
    }
    match tokio::fs::canonicalize(&absolute).await {
        Ok(real) if !real.starts_with(&root) => Err(internal("Path escapes the location")),
        Ok(real) => Ok(real),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            for ancestor in absolute.ancestors().skip(1) {
                match tokio::fs::canonicalize(ancestor).await {
                    Ok(real) if !real.starts_with(&root) => {
                        return Err(internal("Path escapes the location"));
                    }
                    Ok(_) => return Ok(absolute),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(error) => return Err(internal(error)),
                }
            }
            Err(internal("Path escapes the location"))
        }
        Err(error) => Err(internal(error)),
    }
}

fn fuzzy(query: &str, path: &str) -> Option<usize> {
    let mut offset = 0;
    let mut score = 0;
    for character in query.chars() {
        let next = path[offset..].find(character)?;
        score += next;
        offset += next + character.len_utf8();
    }
    Some(score)
}
