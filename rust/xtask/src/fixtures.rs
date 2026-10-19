//! Purpose: verify versioned fixture corpora and execute their real Rust differential runners.
//! Scope: offline build-time checks; no fixture capture or blessing.
//! Uses:
//! rust/xtask/
//! `-- src/workspace.rs
//! External:
//! |-- anyhow::Context
//! |-- anyhow::Result
//! |-- anyhow::bail
//! |-- regex::Regex::new
//! |-- serde_json::Value
//! |-- serde_json::from_str
//! |-- str::is_empty
//! `-- walkdir::WalkDir::new
//! Used by:
//! rust/xtask/
//! |-- src/check.rs
//! |-- src/lib.rs
//! |-- src/main.rs
//! `-- tests/gates.rs
//! Invariants: empty corpora and runner failures fail; strict mode also rejects known parity debt.
//! Contract tests: xtask/tests/gates.rs

use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::Path;

pub fn run(args: &[String]) -> Result<()> {
    let strict = args.iter().any(|arg| arg == "--strict");
    if !args.is_empty() && args != ["run"] && args != ["run", "--strict"] {
        bail!("usage: fixtures [run [--strict]]");
    }
    let root = crate::workspace::rust_root()?;
    let mut errors = validate(&root)?;
    if args.first().is_some_and(|arg| arg == "run") {
        let executed = regex::Regex::new(r"test result: ok\. ([1-9][0-9]*) passed")?;
        for (package, target, filter) in [
            ("opencode-schema", None, None),
            ("opencode-server", Some("replay_baseline"), None),
            ("opencode-provider-adapters", None, None),
            ("opencode", Some("cli_help"), None),
            ("xtask", Some("xtask"), Some("cloud::tests")),
        ] {
            let mut command = std::process::Command::new(
                std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()),
            );
            command
                .current_dir(&root)
                .args(["test", "--locked", "--offline", "-p", package])
                .env_remove("REPLAY_BLESS")
                .env_remove("UPDATE_EXPECT")
                .env_remove("INSTA_UPDATE");
            if let Some(target) = target {
                command.args([if filter.is_some() { "--bin" } else { "--test" }, target]);
            } else {
                command.arg("--tests");
            }
            if let Some(filter) = filter {
                command.arg(filter);
            }
            let output = command
                .output()
                .with_context(|| format!("run {package} {target:?}"))?;
            print!("{}", String::from_utf8_lossy(&output.stdout));
            eprint!("{}", String::from_utf8_lossy(&output.stderr));
            let stdout = String::from_utf8_lossy(&output.stdout);
            if !output.status.success() || !executed.is_match(&stdout) {
                errors.push(format!("differential runner {package} {target:?} failed or executed no passing tests ({})", output.status));
            }
        }
        // The replay harness detects ledger drift, but its known failures are still product
        // incompatibilities. Keep them visible rather than treating a stable red ledger as green.
        let ledger: Value = serde_json::from_str(&std::fs::read_to_string(
            root.join("crates/server/tests/fixtures/replay-ledger.json"),
        )?)?;
        let count = ledger
            .as_object()
            .context("replay ledger must be an object")?
            .len();
        println!(
            "fixtures: differential ledger conformance executed; {count} known server incompatibilities remain (this is NOT full production parity)"
        );
        if strict && count != 0 {
            errors.push(format!("strict parity: server differential replay retains {count} known incompatibilities in crates/server/tests/fixtures/replay-ledger.json"));
        }
    }
    crate::workspace::finish("fixtures", errors)
}

