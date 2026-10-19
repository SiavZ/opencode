//! Purpose: `reference.ts` references and `skill.ts` skills.
//! Scope: pure values.
//! Uses:
//! rust/crates/
//! |-- schema/src/event.rs
//! `-- schema/src/values.rs
//! External:
//! |-- serde::Deserialize
//! |-- serde::Deserializer
//! |-- serde::Serialize
//! |-- serde::Serializer
//! |-- serde::de::Error
//! |-- serde::de::Error::custom
//! |-- serde_json::Value::as_str
//! |-- serde_json::Value::deserialize
//! `-- str::to_owned
//! Used by:
//! rust/crates/
//! |-- schema/src/lib.rs
//! `-- schema/tests/schema_domains_reference.rs
//! Invariants: byte-identical JSON with the baseline module (owner: phase-01 child node, in progress).
//! Contract tests: schema/tests/schema_domains_reference.rs

use crate::event::{Definition, EventData};
use crate::values::{AbsolutePath, optional};
use serde::{Deserialize, Serialize};

/// `Reference.LocalSource`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalSource {
    pub r#type: LocalType,
    pub path: AbsolutePath,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub description: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub hidden: Option<bool>,
}

/// Literal tag for `LocalSource`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LocalType;

impl LocalType {
    pub const VALUE: &'static str = "local";
}

impl Serialize for LocalType {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str("local")
    }
}

impl<'de> Deserialize<'de> for LocalType {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        if value == "local" {
            return Ok(LocalType);
        }
        Err(serde::de::Error::custom(format!(
            "Expected \"local\", got \"{}\"",
            value
        )))
    }
}

/// `Reference.GitSource`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitSource {
    pub r#type: GitType,
    pub repository: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub branch: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub description: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub hidden: Option<bool>,
}

/// Literal tag for `GitSource`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GitType;

impl GitType {
    pub const VALUE: &'static str = "git";
}

impl Serialize for GitType {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str("git")
    }
}

impl<'de> Deserialize<'de> for GitType {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        if value == "git" {
            return Ok(GitType);
        }
        Err(serde::de::Error::custom(format!(
            "Expected \"git\", got \"{}\"",
            value
        )))
    }
}

/// `Reference.Source` tagged union on `type`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Local(LocalSource),
    Git(GitSource),
}

impl Serialize for Source {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Local(value) => value.serialize(serializer),
            Self::Git(value) => value.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for Source {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let value = serde_json::Value::deserialize(deserializer)?;
        let tag = value
            .get("type")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        match tag.as_deref() {
            Some("local") => LocalSource::deserialize(value)
                .map(Self::Local)
                .map_err(D::Error::custom),
            Some("git") => GitSource::deserialize(value)
                .map(Self::Git)
                .map_err(D::Error::custom),
            _ => Err(D::Error::custom(format!(
                "Expected Reference.Source, got {}",
                value
            ))),
        }
    }
}

/// `Reference.Info`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Info {
    pub name: String,
    pub path: AbsolutePath,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub description: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub hidden: Option<bool>,
    pub source: Source,
}

/// `reference.updated` event payload (empty schema in baseline).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Updated {}

impl EventData for Updated {
    const DEFINITION: Definition = Definition {
        r#type: "reference.updated",
        durable: None,
    };
}

/// `SkillV2.DirectorySource`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectorySource {
    pub r#type: DirectoryType,
    pub path: AbsolutePath,
}

/// Literal tag for `DirectorySource`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DirectoryType;

impl DirectoryType {
    pub const VALUE: &'static str = "directory";
}

impl Serialize for DirectoryType {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str("directory")
    }
}

impl<'de> Deserialize<'de> for DirectoryType {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        if value == "directory" {
            return Ok(DirectoryType);
        }
        Err(serde::de::Error::custom(format!(
            "Expected \"directory\", got \"{}\"",
            value
        )))
    }
}

/// `SkillV2.UrlSource`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UrlSource {
    pub r#type: UrlType,
    pub url: String,
}

/// Literal tag for `UrlSource`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UrlType;

impl UrlType {
    pub const VALUE: &'static str = "url";
}

impl Serialize for UrlType {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str("url")
    }
}

impl<'de> Deserialize<'de> for UrlType {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        if value == "url" {
            return Ok(UrlType);
        }
        Err(serde::de::Error::custom(format!(
            "Expected \"url\", got \"{}\"",
            value
        )))
    }
}

/// `SkillV2.Info`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillInfo {
    pub name: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub description: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional::deserialize"
    )]
    pub slash: Option<bool>,
    pub location: AbsolutePath,
    pub content: String,
}

/// `SkillV2.EmbeddedSource`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmbeddedSource {
    pub r#type: EmbeddedType,
    pub skill: SkillInfo,
}

/// Literal tag for `EmbeddedSource`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EmbeddedType;

impl EmbeddedType {
    pub const VALUE: &'static str = "embedded";
}

impl Serialize for EmbeddedType {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str("embedded")
    }
}

impl<'de> Deserialize<'de> for EmbeddedType {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        if value == "embedded" {
            return Ok(EmbeddedType);
        }
        Err(serde::de::Error::custom(format!(
            "Expected \"embedded\", got \"{}\"",
            value
        )))
    }
}

/// `SkillV2.Source` tagged union on `type`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillSource {
    Directory(DirectorySource),
    Url(UrlSource),
    Embedded(EmbeddedSource),
}

impl Serialize for SkillSource {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Directory(value) => value.serialize(serializer),
            Self::Url(value) => value.serialize(serializer),
            Self::Embedded(value) => value.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for SkillSource {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let value = serde_json::Value::deserialize(deserializer)?;
        let tag = value
            .get("type")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        match tag.as_deref() {
            Some("directory") => DirectorySource::deserialize(value)
                .map(Self::Directory)
                .map_err(D::Error::custom),
            Some("url") => UrlSource::deserialize(value)
                .map(Self::Url)
                .map_err(D::Error::custom),
            Some("embedded") => EmbeddedSource::deserialize(value)
                .map(Self::Embedded)
                .map_err(D::Error::custom),
            _ => Err(D::Error::custom(format!(
                "Expected SkillV2.Source, got {}",
                value
            ))),
        }
    }
}
