//! Purpose: per-run desktop log directories, 7-day log retention and the "export debug logs" zip
//! bundle (manifest plus recent desktop, server and crash files) from `logging.ts`.
//! Scope: filesystem only. The shell owns the electron-log replacement (tracing subscriber),
//! stopping/restarting the network log around an export and revealing the zip in the file
//! manager. Chromium net-log and crashpad are Electron features with no direct equivalent.
//! Uses:
//! `-- none
//! External:
//! |-- serde_json::Map
//! |-- serde_json::Value
//! |-- serde_json::json
//! |-- serde_json::to_vec_pretty
//! |-- time::OffsetDateTime
//! |-- time::UtcOffset::UTC
//! |-- zip::CompressionMethod::Deflated
//! |-- zip::ZipWriter::new
//! |-- zip::result::ZipError
//! `-- zip::write::SimpleFileOptions
//! Used by:
//! rust/crates/
//! |-- desktop-host/src/store.rs
//! `-- desktop-host/tests/store_debug_logs.rs
//! Invariants: exported entries are newer than 24 hours, at most 50 MiB each, never
//! `.heapsnapshot`, named `<prefix>/<relative path>` with forward slashes, the manifest is the
//! first entry and uses the baseline field names, the zip is written to a temp name then renamed.
//! Contract tests: desktop-host/tests/store_debug_logs.rs

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde_json::{Map, Value, json};
use time::OffsetDateTime;
use zip::write::SimpleFileOptions;

pub const MAX_LOG_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);
pub const TAIL_LINES: usize = 1000;
pub const EXPORT_WINDOW: Duration = Duration::from_secs(24 * 60 * 60);
pub const MAX_EXPORT_FILE_SIZE: u64 = 50 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum DebugLogError {
    #[error("debug log io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to write debug zip: {0}")]
    Zip(#[from] zip::result::ZipError),
}

/// `stamp()` (`logging.ts:107-112`): ISO-8601 UTC without separators or fraction,
/// e.g. `20261003T113653`.
pub fn stamp(now: OffsetDateTime) -> String {
    let now = now.to_offset(time::UtcOffset::UTC);
    format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}",
        now.year(),
        u8::from(now.month()),
        now.day(),
        now.hour(),
        now.minute(),
        now.second()
    )
}

/// `safeLogName` (`logging.ts:114-116`). The JS regex has no `u` flag, so a character outside
/// the BMP becomes two underscores.
pub fn safe_log_name(name: &str) -> String {
    let safe: String = name
        .chars()
        .flat_map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '_' | '.' | '-' => vec![c],
            _ => vec!['_'; c.len_utf16()],
        })
        .collect();
    if safe.is_empty() {
        "main".to_string()
    } else {
        safe
    }
}

/// Log directory layout: `<userData>/logs/<stamp>/` (`logging.ts:101-105`).
#[derive(Debug, Clone)]
pub struct LogDirs {
    pub root: PathBuf,
    pub run: PathBuf,
}

impl LogDirs {
    /// Creates the run directory for this process.
    pub fn init(user_data: &Path, now: OffsetDateTime) -> Result<Self, DebugLogError> {
        let root = user_data.join("logs");
        let run = root.join(stamp(now));
        std::fs::create_dir_all(&run).map_err(|source| DebugLogError::Io {
            path: run.clone(),
            source,
        })?;
        Ok(Self { root, run })
    }

    /// File for a log scope inside the current run (`logging.ts:27-30`).
    pub fn file_for(&self, scope: &str) -> PathBuf {
        self.run.join(format!("{}.log", safe_log_name(scope)))
    }

    /// `cleanup()` (`logging.ts:118-131`): removes entries of the logs root older than 7 days.
    /// Per-entry failures are skipped. Returns the removed names.
    pub fn cleanup(&self, now: SystemTime) -> Vec<String> {
        let cutoff = now
            .checked_sub(MAX_LOG_AGE)
            .unwrap_or(SystemTime::UNIX_EPOCH);
        std::fs::read_dir(&self.root)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                let modified = std::fs::metadata(&path).ok()?.modified().ok()?;
                if modified >= cutoff {
                    return None;
                }
                let removed = if path.is_dir() {
                    std::fs::remove_dir_all(&path)
                } else {
                    std::fs::remove_file(&path)
                };
                removed.ok()?;
                Some(entry.file_name().to_string_lossy().into_owned())
            })
            .collect()
    }
}

/// `tail()` (`logging.ts:93-99`): last 1000 lines, empty string on any error.
pub fn tail(file: &Path) -> String {
    let Ok(contents) = std::fs::read_to_string(file) else {
        return String::new();
    };
    let lines: Vec<&str> = contents.split('\n').collect();
    lines[lines.len().saturating_sub(TAIL_LINES)..].join("\n")
}

/// `serverLogRoots()` (`logging.ts:152-155`), deduplicated in order.
pub fn server_log_roots(
    xdg_data_home: Option<&Path>,
    home: &Path,
    user_data: &Path,
) -> Vec<PathBuf> {
    let xdg = xdg_data_home
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| home.join(".local").join("share"));
    let mut roots = vec![xdg.join("opencode").join("log")];
    let second = user_data.join("opencode").join("log");
    if !roots.contains(&second) {
        roots.push(second);
    }
    roots
}

