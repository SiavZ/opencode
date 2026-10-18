//! Purpose: pure path helpers shared by mutation resolution, permissions and tools
//! (slash normalization, lexical containment, lexical normalization).
//! Scope: string/Path computations, no filesystem access.
//! Uses:
//! `-- none
//! External:
//! `-- std::path::{Component, Path, PathBuf}
//! Used by:
//! rust/crates/
//! `-- platform-api/src/lib.rs
//! Invariants: `contains` matches baseline `FSUtil.contains` (lexical, relative path does not start
//! with `..` and is not absolute).
//! Contract tests: platform-api/src/path.rs

use std::path::{Component, Path, PathBuf};

/// Replace backslashes with forward slashes, like the baseline `slash` helper.
pub fn slash(value: &str) -> String {
    value.replace('\\', "/")
}

/// Lexically normalize `.` and `..` components without touching the filesystem
/// (equivalent of node `path.resolve` on an already absolute path).
pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push(component.as_os_str());
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// `path.resolve(base, input)`: absolute inputs win, relative inputs join `base`.
pub fn resolve(base: &Path, input: &Path) -> PathBuf {
    if input.is_absolute() {
        return normalize(input);
    }
    normalize(&base.join(input))
}

/// Lexical containment: `child` equals `parent` or lies below it.
pub fn contains(parent: &Path, child: &Path) -> bool {
    let parent = normalize(parent);
    let child = normalize(child);
    child.starts_with(&parent)
}

/// `path.relative(from, to)` for absolute normalized paths, using `/` separators.
pub fn relative(from: &Path, to: &Path) -> String {
    let (from, to) = (normalize(from), normalize(to));
    let from: Vec<_> = from.components().collect();
    let to: Vec<_> = to.components().collect();
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let ups = std::iter::repeat_n("..".to_string(), from.len() - common);
    let downs = to[common..]
        .iter()
        .map(|c| c.as_os_str().to_string_lossy().into_owned());
    ups.chain(downs).collect::<Vec<_>>().join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn containment_is_lexical() {
        assert!(contains(Path::new("/a/b"), Path::new("/a/b")));
        assert!(contains(Path::new("/a/b"), Path::new("/a/b/c/../d")));
        assert!(!contains(Path::new("/a/b"), Path::new("/a/bc")));
        assert!(!contains(Path::new("/a/b"), Path::new("/a/b/../c")));
    }

    #[test]
    fn relative_and_resolve() {
        assert_eq!(relative(Path::new("/a/b"), Path::new("/a/b/c/d")), "c/d");
        assert_eq!(relative(Path::new("/a/b"), Path::new("/a/x")), "../x");
        assert_eq!(relative(Path::new("/a/b"), Path::new("/a/b")), "");
        assert_eq!(
            resolve(Path::new("/a/b"), Path::new("../c")),
            PathBuf::from("/a/c")
        );
        assert_eq!(slash("a\\b"), "a/b");
    }
}
