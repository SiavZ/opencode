//! Purpose: enforce the physical source-line ceiling and independently reviewed exceptions.
//! Scope: read-only build tooling.
//! Uses:
//! rust/xtask/
//! `-- src/workspace.rs
//! External:
//! |-- anyhow::Result
//! |-- anyhow::bail
//! |-- serde::Deserialize
//! |-- str::parse
//! `-- toml::from_str
//! Used by:
//! rust/xtask/
//! |-- src/check.rs
//! |-- src/lib.rs
//! |-- src/main.rs
//! `-- tests/gates.rs
//! Invariants: all source files are checked; stale, expired and self-reviewed records fail.
//! Contract tests: xtask/tests/gates.rs

use anyhow::{Result, bail};
use serde::Deserialize;
use std::collections::BTreeSet;
use std::path::Path;

#[derive(Debug, Default, Deserialize)]
struct Reviews {
    #[serde(default)]
    review: Vec<Review>,
}

#[derive(Debug, Deserialize)]
struct Review {
    path: String,
    lines: usize,
    decision: String,
    reason: String,
    author: String,
    reviewer: String,
    expires: String,
}

pub fn run(args: &[String]) -> Result<()> {
    if !args.is_empty() {
        bail!("size accepts no arguments");
    }
    let root = crate::workspace::rust_root()?;
    // UTC date without a shell or machine-local timezone dependence.
    let days = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs()
        / 86400;
    let today = civil_date(days as i64);
    crate::workspace::finish("size", validate(&root, &today)?)
}

fn civil_date(days: i64) -> String {
    let z = days + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    format!("{:04}-{month:02}-{day:02}", year + i64::from(month <= 2))
}

pub fn validate(root: &Path, today: &str) -> Result<Vec<String>> {
    let reviews: Reviews = toml::from_str(&std::fs::read_to_string(
        root.join("contracts/modularity-reviews.toml"),
    )?)?;
    let mut errors = Vec::new();
    let mut seen = BTreeSet::new();
    for review in &reviews.review {
        if !seen.insert(&review.path) {
            errors.push(format!("duplicate review {}", review.path));
        }
        let date = review
            .expires
            .split('-')
            .map(str::parse::<u32>)
            .collect::<Result<Vec<_>, _>>();
        let valid_date = date.is_ok_and(|d| {
            if d.len() != 3 || review.expires.len() != 10 || d[0] == 0 || !(1..=12).contains(&d[1])
            {
                return false;
            }
            let limit = match d[1] {
                2 => {
                    if d[0] % 4 == 0 && (d[0] % 100 != 0 || d[0] % 400 == 0) {
                        29
                    } else {
                        28
                    }
                }
                4 | 6 | 9 | 11 => 30,
                _ => 31,
            };
            (1..=limit).contains(&d[2])
        });
        if !valid_date
            || review.expires.as_str() < today
            || review.author.trim().is_empty()
            || review.author.trim() == review.reviewer.trim()
            || review.reviewer.trim().is_empty()
            || review.reason.trim().is_empty()
            || review.decision != "exception"
        {
            errors.push(format!(
                "{}: invalid/expired/non-independent exception (expires {}, decision {})",
                review.path, review.expires, review.decision
            ));
        }
    }
    let mut used = BTreeSet::new();
    for file in crate::workspace::sources(root)? {
        let name = crate::workspace::display_path(root, &file);
        let lines = std::fs::read_to_string(file)?.lines().count();
        if lines <= 500 {
            continue;
        }
        match reviews.review.iter().find(|r| r.path == name) {
            None => errors.push(format!(
                "{name}: {lines} physical lines > 500, no modularity review"
            )),
            Some(review) => {
                used.insert(name.clone());
                if lines != review.lines {
                    errors.push(format!(
                        "{name}: review records {} lines, actual {lines}",
                        review.lines
                    ));
                }
            }
        }
    }
    for review in &reviews.review {
        if !used.contains(&review.path) {
            errors.push(format!(
                "{}: stale review (missing file or no longer >500 lines)",
                review.path
            ));
        }
    }
    Ok(errors)
}
