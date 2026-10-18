//! Purpose: core ripgrep adapter (packages/core/src/ripgrep.ts): `glob`, `find` and `grep` rows from
//! `rg --files` / `rg --json` with limits, invalid-pattern detection and bounded records.
//! Scope: process-oriented rows only; no permission or presentation.
//! Uses:
//! rust/crates/
//! |-- platform-api/src/process.rs
//! |-- schema/src/filesystem.rs
//! `-- tools/src/search/binary.rs
//! External:
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! |-- tools/src/search/core.rs
//! |-- tools/src/search/legacy.rs
//! `-- tools/src/search/mod.rs
//! Invariants: stops after `limit + 1` rows and kills rg; exit 1 means no rows; exit 2 with a regex
//! parse error is `InvalidPattern`; other non-0/1/2 exits fail with bounded stderr.
//! Contract tests: tools/tests/search_ripgrep.rs

use super::binary::RipgrepBinary;
use opencode_platform_api::process::{Command, ProcessRunner, RunOptions};
use opencode_schema::filesystem::{Entry, EntryType, Match, Submatch};
use opencode_schema::{NonNegativeInt, PositiveInt, RelativePath};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

const ERROR_BYTES: usize = 8 * 1024;
const MAX_RECORD_BYTES: usize = 64 * 1024;
const MAX_SUBMATCHES: usize = 100;
const PREVIEW_UNITS: usize = 2_000;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RipgrepError {
    #[error("{message}")]
    Failed { message: String },
    #[error("{message}")]
    InvalidPattern { pattern: String, message: String },
}

fn failure(message: impl Into<String>) -> RipgrepError {
    RipgrepError::Failed {
        message: message.into(),
    }
}

#[derive(Debug, Clone, Default)]
pub struct GlobInput {
    pub cwd: PathBuf,
    pub pattern: String,
    pub limit: usize,
    pub hidden: bool,
    pub follow: bool,
    pub cancel: Option<CancellationToken>,
}

#[derive(Debug, Clone, Default)]
pub struct GrepInput {
    pub cwd: PathBuf,
    pub pattern: String,
    pub file: Option<String>,
    pub include: Option<String>,
    pub limit: usize,
    pub cancel: Option<CancellationToken>,
}

#[derive(Debug, Clone)]
pub struct Ripgrep {
    runner: Arc<dyn ProcessRunner>,
    binary: Arc<RipgrepBinary>,
}

/// Strip leading `./` and `/` segments and normalize separators, as the baseline parsers do.
fn relative(line: &str) -> String {
    let mut text = line;
    while let Some(rest) = text.strip_prefix("./").or_else(|| text.strip_prefix(".\\")) {
        text = rest;
    }
    text.trim_start_matches(['/', '\\']).replace('\\', "/")
}

fn entry(path: String) -> Entry {
    Entry {
        path: RelativePath::new(path),
        r#type: EntryType::File,
    }
}

impl Ripgrep {
    pub fn new(runner: Arc<dyn ProcessRunner>, binary: Arc<RipgrepBinary>) -> Self {
        Self { runner, binary }
    }