/// Everything the export needs. Process facts are supplied by the shell.
#[derive(Debug, Clone)]
pub struct DebugExport {
    pub output_dir: PathBuf,
    pub logs: LogDirs,
    pub user_data: PathBuf,
    pub crash_dumps: PathBuf,
    pub server_logs: Vec<PathBuf>,
    pub net_log: Option<PathBuf>,
    pub version: String,
    pub name: String,
    pub packaged: bool,
    /// `process.versions` equivalent (component -> version).
    pub versions: Map<String, Value>,
    pub uptime_secs: f64,
}

/// `exportDebugLogs` (`logging.ts:55-75`) minus net-log restart and reveal. Returns the zip path
/// `<output_dir>/opencode-debug-<stamp>.zip`.
pub fn export_debug_logs(
    export: &DebugExport,
    now: OffsetDateTime,
) -> Result<PathBuf, DebugLogError> {
    let wall: SystemTime = now.into();
    let output = export
        .output_dir
        .join(format!("opencode-debug-{}.zip", stamp(now)));
    let mut entries = collect(&export.logs.root, "desktop", wall)?;
    for (index, dir) in export.server_logs.iter().enumerate() {
        entries.extend(collect(dir, &format!("server-{}", index + 1), wall)?);
    }
    entries.extend(collect(&export.crash_dumps, "crashpad", wall)?);

    let manifest = serde_json::to_vec_pretty(&manifest(export, now)).expect("manifest serializes");
    let tmp = output.with_extension("zip.partial");
    let io = |path: &Path| {
        let path = path.to_path_buf();
        move |source| DebugLogError::Io { path, source }
    };
    let result = (|| {
        std::fs::create_dir_all(&export.output_dir).map_err(io(&export.output_dir))?;
        let file = std::fs::File::create(&tmp).map_err(io(&tmp))?;
        let mut zip = zip::ZipWriter::new(file);
        let options = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .large_file(true);
        zip.start_file("manifest.json", options)?;
        zip.write_all(&manifest).map_err(io(&tmp))?;
        for (name, path) in &entries {
            let data = std::fs::read(path).map_err(io(path))?;
            zip.start_file(name.as_str(), options)?;
            zip.write_all(&data).map_err(io(&tmp))?;
        }
        zip.finish()?.sync_all().map_err(io(&tmp))?;
        std::fs::rename(&tmp, &output).map_err(io(&output))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result.map(|()| output)
}

fn manifest(export: &DebugExport, now: OffsetDateTime) -> Value {
    let mut value = json!({
        "generated": iso_millis(now),
        "version": export.version,
        "name": export.name,
        "packaged": export.packaged,
        "platform": node_platform(),
        "arch": node_arch(),
        "versions": export.versions,
        "uptime": export.uptime_secs,
        "userData": export.user_data,
        "logs": export.logs.root,
        "currentRun": export.logs.run,
        "crashDumps": export.crash_dumps,
        "serverLogs": export.server_logs,
    });
    // `netLog: undefined` is omitted by JSON.stringify.
    if let Some(net_log) = &export.net_log {
        value["netLog"] = json!(net_log);
    }
    value
}

/// `Date.prototype.toISOString`: always millisecond precision with `Z`.
fn iso_millis(now: OffsetDateTime) -> String {
    let now = now.to_offset(time::UtcOffset::UTC);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        now.year(),
        u8::from(now.month()),
        now.day(),
        now.hour(),
        now.minute(),
        now.second(),
        now.millisecond()
    )
}

/// `process.platform` spelling.
pub fn node_platform() -> &'static str {
    match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    }
}

/// `process.arch` spelling.
pub fn node_arch() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "x64",
        "x86" => "ia32",
        other => other,
    }
}

/// `collect` (`logging.ts:159-180`): recursive walk with age, size and heapsnapshot filters.
/// A missing directory yields nothing, other io errors fail the export like the baseline.
fn collect(
    dir: &Path,
    prefix: &str,
    now: SystemTime,
) -> Result<Vec<(String, PathBuf)>, DebugLogError> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let cutoff = now
        .checked_sub(EXPORT_WINDOW)
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let mut result = Vec::new();
    walk(dir, dir, prefix, cutoff, &mut result)?;
    Ok(result)
}

fn walk(
    root: &Path,
    current: &Path,
    prefix: &str,
    cutoff: SystemTime,
    result: &mut Vec<(String, PathBuf)>,
) -> Result<(), DebugLogError> {
    let io = |path: &Path| {
        let path = path.to_path_buf();
        move |source| DebugLogError::Io { path, source }
    };
    let mut children: Vec<PathBuf> = std::fs::read_dir(current)
        .map_err(io(current))?
        .map(|entry| entry.map(|e| e.path()))
        .collect::<Result<_, _>>()
        .map_err(io(current))?;
    // readdirSync returns names sorted on the supported filesystems, make it deterministic.
    children.sort();
    for file in children {
        let info = std::fs::metadata(&file).map_err(io(&file))?;
        if info.is_dir() {
            walk(root, &file, prefix, cutoff, result)?;
            continue;
        }
        let modified = info.modified().map_err(io(&file))?;
        if modified < cutoff
            || info.len() > MAX_EXPORT_FILE_SIZE
            || file.to_string_lossy().ends_with(".heapsnapshot")
        {
            continue;
        }
        let relative = file
            .strip_prefix(root)
            .unwrap_or(&file)
            .to_string_lossy()
            .replace('\\', "/");
        result.push((format!("{prefix}/{relative}"), file));
    }
    Ok(())
}
