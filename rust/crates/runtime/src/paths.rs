//! Purpose: `Global.Path` (packages/core/src/global.ts:11-31): XDG data, cache, config and state
//! directories under the `opencode` app name, plus derived bin, log and repos directories.
//! Scope: path computation and directory creation.
//! Uses:
//! `-- none
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! |-- runtime/src/lib.rs
//! `-- runtime/src/process.rs
//! Invariants: XDG variables that are unset or relative fall back to the `xdg-basedir` defaults
//! (`~/.local/share`, `~/.cache`, `~/.config`, `~/.local/state`); `home` honours
//! `OPENCODE_TEST_HOME`. Directory creation is explicit (`ensure`), never a side effect of
//! computing paths.
//! Contract tests: runtime/src/paths.rs (unit tests)

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const APP: &str = "opencode";

/// `Global.Interface`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobalPaths {
    pub home: PathBuf,
    pub data: PathBuf,
    pub cache: PathBuf,
    pub config: PathBuf,
    pub state: PathBuf,
    pub tmp: PathBuf,
    pub bin: PathBuf,
    pub log: PathBuf,
    pub repos: PathBuf,
}

impl GlobalPaths {
    /// Paths for the current process environment.
    pub fn from_process() -> Self {
        Self::from_vars(&std::env::vars().collect(), std::env::temp_dir())
    }

    /// Paths for an explicit environment (tests, embedded hosts with an isolated home).
    pub fn from_vars(vars: &BTreeMap<String, String>, temp: PathBuf) -> Self {
        let os_home = vars
            .get("HOME")
            .or_else(|| vars.get("USERPROFILE"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/"));
        let xdg = |key: &str, fallback: &[&str]| {
            vars.get(key)
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
                .unwrap_or_else(|| fallback.iter().fold(os_home.clone(), |p, s| p.join(s)))
                .join(APP)
        };
        let data = xdg("XDG_DATA_HOME", &[".local", "share"]);
        let cache = xdg("XDG_CACHE_HOME", &[".cache"]);
        Self {
            home: vars
                .get("OPENCODE_TEST_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| os_home.clone()),
            bin: cache.join("bin"),
            log: data.join("log"),
            repos: data.join("repos"),
            config: xdg("XDG_CONFIG_HOME", &[".config"]),
            state: xdg("XDG_STATE_HOME", &[".local", "state"]),
            tmp: temp.join(APP),
            data,
            cache,
        }
    }

    /// Every path rooted under `root` (embedded hosts and tests).
    pub fn rooted(root: &Path) -> Self {
        let vars = BTreeMap::from([
            ("HOME".to_owned(), root.join("home").display().to_string()),
            (
                "XDG_DATA_HOME".to_owned(),
                root.join("data").display().to_string(),
            ),
            (
                "XDG_CACHE_HOME".to_owned(),
                root.join("cache").display().to_string(),
            ),
            (
                "XDG_CONFIG_HOME".to_owned(),
                root.join("config").display().to_string(),
            ),
            (
                "XDG_STATE_HOME".to_owned(),
                root.join("state").display().to_string(),
            ),
        ]);
        Self::from_vars(&vars, root.join("tmp"))
    }

    /// The `fs.mkdir(..., { recursive: true })` block of `global.ts:35-43`.
    pub async fn ensure(&self) -> std::io::Result<()> {
        for dir in [
            &self.data,
            &self.config,
            &self.state,
            &self.tmp,
            &self.log,
            &self.bin,
            &self.repos,
        ] {
            tokio::fs::create_dir_all(dir).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn defaults_follow_xdg_basedir() {
        let paths = GlobalPaths::from_vars(&vars(&[("HOME", "/h")]), PathBuf::from("/t"));
        assert_eq!(paths.data, PathBuf::from("/h/.local/share/opencode"));
        assert_eq!(paths.cache, PathBuf::from("/h/.cache/opencode"));
        assert_eq!(paths.config, PathBuf::from("/h/.config/opencode"));
        assert_eq!(paths.state, PathBuf::from("/h/.local/state/opencode"));
        assert_eq!(paths.bin, PathBuf::from("/h/.cache/opencode/bin"));
        assert_eq!(paths.log, PathBuf::from("/h/.local/share/opencode/log"));
        assert_eq!(paths.repos, PathBuf::from("/h/.local/share/opencode/repos"));
        assert_eq!(paths.tmp, PathBuf::from("/t/opencode"));
        assert_eq!(paths.home, PathBuf::from("/h"));
    }

    #[test]
    fn absolute_xdg_overrides_and_relative_ones_are_ignored() {
        let paths = GlobalPaths::from_vars(
            &vars(&[
                ("HOME", "/h"),
                ("XDG_DATA_HOME", "/xd"),
                ("XDG_CONFIG_HOME", "relative"),
                ("OPENCODE_TEST_HOME", "/test-home"),
            ]),
            PathBuf::from("/t"),
        );
        assert_eq!(paths.data, PathBuf::from("/xd/opencode"));
        assert_eq!(paths.config, PathBuf::from("/h/.config/opencode"));
        assert_eq!(paths.home, PathBuf::from("/test-home"));
    }
}