    /// Run rg, parsing non-empty stdout lines and keeping at most `limit + 1` parsed rows
    /// (baseline `Stream.mapEffect(parse) -> filter -> take(limit + 1)`).
    async fn run<T>(
        &self,
        cwd: &std::path::Path,
        args: Vec<String>,
        limit: usize,
        pattern: Option<&str>,
        cancel: Option<CancellationToken>,
        parse: impl Fn(&str) -> Result<Option<T>, RipgrepError>,
    ) -> Result<Vec<T>, RipgrepError> {
        let program = self.binary.filepath().await.map_err(failure)?;
        let command = Command::new(program.to_string_lossy()).args(args).cwd(cwd);
        let (sender, mut receiver) = tokio::sync::mpsc::channel::<Vec<u8>>(16);
        let stop = cancel.map(|c| c.child_token()).unwrap_or_default();
        let options = RunOptions {
            max_output_bytes: Some(0),
            max_error_bytes: Some(ERROR_BYTES),
            cancel: Some(stop.clone()),
            output_sink: Some(sender),
            kill_grace: Some(std::time::Duration::from_millis(200)),
            ..RunOptions::default()
        };
        let runner = self.runner.clone();
        let task = tokio::spawn(async move { runner.run(&command, options).await });
        let mut rows = Vec::new();
        let mut pending: Vec<u8> = Vec::new();
        let mut truncated = false;
        let mut parse_error = None;
        'read: while let Some(chunk) = receiver.recv().await {
            pending.extend(chunk);
            while let Some(index) = pending.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = pending.drain(..=index).collect();
                let line = String::from_utf8_lossy(&line);
                let line = line.trim_end_matches('\n').trim_end_matches('\r');
                if line.is_empty() {
                    continue;
                }
                let row = match parse(line) {
                    Ok(Some(row)) => row,
                    Ok(None) => continue,
                    Err(error) => {
                        parse_error = Some(error);
                        break 'read;
                    }
                };
                rows.push(row);
                if rows.len() > limit {
                    truncated = true;
                    break 'read;
                }
            }
        }
        if truncated || parse_error.is_some() {
            // Baseline `Stream.take(limit + 1)` (or a failed parse) closes the scope, killing rg.
            stop.cancel();
            drop(receiver);
            let _ = task.await;
            if let Some(error) = parse_error {
                return Err(error);
            }
            rows.truncate(limit);
            return Ok(rows);
        }
        let tail = String::from_utf8_lossy(&pending)
            .trim_end_matches('\r')
            .to_string();
        if !tail.is_empty()
            && let Some(row) = parse(&tail)?
        {
            rows.push(row);
        }
        let result = task
            .await
            .map_err(|error| failure(format!("ripgrep execution failed: {error}")))?
            .map_err(|error| failure(error.to_string()))?;
        let stderr = String::from_utf8_lossy(&result.stderr).into_owned();
        if let Some(pattern) = pattern
            && result.exit_code == 2
            && (stderr.contains("regex parse error") || stderr.contains("error parsing regex"))
        {
            return Err(RipgrepError::InvalidPattern {
                pattern: pattern.into(),
                message: stderr.trim().into(),
            });
        }
        if !matches!(result.exit_code, 0..=2) {
            let trimmed = stderr.trim();
            return Err(failure(if trimmed.is_empty() {
                format!("ripgrep failed with code {}", result.exit_code)
            } else {
                trimmed.to_string()
            }));
        }
        if result.exit_code == 1 {
            return Ok(Vec::new());
        }
        Ok(rows.into_iter().take(limit).collect())
    }

    fn file_args(pattern: Option<&str>, hidden: bool, follow: bool) -> Vec<String> {
        let mut args = vec!["--no-config".to_string(), "--files".into()];
        if hidden {
            args.push("--hidden".into());
        }
        if follow {
            args.push("--follow".into());
        }
        if let Some(pattern) = pattern {
            args.push(format!("--glob={pattern}"));
        }
        args.extend(["--glob=!**/.git/**".to_string(), ".".into()]);
        args
    }

    pub async fn glob(&self, input: GlobInput) -> Result<Vec<Entry>, RipgrepError> {
        let args = Self::file_args(Some(&input.pattern), input.hidden, input.follow);
        self.run(&input.cwd, args, input.limit, None, input.cancel, |row| {
            Ok(Some(entry(relative(row))))
        })
        .await
    }

    /// `find`: like glob, but `*` lists everything without a glob filter.
    pub async fn find(&self, input: GlobInput) -> Result<Vec<Entry>, RipgrepError> {
        let pattern = (input.pattern != "*").then_some(input.pattern.as_str());
        let args = Self::file_args(pattern, input.hidden, input.follow);
        self.run(&input.cwd, args, input.limit, None, input.cancel, |row| {
            Ok(Some(entry(relative(row))))
        })
        .await
    }

    pub async fn grep(&self, input: GrepInput) -> Result<Vec<Match>, RipgrepError> {
        let mut args: Vec<String> = ["--no-config", "--json", "--hidden", "--no-messages"]
            .map(String::from)
            .into();
        if let Some(include) = &input.include {
            args.push(format!("--glob={include}"));
        }
        args.extend(["--glob=!**/.git/**".to_string(), "--".into()]);
        args.push(input.pattern.clone());
        args.push(input.file.clone().unwrap_or_else(|| ".".into()));
        self.run(
            &input.cwd,
            args,
            input.limit,
            Some(&input.pattern),
            input.cancel,
            parse_match,
        )
        .await
    }
}

/// Truncate at 2,000 UTF-16 code units and drop a dangling high surrogate, then append `...`.
fn preview(text: &str) -> String {
    if text.encode_utf16().count() <= PREVIEW_UNITS {
        return text.to_string();
    }
    let mut units = 0;
    let mut out = String::new();
    for ch in text.chars() {
        let width = ch.len_utf16();
        if units + width > PREVIEW_UNITS {
            break;
        }
        units += width;
        out.push(ch);
    }
    out + "..."
}

fn parse_match(line: &str) -> Result<Option<Match>, RipgrepError> {
    if line.len() > MAX_RECORD_BYTES {
        return Err(failure(format!(
            "Ripgrep JSON record exceeded {MAX_RECORD_BYTES} bytes"
        )));
    }
    let json: Value =
        serde_json::from_str(line).map_err(|_| failure("Invalid ripgrep JSON output"))?;
    if json.get("type").and_then(Value::as_str) != Some("match") {
        return Ok(None);
    }
    let invalid = || failure("Invalid ripgrep match output");
    let data = json.get("data").ok_or_else(invalid)?;
    let text = |value: &Value| {
        value
            .get("text")
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    let path = data.get("path").and_then(text).ok_or_else(invalid)?;
    let lines = data.get("lines").and_then(text).ok_or_else(invalid)?;
    let line_number = data
        .get("line_number")
        .and_then(Value::as_i64)
        .and_then(PositiveInt::new)
        .ok_or_else(invalid)?;
    let offset = data
        .get("absolute_offset")
        .and_then(Value::as_i64)
        .and_then(NonNegativeInt::new)
        .ok_or_else(invalid)?;
    let mut submatches = Vec::new();
    for item in data
        .get("submatches")
        .and_then(Value::as_array)
        .ok_or_else(invalid)?
    {
        let bound = |key: &str| {
            item.get(key)
                .and_then(Value::as_i64)
                .and_then(NonNegativeInt::new)
        };
        submatches.push(Submatch {
            text: item.get("match").and_then(text).ok_or_else(invalid)?,
            start: bound("start").ok_or_else(invalid)?,
            end: bound("end").ok_or_else(invalid)?,
        });
    }
    submatches.truncate(MAX_SUBMATCHES);
    Ok(Some(Match {
        entry: entry(relative(&path)),
        line: line_number,
        offset,
        text: preview(&lines),
        submatches,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_strips_prefixes() {
        assert_eq!(relative("./a/b.ts"), "a/b.ts");
        assert_eq!(relative("././/x"), "x");
        assert_eq!(relative(".\\a\\b"), "a/b");
    }

    #[test]
    fn preview_keeps_surrogate_pairs_whole() {
        let text = format!("needle{}😀\n", "x".repeat(1_993));
        assert_eq!(preview(&text), format!("needle{}...", "x".repeat(1_993)));
        assert_eq!(preview("short"), "short");
    }
}
