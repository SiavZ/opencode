//! Purpose: execute generated network operations through a portable transport seam and preserve wire failure classes.
//! Scope: target-neutral request construction and response decoding, no server dependency.
//! Uses:
//! |-- rust/crates/client/src/errors.rs
//! `-- rust/crates/client/src/transport/mod.rs
//! External:
//! |-- serde
//! `-- serde_json
//! Used by:
//! `-- rust/crates/client/src/generated/mod.rs
//! Invariants: absent and null fields differ; declared errors never become successful values.
//! Contract tests: client/tests/operations.rs
use crate::errors::{ApiError, ClientError, ClientErrorReason};
use crate::transport::RequestDescriptor;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Optional<T> {
    #[default]
    Missing,
    Present(T),
}
impl<T> Optional<T> {
    pub fn is_missing(&self) -> bool {
        matches!(self, Self::Missing)
    }
}
impl<T: Serialize> Serialize for Optional<T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Missing => serializer.serialize_none(),
            Self::Present(value) => value.serialize(serializer),
        }
    }
}
impl<'de, T: Deserialize<'de>> Deserialize<'de> for Optional<T> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        T::deserialize(deserializer).map(Self::Present)
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Never {}

pub enum Body<'a> {
    None,
    Fields(&'a [&'a str]),
    Value(&'a str),
}
pub struct Spec<'a> {
    pub method: &'static str,
    pub path: &'a str,
    pub params: &'a [&'a str],
    pub query: &'a [&'a str],
    pub headers: &'a [&'a str],
    pub body: Body<'a>,
    pub status: u16,
    pub empty: bool,
    pub declared: &'static [u16],
}
pub fn descriptor(value: &Value, spec: &Spec<'_>) -> Result<RequestDescriptor, ClientError> {
    let path = spec
        .path
        .split('/')
        .map(|segment| {
            let key = if segment == "*" {
                Some("wildcard")
            } else {
                segment.strip_prefix(':')
            };
            let Some(key) = key else {
                return Ok(segment.to_owned());
            };
            let value = value
                .get(key)
                .filter(|value| !value.is_null())
                .ok_or_else(|| ClientError::new(ClientErrorReason::MalformedResponse))?;
            Ok(if segment == "*" {
                scalar(value)
                    .split('/')
                    .map(encode_component)
                    .collect::<Vec<_>>()
                    .join("/")
            } else {
                encode_component(&scalar(value))
            })
        })
        .collect::<Result<Vec<_>, ClientError>>()?
        .join("/");
    Ok(RequestDescriptor {
        method: spec.method,
        path,
        query: spec
            .query
            .iter()
            .filter_map(|key| {
                value
                    .get(*key)
                    .map(|value| ((*key).to_owned(), value.clone()))
            })
            .collect(),
        headers: spec
            .headers
            .iter()
            .filter_map(|key| {
                value
                    .get(*key)
                    .filter(|value| !value.is_null())
                    .map(|value| ((*key).to_owned(), scalar(value)))
            })
            .collect(),
        body: match spec.body {
            Body::None => None,
            Body::Fields(fields) => Some(Value::Object(
                fields
                    .iter()
                    .filter_map(|key| {
                        value
                            .get(*key)
                            .map(|value| ((*key).to_owned(), value.clone()))
                    })
                    .collect(),
            )),
            Body::Value(key) => value.get(key).cloned(),
        },
        success_status: spec.status,
        declared_statuses: spec.declared,
        empty: spec.empty,
    })
}
fn scalar(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| value.to_string())
}
fn encode_component(value: &str) -> String {
    value
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}
pub fn malformed(cause: serde_json::Error) -> ClientError {
    ClientError::with_cause(ClientErrorReason::MalformedResponse, cause)
}

#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    pub content_type: Option<String>,
    pub body: Vec<u8>,
}
/// Implementors own network configuration, cancellation and resource disposal.
pub trait Transport {
    fn execute(&self, request: RequestDescriptor) -> Result<Response, ClientError>;
}
/// Async transports may borrow an in-process router or browser fetch without blocking a runtime.
pub trait AsyncTransport {
    fn execute(
        &self,
        request: RequestDescriptor,
    ) -> impl std::future::Future<Output = Result<Response, ClientError>>;
}
pub fn decode_bytes<E: DeserializeOwned>(
    response: Response,
    success: u16,
    declared: &[u16],
) -> Result<Vec<u8>, ApiError<E>> {
    if response.status == success {
        return Ok(response.body);
    }
    decode::<Never, E>(response, success, declared, false, false).map(|value| match value {})
}

pub fn decode<T: DeserializeOwned, E: DeserializeOwned>(
    response: Response,
    success: u16,
    declared: &[u16],
    empty: bool,
    unwrap: bool,
) -> Result<T, ApiError<E>> {
    if response.status != success && !declared.contains(&response.status) {
        return Err(ClientError::unexpected_status(response.status).into());
    }
    if response.status == success && empty {
        return serde_json::from_value(Value::Null).map_err(|error| malformed(error).into());
    }
    if !response.content_type.as_deref().is_some_and(|value| {
        let value = value.split(';').next().unwrap_or_default().trim();
        value.eq_ignore_ascii_case("application/json")
            || value.to_ascii_lowercase().ends_with("+json")
    }) {
        return Err(ClientError::new(ClientErrorReason::UnsupportedContentType).into());
    }
    let value = serde_json::from_slice::<Value>(&response.body).map_err(malformed)?;
    if response.status != success {
        return Err(ApiError::Declared {
            status: response.status,
            body: serde_json::from_value(value).map_err(malformed)?,
        });
    }
    let value = if unwrap {
        value
            .get("data")
            .cloned()
            .ok_or_else(|| ClientError::new(ClientErrorReason::MalformedResponse))?
    } else {
        value
    };
    serde_json::from_value(value).map_err(|error| malformed(error).into())
}
