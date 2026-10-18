//! Purpose: execute acceptance workflows against a real opencode target process and produce one
//! evidence record per (workflow, role) with exactly the production-gates.md evidence fields.
//! Scope: Phase 15 black-box acceptance runner. Fixture classification comes from the workflow.
//! Uses:
//! rust/benchmarks/
//! |-- src/lib.rs
//! `-- src/sse.rs
//! rust/tests/
//! |-- acceptance/src/evidence.rs
//! `-- acceptance/src/workflow.rs
//! External:
//! |-- anyhow::{Result, bail}
//! |-- regex::Regex
//! `-- serde_json::Value
//! Used by:
//! rust/tests/
//! |-- acceptance/src/main.rs
//! `-- acceptance/tests/workflows.rs
//! Invariants: each workflow run gets a fresh isolated HOME/XDG and a fresh git work directory;
//! the first failing step stops the workflow and is recorded verbatim in `observed`.
//! Contract tests: acceptance/tests/workflows.rs

use crate::evidence::{Evidence, Role};
use crate::workflow::{Step, Workflow};
use anyhow::{Result, anyhow, bail};
use opencode_benchmarks::{Home, Server, Target, agent, sse::SseStream, sse::event_type};
use regex::Regex;
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::process::Command;
use std::time::Duration;

/// Run one workflow, never panicking on product failure: failures become a `fail` record.
pub fn run(workflow: &Workflow, role: Role, target: Option<&Target>) -> Evidence {
    let mut ev = Evidence::new(workflow, role, target);
    let Some(target) = target else {
        ev.observed = vec!["not run".into()];
        ev.gate_decision = format!("blocked: {} binary not built or command unset", role.var());
        return ev;
    };
    let mut log = Vec::new();
    let result = execute(workflow, target, &mut log);
    if let Err(e) = &result {
        log.push(format!("FAILED: {e:#}"));
    }
    ev.observed = log;
    ev.gate_decision = match (role, result.is_ok()) {
        (Role::Baseline, true) => "baseline-recorded".into(),
        (Role::Baseline, false) => "baseline-failed: expectation does not match baseline".into(),
        (Role::Candidate, true) => "pass (pending review)".into(),
        (Role::Candidate, false) => "fail".into(),
    };
    ev
}

struct Ctx {
    vars: HashMap<String, String>,
    server: Option<Server>,
    streams: HashMap<String, SseStream>,
}

fn execute(workflow: &Workflow, target: &Target, log: &mut Vec<String>) -> Result<()> {
    let home = Home::new()?;
    let work = tempfile::Builder::new()
        .prefix("opencode-p15-work-")
        .tempdir()?;
    let status = Command::new("git")
        .args(["init", "-q"])
        .arg(work.path())
        .status()?;
    if !status.success() {
        bail!("git init failed");
    }
    let real = work.path().canonicalize()?;
    let mut ctx = Ctx {
        vars: HashMap::from([
            ("workdir".into(), work.path().display().to_string()),
            ("workdir_real".into(), real.display().to_string()),
        ]),
        server: None,
        streams: HashMap::new(),
    };
    for step in &workflow.steps {
        let line = step_once(step, target, &home, &mut ctx)?;
        log.push(line);
    }
    // Drop streams before the server so reader threads observe EOF promptly.
    ctx.streams.clear();
    Ok(())
}

