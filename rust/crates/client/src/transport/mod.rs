//! Purpose: transport boundary shared by typed operations. Request/response values are
//! target-neutral; native and browser adapters implement them behind cfg.
//! Scope: request descriptor and response values only, adapters live in sibling modules.
//! Uses:
//! rust/crates/
//! `-- client/src/errors.rs
//! External:
//! `-- url::Url
//! Used by:
//! rust/crates/
//! `-- client/src/lib.rs
//! Invariants: mirrors `RequestDescriptor` of packages/client/src/generated/client.ts:129-138.
//! Contract tests: client/tests (Phase 08)

use url::Url;

/// One HTTP request as described by a generated operation.
#[derive(Debug, Clone, PartialEq)]
pub struct RequestDescriptor {
    pub method: &'static str,
    /// Already percent-encoded path, e.g. `/api/session/ses_1/message`.
    pub path: String,
    /// Query entries in insertion order; nested objects use `key[child]` bracket form.
    pub query: Vec<(String, serde_json::Value)>,
    pub headers: Vec<(String, String)>,
    pub body: Option<serde_json::Value>,
    pub success_status: u16,
    pub declared_statuses: &'static [u16],
    pub empty: bool,
}

impl RequestDescriptor {
    /// Resolve against `base` and append query with bracket serialization
    /// (`appendQuery`, packages/client/src/generated/client.ts:993-1004).
    pub fn url(&self, base: &Url) -> Result<Url, url::ParseError> {
        let mut url = base.join(&self.path)?;
        {
            let mut pairs = url.query_pairs_mut();
            for (key, value) in &self.query {
                append_query(&mut pairs, key, value);
            }
        }
        if url.query() == Some("") {
            url.set_query(None);
        }
        Ok(url)
    }
}

fn append_query(
    pairs: &mut url::form_urlencoded::Serializer<'_, url::UrlQuery<'_>>,
    key: &str,
    value: &serde_json::Value,
) {
    match value {
        serde_json::Value::Null => {}
        serde_json::Value::Array(items) => {
            items.iter().for_each(|item| append_query(pairs, key, item))
        }
        serde_json::Value::Object(map) => map
            .iter()
            .for_each(|(child, item)| append_query(pairs, &format!("{key}[{child}]"), item)),
        serde_json::Value::String(text) => {
            pairs.append_pair(key, text);
        }
        other => {
            pairs.append_pair(key, &other.to_string());
        }
    }
}