pub fn validate(root: &Path) -> Result<Vec<String>> {
    let mut errors = Vec::new();
    let mut count = 0;
    // Include fixture locations documented by phase contracts, not only contracts/fixtures,
    // which has never existed in this checkout. Never traverse target output or live captures.
    for directory in [
        root.join("contracts"),
        root.join("crates"),
        root.join("bins"),
    ] {
        for entry in walkdir::WalkDir::new(directory)
            .into_iter()
            .filter_entry(|entry| entry.file_name() != "target")
        {
            let entry = entry?;
            let name = entry.path().to_string_lossy();
            if !entry.file_type().is_file()
                || entry.path().extension().is_none_or(|ext| ext != "json")
                || !(name.contains("/fixtures/") || name.contains("/cloud-golden/"))
            {
                continue;
            }
            count += 1;
            let text = std::fs::read_to_string(entry.path())?;
            match serde_json::from_str::<Value>(&text) {
                Err(error) => errors.push(format!(
                    "{}: invalid JSON: {error}",
                    crate::workspace::display_path(root, entry.path())
                )),
                Ok(value) => {
                    if name.contains("/fixtures/baseline/") {
                        errors.extend(validate_family(&value).into_iter().map(|error| {
                            format!(
                                "{}: {error}",
                                crate::workspace::display_path(root, entry.path())
                            )
                        }));
                    }
                }
            }
        }
    }
    let baseline = root.join("crates/server/tests/fixtures/baseline");
    if !baseline.is_dir() {
        errors.push("missing versioned server baseline fixture corpus".into());
    }
    let mut ids = BTreeSet::new();
    let mut sequences = BTreeSet::new();
    if baseline.is_dir() {
        for entry in std::fs::read_dir(baseline)? {
            let path = entry?.path();
            if path.extension().is_none_or(|ext| ext != "json") {
                continue;
            }
            let Ok(value) = serde_json::from_str::<Value>(&std::fs::read_to_string(&path)?) else {
                continue;
            };
            for case in value["cases"].as_array().into_iter().flatten() {
                if let Some(id) = case["id"].as_str()
                    && !ids.insert(id.to_owned())
                {
                    errors.push(format!("duplicate fixture id {id}"));
                }
                if let Some(seq) = case["seq"].as_u64()
                    && !sequences.insert(seq)
                {
                    errors.push(format!("duplicate fixture sequence {seq}"));
                }
            }
        }
    }
    if ids.is_empty() {
        errors.push("server fixture corpus has zero differential cases".into());
    }
    if count == 0 {
        errors.push("no versioned JSON fixtures found".into());
    }
    println!(
        "fixtures: verified {count} JSON documents, {} baseline cases",
        ids.len()
    );
    Ok(errors)
}

pub fn validate_family(value: &Value) -> Vec<String> {
    let mut errors = Vec::new();
    if value["family"].as_str().is_none_or(str::is_empty)
        || value["baseline"].as_str().is_none_or(str::is_empty)
    {
        errors.push("fixture requires family and baseline provenance".into());
    }
    let Some(cases) = value["cases"].as_array().filter(|cases| !cases.is_empty()) else {
        errors.push("fixture requires nonempty cases".into());
        return errors;
    };
    for case in cases {
        let id = case["id"].as_str().unwrap_or("<missing id>");
        if case["id"].as_str().is_none_or(str::is_empty)
            || case["seq"].as_u64().is_none()
            || !matches!(case["server"].as_str(), Some("pass" | "nopass"))
            || case["request"]["method"].as_str().is_none_or(str::is_empty)
            || case["request"]["path"]
                .as_str()
                .is_none_or(|p| !p.starts_with('/'))
            || !matches!(
                case["request"]["auth"].as_str(),
                Some("none" | "invalid" | "valid" | "query-valid")
            )
        {
            errors.push(format!("{id}: malformed request/case metadata"));
        }
        let response = &case["response"];
        if !matches!(
            response["kind"].as_str(),
            Some("json" | "text" | "empty" | "sse" | "timeout" | "json-digest" | "text-digest")
        ) || response["headers"].as_object().is_none()
            || (response["kind"] != "timeout"
                && response["status"]
                    .as_u64()
                    .is_none_or(|status| !(100..=599).contains(&status)))
        {
            errors.push(format!("{id}: malformed expected response"));
        }
    }
    errors
}