fn step_once(step: &Step, target: &Target, home: &Home, ctx: &mut Ctx) -> Result<String> {
    match step {
        Step::Cli {
            args,
            expect_exit,
            expect_stdout_regex,
        } => {
            let args: Vec<String> = args.iter().map(|a| template(a, &ctx.vars)).collect();
            let refs: Vec<&str> = args.iter().map(String::as_str).collect();
            let (out, dt) = target.run(home, &refs)?;
            let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let code = out.status.code();
            if let Some(want) = expect_exit
                && code != Some(*want)
            {
                bail!(
                    "cli {args:?} exit {code:?} != {want}, stderr: {}",
                    tail(&out.stderr)
                );
            }
            if let Some(re) = expect_stdout_regex
                && !Regex::new(re)?.is_match(&stdout)
            {
                bail!("cli {args:?} stdout {stdout:?} !~ /{re}/");
            }
            Ok(format!(
                "cli {args:?} exit={code:?} stdout={:?} {}ms",
                truncate(&stdout, 200),
                dt.as_millis()
            ))
        }
        Step::Serve { ready_timeout_secs } => {
            let server = target.serve(home, Duration::from_secs(*ready_timeout_secs))?;
            let line = format!("serve healthy in {}ms", server.ready.as_millis());
            ctx.server = Some(server);
            Ok(line)
        }
        Step::Http {
            method,
            path,
            body,
            expect_status,
            expect_json,
            expect_types,
            expect_array_item,
            array_pointer,
            capture,
        } => {
            let server = ctx
                .server
                .as_ref()
                .ok_or_else(|| anyhow!("http step before serve"))?;
            let path = template(path, &ctx.vars);
            let url = server.url(&path);
            let http = agent(Duration::from_secs(15));
            let mut resp = match (method.as_str(), body) {
                ("GET", _) => http.get(&url).call()?,
                ("DELETE", _) => http.delete(&url).call()?,
                ("POST", b) => http
                    .post(&url)
                    .send_json(b.clone().unwrap_or(Value::Null))?,
                ("PATCH", b) => http
                    .patch(&url)
                    .send_json(b.clone().unwrap_or(Value::Null))?,
                (m, _) => bail!("unsupported method {m}"),
            };
            let status = resp.status().as_u16();
            let text = resp.body_mut().read_to_string()?;
            if let Some(want) = expect_status
                && status != *want
            {
                bail!(
                    "{method} {path} status {status} != {want}, body {}",
                    truncate(&text, 300)
                );
            }
            let json: Value = if text.is_empty() {
                Value::Null
            } else {
                serde_json::from_str(&text).map_err(|e| {
                    anyhow!(
                        "{method} {path} body not JSON ({e}): {}",
                        truncate(&text, 300)
                    )
                })?
            };
            check_json(&json, expect_json, &ctx.vars)?;
            for (ptr, ty) in expect_types {
                let got = json.pointer(ptr).map(type_name).unwrap_or("missing");
                if Some(got) != ty.as_str() {
                    bail!("{ptr} type {got} != {ty}");
                }
            }
            if !expect_array_item.is_empty() {
                let arr = json
                    .pointer(array_pointer)
                    .and_then(Value::as_array)
                    .ok_or_else(|| anyhow!("{array_pointer:?} is not an array"))?;
                if !arr
                    .iter()
                    .any(|item| check_json(item, expect_array_item, &ctx.vars).is_ok())
                {
                    bail!(
                        "no array item matches {}",
                        Value::Object(expect_array_item.clone())
                    );
                }
            }
            for (var, ptr) in capture {
                let ptr = ptr
                    .as_str()
                    .ok_or_else(|| anyhow!("capture pointer must be string"))?;
                let v = json
                    .pointer(ptr)
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow!("capture {ptr} missing or not a string"))?;
                ctx.vars.insert(var.clone(), v.to_owned());
            }
            Ok(format!(
                "{method} {path} -> {status} {}",
                truncate(&text, 240)
            ))
        }
        Step::SseConnect {
            name,
            path,
            first_type,
        } => {
            let server = ctx
                .server
                .as_ref()
                .ok_or_else(|| anyhow!("sse step before serve"))?;
            let stream = SseStream::connect(server.port, &template(path, &ctx.vars))?;
            let line = match first_type {
                Some(want) => {
                    let (_, first) = stream
                        .wait_for(0, Duration::from_secs(10), |_| true)
                        .ok_or_else(|| anyhow!("sse {name}: no event within 10s"))?;
                    if event_type(&first) != Some(want.as_str()) {
                        bail!("sse {name}: first event {first} is not {want}");
                    }
                    format!("sse {name} first event {want}")
                }
                None => format!("sse {name} connected"),
            };
            ctx.streams.insert(name.clone(), stream);
            Ok(line)
        }
        Step::SseExpect {
            name,
            r#type,
            expect_json,
            timeout_secs,
        } => {
            let stream = ctx
                .streams
                .get(name)
                .ok_or_else(|| anyhow!("unknown sse stream {name}"))?;
            let hit = stream.wait_for(0, Duration::from_secs(*timeout_secs), |v| {
                event_type(v) == Some(r#type.as_str())
                    && check_json(v, expect_json, &ctx.vars).is_ok()
            });
            match hit {
                Some(_) => Ok(format!("sse {name} saw {type}", type = r#type)),
                None => bail!(
                    "sse {name}: no {type} matching {} in {timeout_secs}s, saw types {:?}",
                    Value::Object(expect_json.clone()),
                    stream.snapshot().iter().filter_map(event_type).collect::<Vec<_>>(),
                    type = r#type
                ),
            }
        }
    }
}

/// Every pointer -> expected pair must match exactly (string expectations are templated).
pub fn check_json(
    json: &Value,
    expect: &Map<String, Value>,
    vars: &HashMap<String, String>,
) -> Result<()> {
    for (ptr, want) in expect {
        let want = match want {
            Value::String(s) => Value::String(template(s, vars)),
            other => other.clone(),
        };
        match json.pointer(ptr) {
            Some(got) if *got == want => {}
            got => bail!(
                "{ptr}: got {}, want {want}",
                got.map_or("missing".into(), Value::to_string)
            ),
        }
    }
    Ok(())
}

/// Replace `${name}` with captured or built-in variables. Unknown names stay verbatim.
pub fn template(s: &str, vars: &HashMap<String, String>) -> String {
    vars.iter().fold(s.to_owned(), |acc, (k, v)| {
        acc.replace(&format!("${{{k}}}"), v)
    })
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_owned();
    }
    format!("{}...", s.chars().take(n).collect::<String>())
}

fn tail(b: &[u8]) -> String {
    let s = String::from_utf8_lossy(b);
    let start = s.len().saturating_sub(400);
    s.get(start..).unwrap_or(&s).to_owned()
}
