//! Purpose: tests for log directories, retention, tail and the debug zip export (fixture-based).
//! Scope: integration test through `opencode_desktop_host::store::debug_logs`.
//! Uses:
//! rust/crates/
//! `-- desktop-host/src/store/debug_logs.rs
//! External:
//! |-- serde_json::Map::new
//! |-- serde_json::Value
//! |-- serde_json::from_str
//! |-- str::to_string
//! |-- tempfile::tempdir
//! |-- time::OffsetDateTime::now_utc
//! |-- time::macros::datetime
//! `-- zip::ZipArchive::new
//! Used by:
//! `-- none, entrypoint
//! Invariants: never touches real user data or the real Downloads folder.
//! Contract tests: desktop-host/tests/store_debug_logs.rs

use std::io::Read;
use std::path::Path;
use std::time::{Duration, SystemTime};

use opencode_desktop_host::store::debug_logs::{
    DebugExport, LogDirs, export_debug_logs, safe_log_name, server_log_roots, stamp, tail,
};
use time::macros::datetime;

fn age(path: &Path, by: Duration) {
    let when = SystemTime::now() - by;
    if path.is_dir() {
        std::fs::File::open(path)
            .unwrap()
            .set_modified(when)
            .unwrap();
        return;
    }
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(when)
        .unwrap();
}

#[test]
fn stamp_and_safe_names_match_baseline() {
    assert_eq!(
        stamp(datetime!(2026-10-03 11:36:53.123 UTC)),
        "20261003T113653"
    );
    assert_eq!(
        stamp(datetime!(2026-10-03 14:36:53 +03:00)),
        "20261003T113653"
    );
    assert_eq!(safe_log_name("main"), "main");
    assert_eq!(safe_log_name("wsl/server 1"), "wsl_server_1");
    assert_eq!(safe_log_name("a😀"), "a__");
    assert_eq!(safe_log_name(""), "main");
}

#[test]
fn server_log_roots_dedupe_in_order() {
    let home = Path::new("/h");
    let user = Path::new("/u");
    assert_eq!(
        server_log_roots(None, home, user),
        [
            Path::new("/h/.local/share/opencode/log"),
            Path::new("/u/opencode/log")
        ]
    );
    assert_eq!(
        server_log_roots(Some(Path::new("/u")), home, user),
        [Path::new("/u/opencode/log")]
    );
}

#[test]
fn cleanup_removes_entries_older_than_seven_days() {
    let dir = tempfile::tempdir().unwrap();
    let logs = LogDirs::init(dir.path(), datetime!(2026-10-03 00:00 UTC)).unwrap();
    let old = logs.root.join("20260901T000000");
    std::fs::create_dir_all(&old).unwrap();
    std::fs::write(old.join("main.log"), "x").unwrap();
    age(&old, Duration::from_secs(8 * 86_400));
    std::fs::write(logs.root.join("stray.log"), "x").unwrap();
    let removed = logs.cleanup(SystemTime::now());
    assert_eq!(removed, ["20260901T000000"]);
    assert!(logs.run.exists());
    assert!(logs.root.join("stray.log").exists());
}

#[test]
fn tail_returns_last_thousand_lines_or_empty() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("main.log");
    let text: Vec<String> = (0..1500).map(|i| i.to_string()).collect();
    std::fs::write(&file, text.join("\n")).unwrap();
    let out = tail(&file);
    assert_eq!(out.lines().count(), 1000);
    assert!(out.starts_with("500\n"));
    assert_eq!(tail(&dir.path().join("missing")), "");
}

#[test]
fn export_bundles_recent_files_with_manifest() {
    let dir = tempfile::tempdir().unwrap();
    let user = dir.path().join("user");
    let now = datetime!(2026-10-03 11:36:53.5 UTC);
    let logs = LogDirs::init(&user, now).unwrap();
    std::fs::write(logs.file_for("main"), "hello").unwrap();
    std::fs::write(logs.run.join("dump.heapsnapshot"), "x").unwrap();
    let stale = logs.run.join("stale.log");
    std::fs::write(&stale, "x").unwrap();
    age(&stale, Duration::from_secs(2 * 86_400));
    let server = dir.path().join("server-log");
    std::fs::create_dir_all(server.join("nested")).unwrap();
    std::fs::write(server.join("nested/a.log"), "srv").unwrap();
    let export = DebugExport {
        output_dir: dir.path().join("downloads"),
        logs: logs.clone(),
        user_data: user.clone(),
        crash_dumps: user.join("Crashpad"),
        server_logs: vec![server, dir.path().join("missing")],
        net_log: None,
        version: "1.2.3".into(),
        name: "OpenCode Dev".into(),
        packaged: false,
        versions: serde_json::Map::new(),
        uptime_secs: 1.5,
    };
    // Real "now" for file ages, fixed stamp for the name.
    let out = export_debug_logs(&export, time::OffsetDateTime::now_utc()).unwrap();
    assert!(
        out.file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("opencode-debug-")
    );
    let mut zip = zip::ZipArchive::new(std::fs::File::open(&out).unwrap()).unwrap();
    let names: Vec<String> = zip.file_names().map(str::to_string).collect();
    let run = logs.run.file_name().unwrap().to_str().unwrap();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(
        sorted,
        [
            format!("desktop/{run}/main.log"),
            "manifest.json".into(),
            "server-1/nested/a.log".into()
        ]
    );
    assert_eq!(names[0], "manifest.json");
    let mut manifest = String::new();
    zip.by_name("manifest.json")
        .unwrap()
        .read_to_string(&mut manifest)
        .unwrap();
    let manifest: serde_json::Value = serde_json::from_str(&manifest).unwrap();
    let keys: Vec<&str> = manifest
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "generated",
            "version",
            "name",
            "packaged",
            "platform",
            "arch",
            "versions",
            "uptime",
            "userData",
            "logs",
            "currentRun",
            "crashDumps",
            "serverLogs"
        ]
    );
    assert!(manifest["generated"].as_str().unwrap().ends_with('Z'));
    let _ = now;
}
