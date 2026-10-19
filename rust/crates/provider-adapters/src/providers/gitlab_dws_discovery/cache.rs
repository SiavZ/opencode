//! Purpose: persist GitLab workflow discovery and selections with calendar-correct cache timestamps.
//! Scope: private discovery cache, offline filesystem state and UTC/RFC3339 conversion.
//! Uses:
//! rust/crates/
//! `-- provider-adapters/src/providers/gitlab_dws_discovery.rs
//! External:
//! |-- serde_json::from_str
//! |-- serde_json::to_string_pretty
//! |-- sha2::Digest
//! |-- sha2::Sha256
//! |-- str::to_owned
//! |-- tempfile::tempdir
//! |-- time::Duration
//! |-- time::OffsetDateTime
//! |-- time::UtcOffset::UTC
//! |-- time::format_description::well_known::Rfc3339
//! |-- time::macros::datetime
//! |-- time::macros::format_description
//! `-- u64::try_from
//! Used by:
//! rust/crates/
//! `-- provider-adapters/src/providers/gitlab_dws_discovery.rs
//! Invariants: discovery expires only after ten minutes; selection updates retain discovery age; invalid timestamps expire safely.
//! Contract tests: inline Gregorian rollover, timestamp validation, cache TTL and selection persistence tests.

use super::{DiscoveryResult, GitLabProject};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

const DISCOVERY_TTL_MS: u64 = 10 * 60 * 1000;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(super) struct CacheEntry {
    pub(super) discovery: Option<DiscoveryResult>,
    pub(super) project: Option<GitLabProject>,
    selected_model_ref: Option<String>,
    selected_model_name: Option<String>,
    updated_at: Option<String>,
    discovery_updated_at: Option<String>,
}

pub(super) struct FileCache {
    file_path: PathBuf,
    key: String,
}

