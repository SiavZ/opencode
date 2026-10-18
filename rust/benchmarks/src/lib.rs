//! Purpose: drive an opencode binary (TypeScript dev command or Rust build) as an opaque process for
//! benchmarks and acceptance runs: isolated HOME/XDG dirs, CLI invocation, `serve` readiness, HTTP
//! requests and SSE reading. Knows nothing about product internals.
//! Scope: process-level test/bench support library for Phase 15 standalone projects.
//! Uses:
//! rust/benchmarks/
//! `-- src/sse.rs
//! External:
//! |-- anyhow::{Context, Result, bail}
//! |-- shlex::split
//! |-- tempfile::TempDir
//! `-- ureq::Agent
//! Used by:
//! rust/benchmarks/
//! `-- benches/process.rs
//! rust/tests/
//! `-- acceptance/src/runner.rs
//! Invariants: every spawned process gets HOME and XDG_* inside an owned temp dir and inherits no
//! OPENCODE_* variables; spawned servers are killed on drop; nothing reads real user data.
//! Contract tests: benchmarks/tests/target.rs

pub mod sse;

use anyhow::{Context, Result, bail};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};
use tempfile::TempDir;

/// A command line that launches the product, e.g. `bun run src/index.ts` (with cwd) or `/path/opencode`.
#[derive(Debug, Clone)]
pub struct Target {
    pub argv: Vec<String>,
    pub cwd: Option<PathBuf>,
}

impl Target {
    /// Parse a shell-quoted command. Returns `None` for unset or blank input.
    pub fn parse(cmd: &str, cwd: Option<PathBuf>) -> Option<Self> {
        let argv = shlex::split(cmd)?;
        if argv.is_empty() {
            return None;
        }
        Some(Self { argv, cwd })
    }

    /// Read `<var>` (command) and `<var>_CWD` (working directory) from the environment.
    pub fn from_env(var: &str) -> Option<Self> {
        let cmd = std::env::var(var).ok()?;
        let cwd = std::env::var(format!("{var}_CWD")).ok().map(PathBuf::from);
        Self::parse(&cmd, cwd)
    }

    pub fn command(&self, home: &Home, args: &[&str]) -> Command {
        let mut c = Command::new(&self.argv[0]);
        c.args(&self.argv[1..]).args(args);
        if let Some(cwd) = &self.cwd {
            c.current_dir(cwd);
        }
        home.apply(&mut c);
        c
    }

    /// Run a CLI invocation to completion and return its output plus wall time.
    pub fn run(&self, home: &Home, args: &[&str]) -> Result<(Output, Duration)> {
        let start = Instant::now();
        let out = self
            .command(home, args)
            .stdin(Stdio::null())
            .output()
            .with_context(|| format!("spawn {:?}", self.argv))?;
        Ok((out, start.elapsed()))
    }

    /// Start `serve --port <free> --pure` and wait until `GET /global/health` returns 200.
    pub fn serve(&self, home: &Home, timeout: Duration) -> Result<Server> {
        let port = free_port()?;
        let start = Instant::now();
        let child = self
            .command(home, &["serve", "--port", &port.to_string(), "--pure"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("spawn serve {:?}", self.argv))?;
        let mut server = Server {
            child,
            port,
            ready: Duration::ZERO,
        };
        let agent = agent(Duration::from_secs(1));
        while start.elapsed() < timeout {
            if let Some(status) = server.child.try_wait()? {
                bail!("serve exited before ready: {status}");
            }
            if let Ok(resp) = agent.get(server.url("/global/health")).call()
                && resp.status() == 200
            {
                server.ready = start.elapsed();
                return Ok(server);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        bail!("serve not healthy within {timeout:?}")
    }
}

/// Isolated HOME/XDG root. Dropping it deletes the directory.
#[derive(Debug)]
pub struct Home {
    dir: TempDir,
}

impl Home {
    pub fn new() -> Result<Self> {
        Ok(Self {
            dir: tempfile::Builder::new().prefix("opencode-p15-").tempdir()?,
        })
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    fn apply(&self, c: &mut Command) {
        for (k, _) in std::env::vars() {
            if k.starts_with("OPENCODE_") {
                c.env_remove(k);
            }
        }
        let root = self.path();
        c.env("HOME", root)
            .env("XDG_DATA_HOME", root.join("data"))
            .env("XDG_CONFIG_HOME", root.join("config"))
            .env("XDG_CACHE_HOME", root.join("cache"))
            .env("XDG_STATE_HOME", root.join("state"));
    }
}

/// A running `serve` process, killed on drop.
#[derive(Debug)]
pub struct Server {
    child: Child,
    pub port: u16,
    /// Spawn to first healthy response.
    pub ready: Duration,
}

impl Server {
    pub fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{}", self.port, path)
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// HTTP agent that reports non-2xx statuses as responses instead of errors.
pub fn agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(timeout))
        .build()
        .into()
}

pub fn free_port() -> Result<u16> {
    Ok(TcpListener::bind("127.0.0.1:0")?.local_addr()?.port())
}

/// Nearest-rank-interpolated percentile over milliseconds. `None` when empty.
pub fn percentile(samples: &[f64], p: f64) -> Option<f64> {
    if samples.is_empty() {
        return None;
    }
    let mut s = samples.to_vec();
    s.sort_by(f64::total_cmp);
    let k = (s.len() - 1) as f64 * p / 100.0;
    let lo = k.floor() as usize;
    let hi = (lo + 1).min(s.len() - 1);
    Some(s[lo] + (s[hi] - s[lo]) * (k - lo as f64))
}
