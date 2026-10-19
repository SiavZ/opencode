//! Purpose: download indexed skill sources into an isolated, versioned native cache.
//! Scope: HTTP skill discovery only, no package execution.
//! Uses:
//! rust/crates/
//! `-- integration-api/src/error.rs
//! External:
//! |-- reqwest::Client
//! |-- sha2::Sha256
//! `-- tokio::fs
//! Used by:
//! rust/crates/
//! `-- runtime/src/location_integrations/resources.rs
//! Invariants: all index paths remain inside their skill root; a failed version refresh preserves cached data.
//! Contract tests: integration-adapters/tests/skill_discovery.rs

use opencode_integration_api::IntegrationError;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use url::Url;

static NEXT: AtomicU64 = AtomicU64::new(1);

pub async fn discover(cache: &Path, source: &str) -> Result<Vec<PathBuf>, IntegrationError> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|error| IntegrationError::Failed {
            name: "skill HTTP".into(),
            message: error.to_string(),
        })?;
    pull(&client, cache, source).await
}

pub async fn pull(
    client: &reqwest::Client,
    cache: &Path,
    source: &str,
) -> Result<Vec<PathBuf>, IntegrationError> {
    let base = format!("{}/", source.trim_end_matches('/'));
    let source =
        Url::parse(&base).map_err(|error| IntegrationError::Validation(error.to_string()))?;
    let index = fetch(
        client,
        source
            .join("index.json")
            .map_err(|error| IntegrationError::Validation(error.to_string()))?,
    )
    .await?;
    let index: Value = serde_json::from_slice(&index)
        .map_err(|error| IntegrationError::Validation(error.to_string()))?;
    let skills = index["skills"]
        .as_array()
        .ok_or_else(|| IntegrationError::Validation("skill index requires skills array".into()))?;
    let cache = cache.join("skills").join(
        Sha256::digest(base.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>(),
    );
    let mut roots = Vec::new();
    for skill in skills {
        let Some(name) = skill["name"].as_str().filter(|name| safe_segment(name)) else {
            continue;
        };
        let Some(files) = skill["files"]
            .as_array()
            .and_then(|files| files.iter().map(Value::as_str).collect::<Option<Vec<_>>>())
        else {
            continue;
        };
        if !files
            .iter()
            .any(|file| *file == "SKILL.md" || *file == format!("{name}.md"))
            || files.iter().any(|file| !safe_path(file))
        {
            continue;
        }
        let mut skill_url = source.clone();
        skill_url
            .path_segments_mut()
            .map_err(|()| {
                IntegrationError::Validation("skill URL cannot have path segments".into())
            })?
            .pop_if_empty()
            .push(name)
            .push("");
        let resources = files
            .iter()
            .map(|file| skill_url.join(file))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| IntegrationError::Validation(error.to_string()))?;
        if resources
            .iter()
            .any(|resource| resource.origin() != source.origin())
        {
            continue;
        }
        let root = cache.join(name);
        let version = skill["version"].as_str();
        let current = tokio::fs::read_to_string(root.join(".opencode-version"))
            .await
            .ok();
        let changed = version.is_some_and(|version| current.as_deref() != Some(version));
        let token = NEXT.fetch_add(1, Ordering::Relaxed);
        let destination = if changed {
            cache.join(format!("{name}.tmp-{}-{token}", std::process::id()))
        } else {
            root.clone()
        };
        let mut complete = true;
        for (file, url) in files.iter().zip(resources) {
            let target = destination.join(file);
            if target.exists() {
                continue;
            }
            match fetch(client, url).await {
                Ok(body) => {
                    if let Some(parent) = target.parent() {
                        tokio::fs::create_dir_all(parent).await.map_err(io_error)?;
                    }
                    if let Err(error) = tokio::fs::write(&target, body).await {
                        tracing::warn!(%error,"skill cache write failed");
                        complete = false;
                    }
                }
                Err(error) => {
                    tracing::warn!(%error,"skill download failed");
                    complete = false;
                }
            }
        }
        let exists = destination.join("SKILL.md").exists()
            || destination.join(format!("{name}.md")).exists();
        if changed && complete && exists {
            tokio::fs::write(
                destination.join(".opencode-version"),
                version.unwrap_or_default(),
            )
            .await
            .map_err(io_error)?;
            let backup = cache.join(format!("{name}.old-{}-{token}", std::process::id()));
            let had_cache = root.exists();
            if had_cache {
                tokio::fs::rename(&root, &backup).await.map_err(io_error)?;
            }
            if let Err(error) = tokio::fs::rename(&destination, &root).await {
                if had_cache {
                    let _ = tokio::fs::rename(&backup, &root).await;
                }
                return Err(io_error(error));
            }
            if had_cache {
                let _ = tokio::fs::remove_dir_all(backup).await;
            }
        }
        if changed {
            let _ = tokio::fs::remove_dir_all(destination).await;
        }
        if root.join("SKILL.md").exists() || root.join(format!("{name}.md")).exists() {
            roots.push(root);
        }
    }
    Ok(roots)
}

fn safe_segment(value: &str) -> bool {
    !value.is_empty() && value != "." && value != ".." && !value.contains(['/', '\\', '\0'])
}
fn safe_path(value: &str) -> bool {
    !value.is_empty()
        && !value.contains(['\\', '\0', '?', '#'])
        && !value.starts_with('/')
        && Url::parse(value).is_err()
        && value.split('/').all(|segment| {
            let decoded = decode(segment);
            decoded.is_some_and(|segment| safe_segment(&segment) && !segment.contains(':'))
        })
}
fn decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut out = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = char::from(*bytes.get(index + 1)?).to_digit(16)?;
            let low = char::from(*bytes.get(index + 2)?).to_digit(16)?;
            out.push((high * 16 + low) as u8);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).ok()
}
async fn fetch(client: &reqwest::Client, url: Url) -> Result<Vec<u8>, IntegrationError> {
    for attempt in 0..3 {
        let result = client.get(url.clone()).send().await;
        match result {
            Ok(response) if response.status().is_success() => {
                return response
                    .bytes()
                    .await
                    .map(|bytes| bytes.to_vec())
                    .map_err(|error| IntegrationError::Failed {
                        name: "skill HTTP".into(),
                        message: error.to_string(),
                    });
            }
            Ok(response)
                if !response.status().is_server_error() && response.status().as_u16() != 429 =>
            {
                return Err(IntegrationError::Failed {
                    name: "skill HTTP".into(),
                    message: format!("HTTP {}", response.status()),
                });
            }
            _ if attempt < 2 => {
                tokio::time::sleep(std::time::Duration::from_millis(200 << attempt)).await
            }
            _ => break,
        }
    }
    Err(IntegrationError::Failed {
        name: "skill HTTP".into(),
        message: "request failed after retries".into(),
    })
}
fn io_error(error: std::io::Error) -> IntegrationError {
    IntegrationError::Failed {
        name: "skill cache".into(),
        message: error.to_string(),
    }
}
