//! Purpose: process environment snapshot for configuration: env vars, flags, home, global and
//! managed config directories, username and platform. Tests build it explicitly, so no test
//! ever reads the real HOME or XDG directories.
//! Scope: value + std-backed constructor.
//! Uses:
//! `-- none
//! External:
//! `-- std::env
//! Used by:
//! rust/crates/
//! |-- config/src/current.rs
//! |-- config/src/legacy/load.rs
//! |-- config/src/lib.rs
//! `-- config/src/variable.rs
//! Invariants: flag parsing matches `packages/core/src/flag/flag.ts` (`truthy` = "true" | "1",
//! case-insensitive); managed dir honours `OPENCODE_TEST_MANAGED_CONFIG_DIR` like `managed.ts:31`.
//! Contract tests: config/tests/legacy_load.rs

use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    MacOs,
    Windows,
    Other,
}

impl Platform {
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::MacOs
        } else if cfg!(windows) {
            Self::Windows
        } else {
            Self::Other
        }
    }
}

#[derive(Debug, Clone)]
pub struct Environment {
    pub vars: BTreeMap<String, String>,
    /// `Global.Path.home` (`OPENCODE_TEST_HOME` or the OS home directory).
    pub home: PathBuf,
    /// `Global.Path.config` (`$XDG_CONFIG_HOME/opencode`).
    pub global_config: PathBuf,
    /// `os.userInfo().username`; `None` models the failure path that falls back to `"user"`.
    pub username: Option<String>,
    pub platform: Platform,
}

impl Environment {
    /// Snapshot the real process environment (production only; tests construct `Environment`).
    pub fn from_process() -> Self {
        let vars: BTreeMap<String, String> = std::env::vars().collect();
        let home = vars
            .get("OPENCODE_TEST_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                vars.get("HOME")
                    .or_else(|| vars.get("USERPROFILE"))
                    .map(PathBuf::from)
            })
            .unwrap_or_else(|| PathBuf::from("/"));
        let xdg = vars
            .get("XDG_CONFIG_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"));
        let username = vars
            .get("USER")
            .or_else(|| vars.get("USERNAME"))
            .cloned()
            .filter(|name| !name.is_empty());
        Self {
            vars,
            home,
            global_config: xdg.join("opencode"),
            username,
            platform: Platform::current(),
        }
    }

    /// Hermetic environment rooted at `home`, used by tests and embedders.
    pub fn isolated(home: impl Into<PathBuf>) -> Self {
        let home = home.into();
        Self {
            vars: BTreeMap::new(),
            global_config: home.join(".config").join("opencode"),
            home,
            username: Some("testuser".into()),
            platform: Platform::current(),
        }
    }

    pub fn var(&self, key: &str) -> Option<&str> {
        self.vars.get(key).map(String::as_str)
    }

    pub fn set_var(&mut self, key: &str, value: impl Into<String>) -> &mut Self {
        self.vars.insert(key.into(), value.into());
        self
    }

    /// `Flag.truthy`.
    pub fn truthy(&self, key: &str) -> bool {
        self.var(key)
            .map(str::to_lowercase)
            .is_some_and(|value| value == "true" || value == "1")
    }

    pub fn opencode_config(&self) -> Option<PathBuf> {
        self.var("OPENCODE_CONFIG")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    }

    pub fn opencode_config_dir(&self) -> Option<PathBuf> {
        self.var("OPENCODE_CONFIG_DIR")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    }

    pub fn opencode_config_content(&self) -> Option<&str> {
        self.var("OPENCODE_CONFIG_CONTENT")
            .filter(|value| !value.is_empty())
    }

    pub fn disable_project_config(&self) -> bool {
        self.truthy("OPENCODE_DISABLE_PROJECT_CONFIG")
    }

    /// `Global.make().config`: `OPENCODE_CONFIG_DIR` replaces the global config dir for the
    /// current (V2) config service.
    pub fn current_global_config(&self) -> PathBuf {
        self.opencode_config_dir()
            .unwrap_or_else(|| self.global_config.clone())
    }

    /// `ConfigManaged.managedConfigDir()`.
    pub fn managed_config_dir(&self) -> PathBuf {
        if let Some(dir) = self
            .var("OPENCODE_TEST_MANAGED_CONFIG_DIR")
            .filter(|value| !value.is_empty())
        {
            return PathBuf::from(dir);
        }
        match self.platform {
            Platform::MacOs => PathBuf::from("/Library/Application Support/opencode"),
            Platform::Windows => PathBuf::from(
                self.var("ProgramData")
                    .filter(|v| !v.is_empty())
                    .unwrap_or("C:\\ProgramData"),
            )
            .join("opencode"),
            Platform::Other => PathBuf::from("/etc/opencode"),
        }
    }

    /// `os.userInfo().username || "user"` with the throw fallback.
    pub fn username_or_default(&self) -> String {
        self.username
            .clone()
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "user".into())
    }
}
