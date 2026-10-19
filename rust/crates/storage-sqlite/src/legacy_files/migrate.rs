//! Purpose: the legacy JSON store's one-time file migrations, port of
//! packages/opencode/src/storage/storage.ts:81-213 plus the marker loop at :226-245.
//! Scope: blocking file I/O on a worker thread, `git` subprocess for migration 1.
//! Uses:
//! rust/crates/
//! `-- storage-sqlite/src/legacy_files.rs
//! External:
//! |-- i64::MAX
//! |-- i64::try_from
//! |-- serde_json::Map
//! |-- serde_json::Value
//! |-- serde_json::from_str
//! |-- serde_json::json
//! |-- str::trim
//! |-- usize::MAX
//! `-- usize::try_from
//! Used by:
//! rust/crates/
//! |-- storage-sqlite/src/legacy_files.rs
//! `-- storage-sqlite/src/legacy_files/store.rs
//! Invariants: the journal is the text file `<data>/storage/migration` holding the count of completed
//! steps, separate from the SQL migration journal. It is parsed like `parseInt` (invalid text or a
//! missing file means 0, negative values are clamped to 0 where TS would crash). Steps run in order from
//! the marker. A failing step is logged to stderr and stops the loop without advancing the marker.
//! Failures are unreadable or invalid JSON and a `git` binary that cannot be spawned. A git command
//! that fails is not a failure: it yields no root commit and the project is skipped, like `Git.run`.
//! Migration 1 copies `<data>/project/<dir>/storage/session/{info,message,part}` into the new layout
//! keyed by the worktree's sorted first root commit. The `global` project and projects without a
//! resolvable worktree are skipped. Migration 2 splits `summary.diffs` out of every session file into
//! `session_diff/<id>.json` and replaces `summary` with totals.
//! Contract tests: storage-sqlite/tests/legacy_json.rs

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value};

use super::{js_normalize, js_pretty};

type Step = fn(&Path, &Path) -> Result<(), String>;

const MIGRATIONS: [Step; 2] = [migration_1, migration_2];

/// Run pending migrations for `<data>/storage`. Errors only when the marker cannot be written.
pub(crate) fn run(data: &Path) -> Result<(), String> {
    let dir = data.join("storage");
    let marker = dir.join("migration");
    let start = std::fs::read_to_string(&marker)
        .map(|text| parse_int(&text))
        .unwrap_or(0)
        .max(0);
    let skip = usize::try_from(start).unwrap_or(usize::MAX);
    for (index, step) in MIGRATIONS.iter().enumerate().skip(skip) {
        if let Err(error) = step(data, &dir) {
            eprintln!("failed to run migration {index}: {error}");
            break;
        }
        write(&marker, &(index + 1).to_string())?;
    }
    Ok(())
}

/// `Number.parseInt(text, 10)` with NaN mapped to 0.
fn parse_int(text: &str) -> i64 {
    let trimmed = text.trim_start();
    let (sign, digits) = match trimmed.as_bytes().first() {
        Some(b'-') => (-1, &trimmed[1..]),
        Some(b'+') => (1, &trimmed[1..]),
        _ => (1, trimmed),
    };
    let end = digits
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(digits.len());
    digits[..end].parse::<i64>().map_or(0, |v| sign * v)
}

fn write(path: &Path, content: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    std::fs::write(path, content).map_err(|e| format!("{}: {e}", path.display()))
}

fn read_json(path: &Path) -> Result<Value, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// Non-dot entries of `dir` sorted by name. `files` keeps regular files only (glob `nodir`).
fn entries(dir: &Path, files: bool) -> Vec<PathBuf> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = read
        .flatten()
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .map(|e| e.path())
        .filter(|p| !files || p.is_file())
        .collect();
    out.sort();
    out
}

/// `glob("<dir>/*.json")` (files only).
fn json_files(dir: &Path) -> Vec<PathBuf> {
    entries(dir, true)
        .into_iter()
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect()
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

fn string_field<'a>(value: &'a Value, field: &str) -> Option<&'a str> {
    value.as_object()?.get(field)?.as_str()
}

/// Worktree root from the first legacy message with `path.root` (decodeRoot).
fn legacy_root(full: &Path) -> Result<Option<String>, String> {
    for session in entries(&full.join("storage/session/message"), false) {
        if !session.is_dir() {
            continue;
        }
        for file in json_files(&session) {
            let json = read_json(&file)?;
            let Some(object) = json.as_object() else {
                continue;
            };
            let root = match object.get("path") {
                None => None,
                Some(Value::Object(path)) => match path.get("root") {
                    None => None,
                    Some(Value::String(root)) => Some(root.clone()),
                    Some(_) => continue,
                },
                Some(_) => continue,
            };
            if let Some(root) = root.filter(|r| !r.is_empty()) {
                return Ok(Some(root));
            }
        }
    }
    Ok(None)
}

