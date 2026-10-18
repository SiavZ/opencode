//! Purpose: generation-specific hunk parsers between `*** Begin Patch` and `*** End Patch`.
//! Scope: line-level syntax only, markers are located by the caller.
//! Uses:
//! rust/crates/
//! `-- tools/src/files/patch.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! `-- tools/src/files/patch.rs
//! Invariants: Core throws on unknown lines, empty paths, `+`-less add lines and chunk-less updates.
//! Legacy skips unknown lines, keeps empty move targets, stops chunk bodies at any `***` line (so
//! `*** End of File` is never recorded) and ignores unprefixed chunk lines.
//! Contract tests: tools/tests/files_patch.rs

use super::{Hunk, PatchError, UpdateFileChunk, js_trim};

const ADD: &str = "*** Add File:";
const DELETE: &str = "*** Delete File:";
const UPDATE: &str = "*** Update File:";
const MOVE: &str = "*** Move to:";

pub(super) fn core(lines: &[&str], begin: usize, end: usize) -> Result<Vec<Hunk>, PatchError> {
    let mut hunks = Vec::new();
    let mut index = begin + 1;
    while index < end {
        let line = lines[index];
        if let Some(rest) = line.strip_prefix(ADD) {
            let path = required(rest, "Invalid add file path")?;
            let mut contents = Vec::new();
            index += 1;
            while index < lines.len() && !lines[index].starts_with("***") {
                let Some(content) = lines[index].strip_prefix('+') else {
                    return Err(PatchError::new(format!(
                        "Invalid add file line: {}",
                        lines[index]
                    )));
                };
                contents.push(content);
                index += 1;
            }
            hunks.push(Hunk::Add {
                path,
                contents: contents.join("\n"),
            });
            continue;
        }
        if let Some(rest) = line.strip_prefix(DELETE) {
            let path = required(rest, "Invalid delete file path")?;
            hunks.push(Hunk::Delete { path });
            index += 1;
            continue;
        }
        if let Some(rest) = line.strip_prefix(UPDATE) {
            let path = required(rest, "Invalid update file path")?;
            index += 1;
            let move_path = match lines.get(index).and_then(|l| l.strip_prefix(MOVE)) {
                Some(rest) => {
                    index += 1;
                    Some(required(rest, "Invalid move file path")?)
                }
                None => None,
            };
            let chunks = core_chunks(lines, &mut index)?;
            if chunks.is_empty() {
                return Err(PatchError::new(format!(
                    "Invalid update hunk for {path}: expected at least one @@ chunk"
                )));
            }
            hunks.push(Hunk::Update {
                path,
                move_path,
                chunks,
            });
            continue;
        }
        return Err(PatchError::new(format!("Invalid patch line: {line}")));
    }
    Ok(hunks)
}

fn core_chunks(lines: &[&str], index: &mut usize) -> Result<Vec<UpdateFileChunk>, PatchError> {
    let mut chunks = Vec::new();
    while *index < lines.len() && !lines[*index].starts_with("***") {
        let Some(header) = lines[*index].strip_prefix("@@") else {
            return Err(PatchError::new(format!(
                "Invalid update file line: {}",
                lines[*index]
            )));
        };
        let mut chunk = UpdateFileChunk {
            change_context: context(header),
            ..UpdateFileChunk::default()
        };
        *index += 1;
        while *index < lines.len() && !lines[*index].starts_with("@@") {
            let line = lines[*index];
            if line == "*** End of File" {
                chunk.end_of_file = true;
                *index += 1;
                break;
            }
            if line.starts_with("***") {
                break;
            }
            if !body_line(&mut chunk, line) {
                return Err(PatchError::new(format!(
                    "Invalid update chunk line: {line}"
                )));
            }
            *index += 1;
        }
        chunks.push(chunk);
    }
    Ok(chunks)
}

pub(super) fn legacy(lines: &[&str], begin: usize, end: usize) -> Vec<Hunk> {
    let mut hunks = Vec::new();
    let mut index = begin + 1;
    while index < end {
        let line = lines[index];
        if let Some(rest) = line.strip_prefix(ADD) {
            let path = js_trim(rest);
            index += 1;
            if path.is_empty() {
                continue;
            }
            let mut contents = String::new();
            while index < lines.len() && !lines[index].starts_with("***") {
                if let Some(content) = lines[index].strip_prefix('+') {
                    contents.push_str(content);
                    contents.push('\n');
                }
                index += 1;
            }
            if contents.ends_with('\n') {
                contents.pop();
            }
            hunks.push(Hunk::Add {
                path: path.into(),
                contents,
            });
            continue;
        }
        if let Some(rest) = line.strip_prefix(DELETE) {
            let path = js_trim(rest);
            index += 1;
            if !path.is_empty() {
                hunks.push(Hunk::Delete { path: path.into() });
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix(UPDATE) {
            let path = js_trim(rest);
            let mut next = index + 1;
            let move_path = lines.get(next).and_then(|l| l.strip_prefix(MOVE)).map(|m| {
                next += 1;
                js_trim(m).to_string()
            });
            if path.is_empty() {
                index += 1;
                continue;
            }
            index = next;
            let chunks = legacy_chunks(lines, &mut index);
            hunks.push(Hunk::Update {
                path: path.into(),
                move_path,
                chunks,
            });
            continue;
        }
        index += 1;
    }
    hunks
}

fn legacy_chunks(lines: &[&str], index: &mut usize) -> Vec<UpdateFileChunk> {
    let mut chunks = Vec::new();
    while *index < lines.len() && !lines[*index].starts_with("***") {
        let Some(header) = lines[*index].strip_prefix("@@") else {
            *index += 1;
            continue;
        };
        let mut chunk = UpdateFileChunk {
            change_context: context(header),
            ..UpdateFileChunk::default()
        };
        *index += 1;
        // Baseline checks `*** End of File` after the `***` loop guard, so the flag is never set.
        while *index < lines.len()
            && !lines[*index].starts_with("@@")
            && !lines[*index].starts_with("***")
        {
            body_line(&mut chunk, lines[*index]);
            *index += 1;
        }
        chunks.push(chunk);
    }
    chunks
}

fn body_line(chunk: &mut UpdateFileChunk, line: &str) -> bool {
    if let Some(keep) = line.strip_prefix(' ') {
        chunk.old_lines.push(keep.into());
        chunk.new_lines.push(keep.into());
        return true;
    }
    if let Some(removed) = line.strip_prefix('-') {
        chunk.old_lines.push(removed.into());
        return true;
    }
    if let Some(added) = line.strip_prefix('+') {
        chunk.new_lines.push(added.into());
        return true;
    }
    false
}

fn context(header: &str) -> Option<String> {
    Some(js_trim(header))
        .filter(|c| !c.is_empty())
        .map(Into::into)
}

fn required(rest: &str, message: &str) -> Result<String, PatchError> {
    let path = js_trim(rest);
    if path.is_empty() {
        return Err(PatchError::new(message));
    }
    Ok(path.into())
}
