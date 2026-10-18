//! Purpose: comment-preserving JSONC edits ported from jsonc-parser 3.3.1 `impl/edit.js`
//! (`modify` + `applyEdits` with formatting), used by secret-safe config writes.
//! Scope: pure text edits.
//! Uses:
//! rust/crates/
//! |-- config/src/jsonc/format.rs
//! `-- config/src/jsonc/visit.rs
//! External:
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! `-- config/src/jsonc/mod.rs
//! Invariants: only property paths (object keys) are supported, matching `patchJsonc` callers;
//! deleting from an empty document is a typed error like the baseline throw.
//! Contract tests: config/tests/jsonc.rs

use super::format::{Edit, Options, format, is_eol};
use super::visit::{NodeKind, parse_tree};
use serde_json::{Map, Value};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EditError {
    #[error("Can not delete in empty document")]
    DeleteInEmptyDocument,
    #[error("Can not add property to parent of type {0}")]
    InvalidParent(&'static str),
}

/// `modify(text, path, value, { formattingOptions: { insertSpaces: true, tabSize: 2 } })` then
/// `applyEdits`. `value: None` removes the property.
pub fn modify(text: &str, path: &[String], value: Option<&Value>) -> Result<String, EditError> {
    let units: Vec<u16> = text.encode_utf16().collect();
    let edits = set_property(&units, path, value.cloned())?;
    Ok(String::from_utf16_lossy(&apply(&units, &edits)))
}

fn set_property(
    text: &[u16],
    original: &[String],
    value: Option<Value>,
) -> Result<Vec<Edit>, EditError> {
    let mut path = original.to_vec();
    let mut errors = Vec::new();
    let root = parse_tree(text, &mut errors, false);
    let mut value = value;
    let mut last: Option<String> = None;
    let mut parent_path: Option<Vec<String>> = None;
    while let Some(segment) = path.pop() {
        let found = root.as_ref().and_then(|root| find(root, &path)).is_some();
        last = Some(segment.clone());
        if !found && value.is_some() {
            let mut wrapped = Map::new();
            wrapped.insert(segment, value.take().unwrap_or(Value::Null));
            value = Some(Value::Object(wrapped));
            continue;
        }
        if found {
            parent_path = Some(path.clone());
        }
        break;
    }
    let parent = parent_path
        .as_ref()
        .and_then(|path| root.as_ref().and_then(|root| find(root, path)));
    let Some(parent) = parent else {
        let Some(value) = value else {
            return Err(EditError::DeleteInEmptyDocument);
        };
        let (offset, length) = root
            .as_ref()
            .map(|root| (root.offset, root.length))
            .unwrap_or((0, 0));
        return Ok(with_formatting(
            text,
            Edit {
                offset,
                length,
                content: stringify(&value),
            },
        ));
    };
    let Some(last) = last else {
        return Err(EditError::InvalidParent("root"));
    };
    if parent.kind != NodeKind::Object {
        return Err(EditError::InvalidParent(
            if parent.kind == NodeKind::Array {
                "array"
            } else {
                "value"
            },
        ));
    }
    let existing = parent.children.iter().position(|property| {
        property.children.len() == 2 && property.children[0].value.as_str() == Some(&last)
    });
    if let Some(index) = existing {
        let property = &parent.children[index];
        let Some(value) = value else {
            let mut remove_end = property.offset + property.length;
            let remove_begin = if index > 0 {
                let previous = &parent.children[index - 1];
                previous.offset + previous.length
            } else {
                if parent.children.len() > 1 {
                    remove_end = parent.children[1].offset;
                }
                parent.offset + 1
            };
            return Ok(with_formatting(
                text,
                Edit {
                    offset: remove_begin,
                    length: remove_end - remove_begin,
                    content: Vec::new(),
                },
            ));
        };
        let node = &property.children[1];
        return Ok(with_formatting(
            text,
            Edit {
                offset: node.offset,
                length: node.length,
                content: stringify(&value),
            },
        ));
    }
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let mut content = stringify(&Value::String(last));
    content.extend(": ".encode_utf16());
    content.extend(stringify(&value));
    let edit = match parent.children.last() {
        Some(previous) => {
            let mut with_comma: Vec<u16> = vec![44];
            with_comma.extend(content);
            Edit {
                offset: previous.offset + previous.length,
                length: 0,
                content: with_comma,
            }
        }
        None => Edit {
            offset: parent.offset + 1,
            length: 0,
            content,
        },
    };
    Ok(with_formatting(text, edit))
}

fn find<'n>(root: &'n super::visit::Node, path: &[String]) -> Option<&'n super::visit::Node> {
    path.iter()
        .try_fold(root, |node, segment| node.find(segment))
}

fn stringify(value: &Value) -> Vec<u16> {
    serde_json::to_string(value)
        .unwrap_or_default()
        .encode_utf16()
        .collect()
}

fn apply_one(text: &[u16], edit: &Edit) -> Vec<u16> {
    let mut out = text[..edit.offset].to_vec();
    out.extend_from_slice(&edit.content);
    out.extend_from_slice(&text[edit.offset + edit.length..]);
    out
}

fn with_formatting(text: &[u16], edit: Edit) -> Vec<Edit> {
    let mut next = apply_one(text, &edit);
    let mut begin = edit.offset;
    let mut end = edit.offset + edit.content.len();
    if edit.length == 0 || edit.content.is_empty() {
        while begin > 0 && !is_eol(&next, begin - 1) {
            begin -= 1;
        }
        while end < next.len() && !is_eol(&next, end) {
            end += 1;
        }
    }
    let options = Options {
        tab_size: 2,
        insert_spaces: true,
        keep_lines: false,
        insert_final_newline: false,
    };
    let edits = format(&next, (begin, end - begin), options);
    for edit in edits.iter().rev() {
        next = apply_one(&next, edit);
        begin = begin.min(edit.offset);
        end = end.max(edit.offset + edit.length);
        end = (end + edit.content.len()).saturating_sub(edit.length);
    }
    let length = text.len() + end - next.len() - begin;
    vec![Edit {
        offset: begin,
        length,
        content: next[begin..end].to_vec(),
    }]
}

/// `applyEdits` for non-overlapping edits.
pub fn apply(text: &[u16], edits: &[Edit]) -> Vec<u16> {
    let mut sorted = edits.to_vec();
    sorted.sort_by(|a, b| a.offset.cmp(&b.offset).then(a.length.cmp(&b.length)));
    sorted
        .iter()
        .rev()
        .fold(text.to_vec(), |text, edit| apply_one(&text, edit))
}