/// `git rev-list --max-parents=0 --all`, sorted, first line. Spawn failure is an error.
fn root_commit(worktree: &str) -> Result<Option<String>, String> {
    let output = Command::new("git")
        .args([
            "--no-optional-locks",
            "-c",
            "core.autocrlf=false",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.longpaths=true",
            "-c",
            "core.symlinks=true",
            "-c",
            "core.quotepath=false",
            "rev-list",
            "--max-parents=0",
            "--all",
        ])
        .current_dir(worktree)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("git: {e}"))?;
    let text = String::from_utf8_lossy(&output.stdout);
    let mut ids: Vec<&str> = text
        .split('\n')
        .filter(|l| !l.is_empty())
        .map(str::trim)
        .collect();
    ids.sort_unstable();
    Ok(ids.first().map(|id| (*id).to_owned()))
}

fn migration_1(data: &Path, dir: &Path) -> Result<(), String> {
    let project = data.join("project");
    if !project.is_dir() {
        return Ok(());
    }
    for full in entries(&project, false) {
        if !full.is_dir() {
            continue;
        }
        let name = full
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if name == "global" {
            continue;
        }
        let worktree = legacy_root(&full)?.unwrap_or_else(|| "/".to_owned());
        if !Path::new(&worktree).is_dir() {
            continue;
        }
        let Some(id) = root_commit(&worktree)? else {
            continue;
        };
        let now = now_ms();
        let info = serde_json::json!({
            "id": id,
            "vcs": "git",
            "worktree": worktree,
            "time": { "created": now, "initialized": now },
        });
        write(
            &dir.join("project").join(format!("{id}.json")),
            &js_pretty(&info),
        )?;
        for session_file in json_files(&full.join("storage/session/info")) {
            let file_name = session_file.file_name().unwrap_or_default();
            let session = read_json(&session_file)?;
            write(
                &dir.join("session").join(&id).join(file_name),
                &js_pretty(&session),
            )?;
            let Some(session_id) = string_field(&session, "id") else {
                continue;
            };
            for message_file in json_files(&full.join("storage/session/message").join(session_id)) {
                let message = read_json(&message_file)?;
                let target = dir
                    .join("message")
                    .join(session_id)
                    .join(message_file.file_name().unwrap_or_default());
                write(&target, &js_pretty(&message))?;
                let Some(message_id) = string_field(&message, "id") else {
                    continue;
                };
                let parts = full
                    .join("storage/session/part")
                    .join(session_id)
                    .join(message_id);
                for part_file in json_files(&parts) {
                    let part = read_json(&part_file)?;
                    let target = dir
                        .join("part")
                        .join(message_id)
                        .join(part_file.file_name().unwrap_or_default());
                    write(&target, &js_pretty(&part))?;
                }
            }
        }
    }
    Ok(())
}

/// `NonNegativeInt` after JSON.parse (integral floats are integers in JS).
fn non_negative_int(value: &Value) -> Option<i64> {
    match js_normalize(value) {
        Value::Number(n) => n.as_i64().filter(|v| *v >= 0),
        _ => None,
    }
}

/// Decode `DiffFile` with excess properties preserved: Effect Schema emits excess keys first (in input
/// order), then the declared `additions`, `deletions`.
fn decode_diff(value: &Value) -> Option<(Value, i64, i64)> {
    let object = value.as_object()?;
    let additions = non_negative_int(object.get("additions")?)?;
    let deletions = non_negative_int(object.get("deletions")?)?;
    let mut out: Map<String, Value> = object
        .iter()
        .filter(|(k, _)| *k != "additions" && *k != "deletions")
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    out.insert("additions".into(), additions.into());
    out.insert("deletions".into(), deletions.into());
    Some((Value::Object(out), additions, deletions))
}

fn migration_2(_data: &Path, dir: &Path) -> Result<(), String> {
    for project in entries(&dir.join("session"), false) {
        if !project.is_dir() {
            continue;
        }
        for item in json_files(&project) {
            let raw = read_json(&item)?;
            let Some(object) = raw.as_object() else {
                continue;
            };
            let (Some(id), Some(project_id)) = (
                object.get("id").and_then(Value::as_str),
                object.get("projectID").and_then(Value::as_str),
            ) else {
                continue;
            };
            let Some(diffs) = object
                .get("summary")
                .and_then(Value::as_object)
                .and_then(|s| s.get("diffs"))
                .and_then(Value::as_array)
            else {
                continue;
            };
            let Some(decoded) = diffs.iter().map(decode_diff).collect::<Option<Vec<_>>>() else {
                continue;
            };
            let additions: i64 = decoded.iter().map(|d| d.1).sum();
            let deletions: i64 = decoded.iter().map(|d| d.2).sum();
            let diffs = Value::Array(decoded.into_iter().map(|d| d.0).collect());
            write(
                &dir.join("session_diff").join(format!("{id}.json")),
                &js_pretty(&diffs),
            )?;
            let mut next = object.clone();
            next.insert(
                "summary".into(),
                serde_json::json!({ "additions": additions, "deletions": deletions }),
            );
            write(
                &dir.join("session")
                    .join(project_id)
                    .join(format!("{id}.json")),
                &js_pretty(&Value::Object(next)),
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_int_like_js() {
        assert_eq!(parse_int("2"), 2);
        assert_eq!(parse_int("  7x"), 7);
        assert_eq!(parse_int("wat"), 0);
        assert_eq!(parse_int("-1"), -1);
        assert_eq!(parse_int("+3"), 3);
        assert_eq!(parse_int(""), 0);
    }
}
