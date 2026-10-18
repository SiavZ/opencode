//! Purpose: path normalization of packages/core/src/database/path.ts (storagePath, absolute, toPlatform).
//! Scope: pure string functions, platform selected at compile time like `process.platform`.
//! Uses:
//! `-- none
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! |-- storage-api/src/records/codec.rs
//! `-- storage-api/src/records/mod.rs
//! Invariants: on posix values are stored verbatim and "absolute" means `path.posix.isAbsolute`
//! (leading `/`). On windows `\` is stored as `/`, drive (`C:/`) and UNC (`//`) paths are accepted
//! and converted back to `\` when read. Errors carry the TS message `Path is not absolute: <input>`.
//! Contract tests: storage-api/src/records/path.rs

/// `storagePath`: backslashes become `/` on windows, identity elsewhere.
pub fn storage_path(input: &str) -> String {
    storage_path_for(input, cfg!(windows))
}

/// `absolute`: normalize and require an absolute path.
pub fn absolute(input: &str) -> Result<String, String> {
    absolute_for(input, cfg!(windows))
}

/// `fromDriver` of `absoluteColumn`: validate then convert to the platform form.
pub fn from_driver(input: &str) -> Result<String, String> {
    let windows = cfg!(windows);
    absolute_for(input, windows).map(|value| to_platform_for(&value, windows))
}

fn storage_path_for(input: &str, windows: bool) -> String {
    if windows {
        return input.replace('\\', "/");
    }
    input.to_owned()
}

fn is_windows_storage_path(input: &str) -> bool {
    let bytes = input.as_bytes();
    let drive =
        bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'/';
    drive || input.starts_with("//")
}

fn absolute_for(input: &str, windows: bool) -> Result<String, String> {
    let result = storage_path_for(input, windows);
    if !result.starts_with('/') && !(windows && is_windows_storage_path(&result)) {
        return Err(format!("Path is not absolute: {input}"));
    }
    Ok(result)
}

fn to_platform_for(input: &str, windows: bool) -> String {
    if !windows || !is_windows_storage_path(input) {
        return input.to_owned();
    }
    input.replace('/', "\\")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn posix_rules() {
        assert_eq!(absolute_for("/a/b", false).unwrap(), "/a/b");
        assert_eq!(
            absolute_for("a/b", false).unwrap_err(),
            "Path is not absolute: a/b"
        );
        assert!(absolute_for("C:\\x", false).is_err());
        assert_eq!(storage_path_for("a\\b", false), "a\\b");
        assert_eq!(to_platform_for("C:/x", false), "C:/x");
    }

    #[test]
    fn windows_rules() {
        assert_eq!(absolute_for("C:\\repo\\x", true).unwrap(), "C:/repo/x");
        assert_eq!(absolute_for("\\\\srv\\share", true).unwrap(), "//srv/share");
        assert_eq!(absolute_for("/posix", true).unwrap(), "/posix");
        assert!(absolute_for("repo\\x", true).is_err());
        assert_eq!(to_platform_for("C:/repo/x", true), "C:\\repo\\x");
        assert_eq!(to_platform_for("/posix/x", true), "/posix/x");
    }
}
