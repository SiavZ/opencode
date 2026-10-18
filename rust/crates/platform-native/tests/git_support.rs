//! Purpose: shared fixture helpers for the live VCS tests: isolated git environment, temp repos,
//! and a `GitCli` that never reads the user's global or system git config.
//! Scope: test support only (also compiled as its own empty test target).
//! Uses:
//! rust/crates/
//! `-- platform-native/src/vcs/run.rs
//! External:
//! `-- tempfile::TempDir
//! Used by:
//! rust/crates/
//! |-- platform-native/tests/git.rs
//! |-- platform-native/tests/snapshot.rs
//! `-- platform-native/tests/worktree.rs
//! Invariants: HOME, XDG dirs and GIT_CONFIG_GLOBAL point inside the temp dir; system config off.
//! Contract tests: none

#![allow(dead_code, clippy::new_without_default)]

use opencode_platform_native::vcs::GitCli;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub struct Fixture {
    pub tmp: tempfile::TempDir,
    pub env: BTreeMap<String, String>,
}

impl Fixture {
    pub fn new() -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).expect("home");
        let config = home.join(".gitconfig");
        std::fs::write(
            &config,
            "[user]\n\temail = test@opencode.test\n\tname = Test\n[commit]\n\tgpgsign = false\n\
             [init]\n\tdefaultBranch = main\n[core]\n\tfsmonitor = false\n",
        )
        .expect("gitconfig");
        let env = BTreeMap::from([
            ("HOME".into(), home.to_string_lossy().into_owned()),
            (
                "XDG_CONFIG_HOME".into(),
                home.join(".config").to_string_lossy().into_owned(),
            ),
            (
                "GIT_CONFIG_GLOBAL".into(),
                config.to_string_lossy().into_owned(),
            ),
            ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
        ]);
        Self { tmp, env }
    }

    /// Canonical temp root (macOS /var -> /private/var).
    pub fn root(&self) -> PathBuf {
        std::fs::canonicalize(self.tmp.path()).expect("canonical")
    }

    pub fn cli(&self) -> GitCli {
        GitCli::default().with_env(self.env.clone())
    }

    /// Run git synchronously for fixture setup; panics on failure.
    pub fn git(&self, cwd: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .envs(&self.env)
            .output()
            .expect("spawn git");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// `git init` + files + initial commit.
    pub fn repo(&self, name: &str, files: &[(&str, &str)]) -> PathBuf {
        let dir = self.root().join(name);
        std::fs::create_dir_all(&dir).expect("repo dir");
        for (file, content) in files {
            write(&dir.join(file), content);
        }
        self.git(&dir, &["init", "-q"]);
        self.git(&dir, &["add", "."]);
        self.git(&dir, &["commit", "-q", "-m", "initial"]);
        dir
    }
}

pub fn write(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("parent");
    }
    std::fs::write(path, content).expect("write");
}

pub fn read(path: &Path) -> String {
    std::fs::read_to_string(path)
        .expect("read")
        .replace("\r\n", "\n")
}