impl FileCache {
    pub(super) fn new(work_dir: &str, instance_url: &str, scope: Option<&str>) -> Self {
        let cache_home = std::env::var("XDG_CACHE_HOME")
            .ok()
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_owned());
                PathBuf::from(home).join(".cache")
            });
        let file_path = cache_home
            .join("opencode")
            .join("gitlab-workflow-model-cache.json");
        let normalized_url = instance_url.trim_end_matches('/');
        let input = if let Some(scope) = scope {
            format!("{}\0{}\0{}", work_dir, normalized_url, scope)
        } else {
            format!("{}\0{}", work_dir, normalized_url)
        };
        let key = Sha256::digest(input.as_bytes())
            .iter()
            .take(6)
            .map(|b| format!("{:02x}", b))
            .collect::<String>();
        Self { file_path, key }
    }

    fn read_all(&self) -> HashMap<String, CacheEntry> {
        std::fs::read_to_string(&self.file_path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    fn write_all(&self, data: &HashMap<String, CacheEntry>) {
        if let Some(dir) = self.file_path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp_path = format!(
            "{}.{}.{}.tmp",
            self.file_path.display(),
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
        );
        if let Ok(json) = serde_json::to_string_pretty(data) {
            let _ = std::fs::write(&tmp_path, json.as_bytes());
            let _ = std::fs::rename(&tmp_path, &self.file_path);
        }
    }

    pub(super) fn load(&self) -> Option<CacheEntry> {
        self.read_all().get(&self.key).cloned()
    }

    fn save(&self, entry: CacheEntry) {
        let mut data = self.read_all();
        data.insert(self.key.clone(), entry);
        self.write_all(&data);
    }

    pub(super) fn is_discovery_expired(&self) -> bool {
        self.is_discovery_expired_at(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        )
    }

    fn is_discovery_expired_at(&self, now_ms: u64) -> bool {
        let Some(entry) = self.load() else {
            return true;
        };
        if entry.discovery.is_none() {
            return true;
        }
        let Some(updated_ms) = entry
            .discovery_updated_at
            .as_deref()
            .and_then(parse_iso_timestamp_ms)
        else {
            return true;
        };
        now_ms.saturating_sub(updated_ms) > DISCOVERY_TTL_MS
    }

    pub(super) fn save_discovery(&self, discovery: &DiscoveryResult, project: &GitLabProject) {
        let existing = self.load();
        let now = format_iso_timestamp_at(OffsetDateTime::now_utc());
        self.save(CacheEntry {
            discovery: Some(discovery.clone()),
            project: Some(project.clone()),
            selected_model_ref: existing.as_ref().and_then(|e| e.selected_model_ref.clone()),
            selected_model_name: existing
                .as_ref()
                .and_then(|e| e.selected_model_name.clone()),
            updated_at: Some(now.clone()),
            discovery_updated_at: Some(now),
        });
    }

    pub(super) fn save_selection(&self, r#ref: &str, name: Option<&str>) {
        let existing = self.load();
        let resolved_name = name.map(str::to_owned).or_else(|| {
            existing.as_ref().and_then(|e| {
                e.discovery.as_ref().and_then(|d| {
                    d.pinned_model
                        .as_ref()
                        .filter(|m| m.r#ref == r#ref)
                        .map(|m| m.name.clone())
                        .or_else(|| {
                            d.selectable_models
                                .iter()
                                .find(|m| m.r#ref == r#ref)
                                .map(|m| m.name.clone())
                        })
                        .or_else(|| {
                            d.default_model
                                .as_ref()
                                .filter(|m| m.r#ref == r#ref)
                                .map(|m| m.name.clone())
                        })
                })
            })
        });
        self.save(CacheEntry {
            discovery: existing.as_ref().and_then(|e| e.discovery.clone()),
            project: existing.as_ref().and_then(|e| e.project.clone()),
            selected_model_ref: Some(r#ref.to_owned()),
            selected_model_name: resolved_name,
            updated_at: Some(format_iso_timestamp_at(OffsetDateTime::now_utc())),
            discovery_updated_at: existing
                .as_ref()
                .and_then(|e| e.discovery_updated_at.clone()),
        });
    }

    pub(super) fn get_selected_model_ref(&self) -> Option<String> {
        self.load().and_then(|e| e.selected_model_ref)
    }
}

fn format_iso_timestamp_at(timestamp: OffsetDateTime) -> String {
    timestamp
        .to_offset(time::UtcOffset::UTC)
        .format(time::macros::format_description!(
            "[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]Z"
        ))
        .expect("UTC date and time provide every timestamp component")
}

fn parse_iso_timestamp_ms(timestamp: &str) -> Option<u64> {
    let timestamp = OffsetDateTime::parse(timestamp, &Rfc3339).ok()?;
    if timestamp.unix_timestamp_nanos() < 0 {
        return None;
    }
    u64::try_from(timestamp.unix_timestamp_nanos() / 1_000_000).ok()
}

#[cfg(test)]
mod tests {
    use super::{CacheEntry, FileCache, format_iso_timestamp_at, parse_iso_timestamp_ms};
    use crate::providers::gitlab_dws_discovery::{DiscoveredModel, DiscoveryResult};
    use time::Duration;
    use time::macros::datetime;

    #[test]
    fn calendar_rollovers_round_trip_exact_milliseconds() {
        for (start, expected) in [
            (
                datetime!(2024-02-28 23:59:59.999 UTC),
                "2024-02-29T00:00:00.000Z",
            ),
            (
                datetime!(2024-02-29 23:59:59.999 UTC),
                "2024-03-01T00:00:00.000Z",
            ),
            (
                datetime!(2025-02-28 23:59:59.999 UTC),
                "2025-03-01T00:00:00.000Z",
            ),
            (
                datetime!(2026-04-30 23:59:59.999 UTC),
                "2026-05-01T00:00:00.000Z",
            ),
            (
                datetime!(2026-12-31 23:59:59.999 UTC),
                "2027-01-01T00:00:00.000Z",
            ),
        ] {
            let timestamp = start + Duration::milliseconds(1);
            assert_eq!(format_iso_timestamp_at(timestamp), expected);
            assert_eq!(
                parse_iso_timestamp_ms(expected),
                Some((timestamp.unix_timestamp_nanos() / 1_000_000) as u64)
            );
        }
        assert_eq!(
            parse_iso_timestamp_ms("2024-02-29T00:00:00.123Z"),
            Some(1_709_164_800_123)
        );
        assert_eq!(
            parse_iso_timestamp_ms("2024-02-29T03:00:00.123+03:00"),
            Some(1_709_164_800_123)
        );
        assert_eq!(
            format_iso_timestamp_at(datetime!(2024-02-29 03:00:00.123 +03:00)),
            "2024-02-29T00:00:00.123Z"
        );
    }

    #[test]
    fn invalid_calendar_values_and_pre_epoch_times_are_rejected() {
        for timestamp in [
            "2025-02-29T00:00:00Z",
            "2024-02-30T00:00:00Z",
            "2024-13-01T00:00:00Z",
            "2024-01-01T24:00:00Z",
            "2024-01-01T00:00:00",
            "1969-12-31T23:59:59Z",
            "1969-12-31T23:59:59.999999999Z",
            "garbage",
        ] {
            assert_eq!(parse_iso_timestamp_ms(timestamp), None, "{timestamp}");
        }
    }

    #[test]
    fn persisted_cache_ttl_crosses_leap_day_and_selection_preserves_discovery_age() {
        let dir = tempfile::tempdir().unwrap();
        let cache = FileCache {
            file_path: dir.path().join("cache.json"),
            key: "scope".into(),
        };
        let updated = "2024-02-29T23:55:00.000Z";
        cache.save(CacheEntry {
            discovery: Some(DiscoveryResult {
                default_model: Some(DiscoveredModel {
                    name: "Default".into(),
                    r#ref: "default".into(),
                }),
                selectable_models: vec![],
                pinned_model: None,
                model_switching_enabled: true,
                instance_version: None,
            }),
            project: None,
            selected_model_ref: None,
            selected_model_name: None,
            updated_at: Some(updated.into()),
            discovery_updated_at: Some(updated.into()),
        });
        let boundary = parse_iso_timestamp_ms("2024-03-01T00:05:00.000Z").unwrap();
        assert!(!cache.is_discovery_expired_at(boundary));
        assert!(cache.is_discovery_expired_at(boundary + 1));
        cache.save_selection("default", None);
        let entry = cache.load().unwrap();
        assert_eq!(entry.discovery_updated_at.as_deref(), Some(updated));
        assert_eq!(entry.selected_model_name.as_deref(), Some("Default"));
        assert_eq!(cache.get_selected_model_ref().as_deref(), Some("default"));
        assert!(cache.is_discovery_expired_at(boundary + 1));
        cache.save(CacheEntry {
            discovery_updated_at: Some("2025-02-29T00:00:00Z".into()),
            ..entry
        });
        assert!(cache.is_discovery_expired_at(boundary));
    }
}
