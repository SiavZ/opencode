//! Purpose: legacy VCS status, patch, branch and application HTTP operations.
//! Scope: real native Git subprocesses in the request checkout.
//! Uses:
//! rust/crates/
//! |-- platform-native/src/vcs/run.rs
//! `-- runtime/src/global_services.rs
//! External:
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! `-- runtime/src/global_services.rs
//! Invariants: no shell interpolation; apply failures retain non-git versus not-clean reasons.
//! Contract tests: runtime/tests/global_services.rs

use super::{
    GlobalServices, LegacyError, Map, RouteContext, Value, internal, json, required_payload,
};
use opencode_platform_native::vcs::GitCli;
use opencode_platform_native::vcs::run::GitOutput;
use std::collections::BTreeMap;
use std::path::Path;

const CONFIG: &[&str] = &[
    "--no-optional-locks",
    "-c",
    "core.autocrlf=false",
    "-c",
    "core.fsmonitor=false",
    "-c",
    "core.longpaths=true",
    "-c",
    "core.symlinks=true",
    "-c",
    "core.quotepath=false",
];

impl GlobalServices {
    pub(super) fn git(&self) -> GitCli {
        let mut environment = self.options.environment.vars.clone();
        environment.extend(self.options.git_env.iter().cloned());
        GitCli::default().with_env(environment)
    }

    pub(super) async fn vcs(
        &self,
        operation: &str,
        context: &RouteContext,
        query: &Map<String, Value>,
        payload: Option<Value>,
    ) -> Result<Value, LegacyError> {
        if !matches!(
            operation,
            "vcs.get" | "vcs.status" | "vcs.diff" | "vcs.diff.raw" | "vcs.apply"
        ) {
            return Err(LegacyError::BadRequest(format!(
                "Unknown VCS operation: {operation}"
            )));
        }
        let cwd = Path::new(&context.directory);
        let cli = self.git();
        let project = self
            .projects
            .from_directory(&context.directory)
            .await
            .map_err(internal)?;
        let git = project.project.vcs.is_some();
        if operation == "vcs.apply" {
            if !git {
                return Err(apply_error(
                    "non-git",
                    "Patch can't be applied because the project is not git-based",
                ));
            }
            let payload = required_payload(payload)?;
            let patch = payload
                .get("patch")
                .and_then(Value::as_str)
                .ok_or_else(|| LegacyError::BadRequest("Missing patch".into()))?;
            let args = CONFIG
                .iter()
                .copied()
                .chain(["apply", "-"])
                .collect::<Vec<_>>();
            let result = cli
                .exec(cwd, &args, Some(patch.as_bytes().to_vec()), &[])
                .await
                .map_err(internal)?;
            if !result.ok() {
                return Err(apply_error("not-clean", "Patch can't be applied"));
            }
            return Ok(json!({"applied":true}));
        }
        if operation == "vcs.get" {
            if !git {
                return Ok(json!({}));
            }
            let branch = run(&cli, cwd, &["symbolic-ref", "--quiet", "--short", "HEAD"]).await;
            let mut value = json!({});
            if branch.ok() && !branch.text.trim().is_empty() {
                value["branch"] = json!(branch.text.trim());
            }
            if let Some((name, _)) = default_branch(&cli, cwd).await {
                value["default_branch"] = json!(name);
            }
            return Ok(value);
        }
        if !git {
            return Ok(if operation == "vcs.diff.raw" {
                json!("")
            } else {
                json!([])
            });
        }
        let has_head = run(&cli, cwd, &["rev-parse", "--verify", "HEAD"])
            .await
            .ok();
        let status = statuses(&cli, cwd).await;
        if operation == "vcs.diff.raw" {
            let tracked = if has_head {
                patch(&cli, cwd, Some("HEAD"), ".", 3).await
            } else {
                String::new()
            };
            let mut parts = vec![tracked];
            for (file, code) in &status {
                if code == "??" {
                    parts.push(patch(&cli, cwd, None, file, 3).await);
                }
            }
            return Ok(json!(
                parts
                    .into_iter()
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n")
            ));
        }
        let reference = if operation == "vcs.diff"
            && query.get("mode").and_then(Value::as_str) == Some("branch")
        {
            let Some((name, reference)) = default_branch(&cli, cwd).await else {
                return Ok(json!([]));
            };
            if run(&cli, cwd, &["symbolic-ref", "--quiet", "--short", "HEAD"])
                .await
                .text
                .trim()
                == name
            {
                return Ok(json!([]));
            }
            let base = run(&cli, cwd, &["merge-base", &reference, "HEAD"]).await;
            if !base.ok() || base.text.trim().is_empty() {
                return Ok(json!([]));
            }
            Some(base.text.trim().to_owned())
        } else {
            has_head.then(|| "HEAD".to_owned())
        };
        let stats = match reference.as_deref() {
            Some(reference) => numstats(
                &run(
                    &cli,
                    cwd,
                    &[
                        "diff",
                        "--no-ext-diff",
                        "--no-renames",
                        "--numstat",
                        "-z",
                        reference,
                        "--",
                        ".",
                    ],
                )
                .await
                .text,
            ),
            None => BTreeMap::new(),
        };
        let mut items = if operation == "vcs.diff"
            && let Some(reference) = &reference
        {
            let result = run(
                &cli,
                cwd,
                &[
                    "diff",
                    "--no-ext-diff",
                    "--no-renames",
                    "--name-status",
                    "-z",
                    reference,
                    "--",
                    ".",
                ],
            )
            .await;
            let parts = result
                .text
                .split('\0')
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>();
            let mut items = parts
                .chunks_exact(2)
                .map(|pair| (pair[1].to_owned(), pair[0].to_owned()))
                .collect::<Vec<_>>();
            items.extend(status.iter().filter(|(_, code)| code == "??").cloned());
            items
        } else {
            status
        };
        items.sort_by(|a, b| a.0.cmp(&b.0));
        items.dedup_by(|a, b| a.0 == b.0);
        let mut total = 0;
        let mut out = Vec::new();
        for (file, code) in items {
            let kind = kind(&code);
            let (additions, deletions) = if let Some(stats) = stats.get(&file) {
                *stats
            } else if kind == "added" {
                let result = run(
                    &cli,
                    Path::new(&project.sandbox),
                    &["diff", "--no-index", "--numstat", "--", "/dev/null", &file],
                )
                .await;
                let parts = result.text.split('\t').collect::<Vec<_>>();
                (
                    parts
                        .first()
                        .and_then(|part| part.parse::<u64>().ok())
                        .unwrap_or(0),
                    parts
                        .get(1)
                        .and_then(|part| part.parse::<u64>().ok())
                        .unwrap_or(0),
                )
            } else {
                (0, 0)
            };
            let mut item =
                json!({"file":file,"additions":additions,"deletions":deletions,"status":kind});
            if operation == "vcs.diff" {
                let context = query
                    .get("context")
                    .and_then(Value::as_u64)
                    .unwrap_or(2_147_483_647);
                let text = patch(
                    &cli,
                    cwd,
                    if code == "??" {
                        None
                    } else {
                        reference.as_deref()
                    },
                    &file,
                    context,
                )
                .await;
                let text = if text.is_empty()
                    || text.len() > 10_000_000
                    || total + text.len() > 10_000_000
                {
                    empty_patch(&file)
                } else {
                    total += text.len();
                    text
                };
                item["patch"] = json!(text);
            }
            out.push(item);
        }
        Ok(Value::Array(out))
    }
}

async fn run(cli: &GitCli, cwd: &Path, args: &[&str]) -> GitOutput {
    cli.run(
        cwd,
        &CONFIG
            .iter()
            .copied()
            .chain(args.iter().copied())
            .collect::<Vec<_>>(),
    )
    .await
}

async fn statuses(cli: &GitCli, cwd: &Path) -> Vec<(String, String)> {
    run(
        cli,
        cwd,
        &[
            "status",
            "--porcelain=v1",
            "--untracked-files=all",
            "--no-renames",
            "-z",
            "--",
            ".",
        ],
    )
    .await
    .text
    .split('\0')
    .filter_map(|item| (item.len() > 3).then(|| (item[3..].to_owned(), item[..2].to_owned())))
    .collect()
}

fn kind(code: &str) -> &'static str {
    if code == "??" {
        return "added";
    }
    if code.contains('U') {
        return "modified";
    }
    if code.contains('A') && !code.contains('D') {
        return "added";
    }
    if code.contains('D') && !code.contains('A') {
        return "deleted";
    }
    "modified"
}

fn numstats(text: &str) -> BTreeMap<String, (u64, u64)> {
    text.split('\0')
        .filter_map(|line| {
            let mut parts = line.splitn(3, '\t');
            let additions = parts.next()?.parse().unwrap_or(0);
            let deletions = parts.next()?.parse().unwrap_or(0);
            Some((parts.next()?.into(), (additions, deletions)))
        })
        .collect()
}

async fn patch(
    cli: &GitCli,
    cwd: &Path,
    reference: Option<&str>,
    file: &str,
    context: u64,
) -> String {
    let context = format!("--unified={context}");
    let args = if let Some(reference) = reference {
        vec![
            "diff",
            "--patch",
            "--no-ext-diff",
            "--no-renames",
            &context,
            reference,
            "--",
            file,
        ]
    } else {
        vec![
            "diff",
            "--no-index",
            "--patch",
            "--no-ext-diff",
            "--no-renames",
            &context,
            "--",
            "/dev/null",
            file,
        ]
    };
    run(cli, cwd, &args).await.text
}

fn empty_patch(file: &str) -> String {
    format!(
        "Index: {file}\n===================================================================\n--- {file}\t\n+++ {file}\t\n"
    )
}

async fn default_branch(cli: &GitCli, cwd: &Path) -> Option<(String, String)> {
    let remotes = run(cli, cwd, &["remote"]).await;
    let remotes = remotes
        .text
        .lines()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .collect::<Vec<_>>();
    let remote = if remotes.contains(&"origin") {
        Some("origin")
    } else if remotes.len() == 1 {
        remotes.first().copied()
    } else if remotes.contains(&"upstream") {
        Some("upstream")
    } else {
        remotes.first().copied()
    };
    if let Some(remote) = remote {
        let result = run(
            cli,
            cwd,
            &["symbolic-ref", &format!("refs/remotes/{remote}/HEAD")],
        )
        .await;
        if result.ok() {
            let reference = result.text.trim().trim_start_matches("refs/remotes/");
            if let Some(name) = reference.strip_prefix(&format!("{remote}/"))
                && !name.is_empty()
            {
                return Some((name.into(), reference.into()));
            }
        }
    }
    let refs = run(
        cli,
        cwd,
        &["for-each-ref", "--format=%(refname:short)", "refs/heads"],
    )
    .await;
    let refs = refs.text.lines().map(str::trim).collect::<Vec<_>>();
    let configured = run(cli, cwd, &["config", "init.defaultBranch"]).await;
    [configured.text.trim(), "main", "master"]
        .into_iter()
        .find(|name| !name.is_empty() && refs.contains(name))
        .map(|name| (name.into(), name.into()))
}

fn apply_error(reason: &str, message: &str) -> LegacyError {
    LegacyError::Response {
        status: 400,
        body: json!({"name":"VcsApplyError","data":{"message":message,"reason":reason}}),
    }
}
