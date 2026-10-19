//! Purpose: port of jsdiff 8.0.2 `diffLines` and `createTwoFilesPatch` (default options), the
//! diff producer behind edit/write metadata and core FileDiff patches.
//! Scope: pure text diffing, no filesystem effects.
//! Uses:
//! `-- none
//! External:
//! |-- i64::MAX
//! `-- i64::MIN
//! Used by:
//! rust/crates/
//! |-- tools/src/files/edit.rs
//! |-- tools/src/files/edit_legacy.rs
//! |-- tools/src/files/mod.rs
//! |-- tools/src/files/write.rs
//! `-- tools/tests/files_edit.rs
//! Invariants: identical edit-path selection to jsdiff's Myers variant (diagonal pruning, add
//! preferred only when strictly farther), 4 context lines, `\ No newline at end of file` markers.
//! Contract tests: tools/tests/files_edit.rs (live_jsdiff_differential)

/// One jsdiff change object. `count` is in line tokens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub value: String,
    pub count: usize,
    pub added: bool,
    pub removed: bool,
}

#[derive(Clone, Copy)]
struct Component {
    count: usize,
    added: bool,
    removed: bool,
    previous: Option<usize>,
}

#[derive(Clone, Copy)]
struct PathState {
    old_pos: i64,
    last: Option<usize>,
}

/// `diffLines(old, new)`.
pub fn diff_lines(old: &str, new: &str) -> Vec<Change> {
    let old_tokens = tokenize(old);
    let new_tokens = tokenize(new);
    let mut arena: Vec<Component> = Vec::new();
    let components = myers(&old_tokens, &new_tokens, &mut arena);
    build_values(&components, &arena, &old_tokens, &new_tokens)
}

/// `(additions, deletions)` summed over `diffLines` change counts, as the tools compute them.
pub fn line_counts(old: &str, new: &str) -> (usize, usize) {
    diff_lines(old, new)
        .iter()
        .fold((0, 0), |(additions, deletions), change| {
            (
                additions + if change.added { change.count } else { 0 },
                deletions + if change.removed { change.count } else { 0 },
            )
        })
}

fn tokenize(value: &str) -> Vec<&str> {
    // `value.split(/(\n|\r\n)/)` then merge separators into the preceding line, dropping a final
    // empty part. `removeEmpty` then drops empty tokens.
    let mut tokens = Vec::new();
    let mut start = 0;
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let width = match bytes[index] {
            b'\n' => 1,
            b'\r' if bytes.get(index + 1) == Some(&b'\n') => 2,
            _ => {
                index += 1;
                continue;
            }
        };
        tokens.push(&value[start..index + width]);
        index += width;
        start = index;
    }
    if start < value.len() {
        tokens.push(&value[start..]);
    }
    tokens.retain(|token| !token.is_empty());
    tokens
}

fn myers(old: &[&str], new: &[&str], arena: &mut Vec<Component>) -> Option<usize> {
    let (old_len, new_len) = (old.len() as i64, new.len() as i64);
    let max_edit = old_len + new_len;
    let offset = max_edit + 1;
    let mut best: Vec<Option<PathState>> = vec![None; (2 * offset + 1) as usize];
    let slot = |diagonal: i64| (diagonal + offset) as usize;
    let mut seed = PathState {
        old_pos: -1,
        last: None,
    };
    let new_pos = extract_common(&mut seed, old, new, 0, arena);
    if seed.old_pos + 1 >= old_len && new_pos + 1 >= new_len {
        return seed.last;
    }
    best[slot(0)] = Some(seed);
    let (mut min_diagonal, mut max_diagonal) = (i64::MIN, i64::MAX);
    let mut edit = 1;
    while edit <= max_edit {
        let mut diagonal = min_diagonal.max(-edit);
        while diagonal <= max_diagonal.min(edit) {
            let remove = best[slot(diagonal - 1)].take();
            let add = best[slot(diagonal + 1)];
            let can_add = add.is_some_and(|add| {
                let pos = add.old_pos - diagonal;
                (0..new_len).contains(&pos)
            });
            let can_remove = remove.is_some_and(|remove| remove.old_pos + 1 < old_len);
            if !can_add && !can_remove {
                best[slot(diagonal)] = None;
                diagonal += 2;
                continue;
            }
            let mut base = match (add, remove) {
                (Some(add), Some(remove))
                    if can_remove && !(can_add && remove.old_pos < add.old_pos) =>
                {
                    add_to_path(remove, false, true, 1, arena)
                }
                (Some(add), _) if can_add => add_to_path(add, true, false, 0, arena),
                (_, Some(remove)) => add_to_path(remove, false, true, 1, arena),
                _ => unreachable!("one of add/remove is possible"),
            };
            let new_pos = extract_common(&mut base, old, new, diagonal, arena);
            if base.old_pos + 1 >= old_len && new_pos + 1 >= new_len {
                return base.last;
            }
            best[slot(diagonal)] = Some(base);
            if base.old_pos + 1 >= old_len {
                max_diagonal = max_diagonal.min(diagonal - 1);
            }
            if new_pos + 1 >= new_len {
                min_diagonal = min_diagonal.max(diagonal + 1);
            }
            diagonal += 2;
        }
        edit += 1;
    }
    None
}

fn add_to_path(
    path: PathState,
    added: bool,
    removed: bool,
    old_inc: i64,
    arena: &mut Vec<Component>,
) -> PathState {
    let component = match path.last.map(|index| arena[index]) {
        Some(last) if last.added == added && last.removed == removed => Component {
            count: last.count + 1,
            added,
            removed,
            previous: last.previous,
        },
        _ => Component {
            count: 1,
            added,
            removed,
            previous: path.last,
        },
    };
    arena.push(component);
    PathState {
        old_pos: path.old_pos + old_inc,
        last: Some(arena.len() - 1),
    }
}

fn extract_common(
    path: &mut PathState,
    old: &[&str],
    new: &[&str],
    diagonal: i64,
    arena: &mut Vec<Component>,
) -> i64 {
    let (old_len, new_len) = (old.len() as i64, new.len() as i64);
    let mut old_pos = path.old_pos;
    let mut new_pos = old_pos - diagonal;
    let mut common = 0;
    while new_pos + 1 < new_len
        && old_pos + 1 < old_len
        && old[(old_pos + 1) as usize] == new[(new_pos + 1) as usize]
    {
        new_pos += 1;
        old_pos += 1;
        common += 1;
    }
    if common > 0 {
        arena.push(Component {
            count: common,
            added: false,
            removed: false,
            previous: path.last,
        });
        path.last = Some(arena.len() - 1);
    }
    path.old_pos = old_pos;
    new_pos
}

fn build_values(
    last: &Option<usize>,
    arena: &[Component],
    old: &[&str],
    new: &[&str],
) -> Vec<Change> {
    let mut chain = Vec::new();
    let mut cursor = *last;
    while let Some(index) = cursor {
        chain.push(arena[index]);
        cursor = arena[index].previous;
    }
    chain.reverse();
    let (mut new_pos, mut old_pos) = (0, 0);
    chain
        .into_iter()
        .map(|component| {
            let value = if component.removed {
                let value = old[old_pos..old_pos + component.count].concat();
                old_pos += component.count;
                value
            } else {
                let value = new[new_pos..new_pos + component.count].concat();
                new_pos += component.count;
                if !component.added {
                    old_pos += component.count;
                }
                value
            };
            Change {
                value,
                count: component.count,
                added: component.added,
                removed: component.removed,
            }
        })
        .collect()
}

struct Hunk {
    old_start: usize,
    old_lines: usize,
    new_start: usize,
    new_lines: usize,
    lines: Vec<String>,
}

const CONTEXT: usize = 4;

/// `createTwoFilesPatch(oldName, newName, old, new)` with undefined headers and default context.
pub fn create_two_files_patch(old_name: &str, new_name: &str, old: &str, new: &str) -> String {
    let mut ret = Vec::new();
    if old_name == new_name {
        ret.push(format!("Index: {old_name}"));
    }
    ret.push("=".repeat(67));
    ret.push(format!("--- {old_name}"));
    ret.push(format!("+++ {new_name}"));
    for hunk in structured_hunks(old, new) {
        let old_start = if hunk.old_lines == 0 {
            hunk.old_start as i64 - 1
        } else {
            hunk.old_start as i64
        };
        let new_start = if hunk.new_lines == 0 {
            hunk.new_start as i64 - 1
        } else {
            hunk.new_start as i64
        };
        ret.push(format!(
            "@@ -{old_start},{} +{new_start},{} @@",
            hunk.old_lines, hunk.new_lines
        ));
        ret.extend(hunk.lines);
    }
    ret.join("\n") + "\n"
}

fn structured_hunks(old: &str, new: &str) -> Vec<Hunk> {
    let mut diff: Vec<(Change, Vec<String>)> = diff_lines(old, new)
        .into_iter()
        .map(|change| {
            let lines = split_lines(&change.value);
            (change, lines)
        })
        .collect();
    diff.push((
        Change {
            value: String::new(),
            count: 0,
            added: false,
            removed: false,
        },
        vec![],
    ));
    let mut hunks = Vec::new();
    let (mut old_range_start, mut new_range_start) = (0usize, 0usize);
    let mut range: Vec<String> = Vec::new();
    let (mut old_line, mut new_line) = (1usize, 1usize);
    for index in 0..diff.len() {
        let (current, lines) = &diff[index];
        if current.added || current.removed {
            if old_range_start == 0 {
                old_range_start = old_line;
                new_range_start = new_line;
                if index > 0 {
                    let previous = &diff[index - 1].1;
                    let skip = previous.len().saturating_sub(CONTEXT);
                    range = previous[skip..]
                        .iter()
                        .map(|line| format!(" {line}"))
                        .collect();
                    old_range_start -= range.len();
                    new_range_start -= range.len();
                }
            }
            let prefix = if current.added { '+' } else { '-' };
            range.extend(lines.iter().map(|line| format!("{prefix}{line}")));
            if current.added {
                new_line += lines.len();
            } else {
                old_line += lines.len();
            }
            continue;
        }
        if old_range_start != 0 {
            if lines.len() <= CONTEXT * 2 && index + 2 < diff.len() {
                range.extend(lines.iter().map(|line| format!(" {line}")));
            } else {
                let size = lines.len().min(CONTEXT);
                range.extend(lines[..size].iter().map(|line| format!(" {line}")));
                hunks.push(Hunk {
                    old_start: old_range_start,
                    old_lines: old_line - old_range_start + size,
                    new_start: new_range_start,
                    new_lines: new_line - new_range_start + size,
                    lines: std::mem::take(&mut range),
                });
                old_range_start = 0;
                new_range_start = 0;
            }
        }
        old_line += lines.len();
        new_line += lines.len();
    }
    for hunk in &mut hunks {
        let mut lines = Vec::with_capacity(hunk.lines.len());
        for line in hunk.lines.drain(..) {
            match line.strip_suffix('\n') {
                Some(stripped) => lines.push(stripped.to_string()),
                None => {
                    lines.push(line);
                    lines.push("\\ No newline at end of file".into());
                }
            }
        }
        hunk.lines = lines;
    }
    hunks
}

/// jsdiff `splitLines`: lines keep their trailing `\n` where present.
fn split_lines(text: &str) -> Vec<String> {
    let mut result: Vec<String> = text.split('\n').map(|line| format!("{line}\n")).collect();
    if text.ends_with('\n') {
        result.pop();
    } else if let Some(last) = result.last_mut() {
        last.pop();
    }
    result
}
