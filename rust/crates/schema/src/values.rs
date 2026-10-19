//! Purpose: shared scalar values from `packages/schema/src/schema.ts` (Finite, Int,
//! NonNegativeInt, PositiveInt, DateTimeUtcFromMillis, RelativePath, AbsolutePath) and the
//! `optional` field helper, with JavaScript-compatible JSON number semantics.
//! Scope: pure values.
//! Uses:
//! rust/crates/
//! `-- schema/src/lib.rs
//! External:
//! |-- de::Error
//! |-- de::Error::custom
//! |-- fmt::Formatter
//! |-- fmt::Result
//! |-- serde::Deserialize
//! |-- serde::Deserializer
//! |-- serde::Serialize
//! |-- serde::Serializer
//! |-- serde::de
//! |-- serde::de::Visitor
//! |-- serde::de::self
//! |-- serde::ser::Error::custom
//! |-- serde_json::Map
//! |-- serde_json::Value
//! |-- serde_json::Value::deserialize
//! |-- serde_json::from_str
//! |-- serde_json::from_value
//! |-- serde_json::json
//! `-- serde_json::to_string
//! Used by:
//! rust/crates/
//! |-- locations/src/ports.rs
//! |-- locations/src/project_store.rs
//! |-- runtime/src/adapters.rs
//! |-- runtime/src/execution_support.rs
//! |-- runtime/src/location.rs
//! |-- runtime/src/process.rs
//! |-- runtime/tests/process.rs
//! |-- schema/src/agent.rs
//! |-- schema/src/event.rs
//! |-- schema/src/filesystem.rs
//! |-- schema/src/integration.rs
//! |-- schema/src/lib.rs
//! |-- schema/src/location.rs
//! |-- schema/src/misc_event.rs
//! |-- schema/src/permission.rs
//! |-- schema/src/project.rs
//! |-- schema/src/prompt.rs
//! |-- schema/src/provider.rs
//! |-- schema/src/pty.rs
//! |-- schema/src/question.rs
//! |-- schema/src/reference.rs
//! |-- schema/src/revert.rs
//! |-- schema/src/session.rs
//! |-- schema/src/session_aux_event.rs
//! |-- schema/src/session_event.rs
//! |-- schema/src/session_event/stream.rs
//! |-- schema/src/session_input.rs
//! |-- schema/src/session_message.rs
//! |-- schema/src/session_message/assistant.rs
//! |-- schema/src/workspace.rs
//! |-- server/src/handlers/session.rs
//! |-- server/src/middleware/location.rs
//! |-- session-api/src/ids.rs
//! |-- session-api/tests/wire.rs
//! |-- session-runtime/src/admission.rs
//! |-- session-runtime/src/compaction.rs
//! |-- session-runtime/src/inbox.rs
//! |-- session-runtime/src/legacy/types.rs
//! |-- session-runtime/src/projection.rs
//! |-- session-runtime/src/projection/updater.rs
//! |-- session-runtime/src/revert.rs
//! |-- session-runtime/src/runner/model.rs
//! |-- session-runtime/src/runner/ports.rs
//! |-- session-runtime/src/runner/publish.rs
//! |-- session-runtime/src/runner/publish/tools.rs
//! |-- session-runtime/src/runner/turn.rs
//! |-- session-runtime/src/runner/turn/attempt.rs
//! |-- session-runtime/src/service.rs
//! |-- session-runtime/src/store.rs
//! |-- session-runtime/tests/admission.rs
//! |-- session-runtime/tests/admission_historical.rs
//! |-- session-runtime/tests/admission_inbox.rs
//! |-- session-runtime/tests/admission_projection.rs
//! `-- session-runtime/tests/support/provider.rs
//! Invariants: numbers encode like `JSON.stringify` for integral values (no `.0`, `-0` -> `0`);
//! Int accepts any JSON number that is a safe integer (`3.0` decodes, `2^53` fails);
//! DateTimeUtcFromMillis truncates fractional millis toward zero like `new Date(x)` and
//! rejects values outside +-8.64e15; optional keys omit `None` and reject explicit `null`.
//! Contract tests: schema/src/values.rs (unit tests), schema/tests/baseline_fixtures.rs

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

/// Largest integer that `Number.isSafeInteger` accepts.
pub const MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;
/// Largest absolute millisecond value accepted by a JavaScript `Date`.
pub const MAX_DATE_MILLIS: i64 = 8_640_000_000_000_000;

/// Reads any JSON number as an IEEE double, like `JSON.parse`.
fn number<'de, D: Deserializer<'de>>(deserializer: D) -> Result<f64, D::Error> {
    struct NumberVisitor;
    impl Visitor<'_> for NumberVisitor {
        type Value = f64;
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("a number")
        }
        fn visit_i64<E: de::Error>(self, v: i64) -> Result<f64, E> {
            Ok(v as f64)
        }
        fn visit_u64<E: de::Error>(self, v: u64) -> Result<f64, E> {
            Ok(v as f64)
        }
        fn visit_f64<E: de::Error>(self, v: f64) -> Result<f64, E> {
            Ok(v)
        }
    }
    deserializer.deserialize_any(NumberVisitor)
}

/// Serializes a double the way `JSON.stringify` does for integral values.
/// Non-integral values use the shortest round-trip form (serde_json/ryu), which matches
/// JavaScript except for exponent spelling outside `1e-7..1e21` (recorded in contract.md).
fn serialize_js_number<S: Serializer>(value: f64, serializer: S) -> Result<S::Ok, S::Error> {
    if value.fract() == 0.0 && value.abs() < 1e21 {
        if value == 0.0 {
            return serializer.serialize_i64(0);
        }
        if value.abs() <= MAX_SAFE_INTEGER as f64 {
            return serializer.serialize_i64(value as i64);
        }
        // Above 2^53 JavaScript prints the shortest round-trip digits padded with zeros
        // (1.2345678901234568e20 -> "123456789012345680000"). Rust `Display` does the same.
        let digits = format!("{}", value.abs());
        let magnitude: u128 = digits.parse().map_err(serde::ser::Error::custom)?;
        return match value < 0.0 {
            true => serializer.serialize_i128(-(magnitude as i128)),
            false => serializer.serialize_u128(magnitude),
        };
    }
    serializer.serialize_f64(value)
}

/// `Schema.Finite` (and `Schema.Number` where only finite values occur on the wire).
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Default)]
pub struct Finite(f64);

impl Finite {
    /// Returns `None` for NaN or infinite values.
    pub fn new(value: f64) -> Option<Self> {
        value.is_finite().then_some(Self(value))
    }
    pub const fn get(self) -> f64 {
        self.0
    }
}

impl From<i32> for Finite {
    fn from(value: i32) -> Self {
        Self(value as f64)
    }
}

impl From<u32> for Finite {
    fn from(value: u32) -> Self {
        Self(value as f64)
    }
}

impl Serialize for Finite {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serialize_js_number(self.0, serializer)
    }
}

impl<'de> Deserialize<'de> for Finite {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = number(deserializer)?;
        Finite::new(value).ok_or_else(|| de::Error::custom("Expected a finite number"))
    }
}

macro_rules! int_type {
    ($(#[$meta:meta])* $name:ident, $min:expr, $message:literal) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(i64);

        impl $name {
            /// Returns `None` when the value violates the baseline check.
            pub const fn new(value: i64) -> Option<Self> {
                if value < $min || value > MAX_SAFE_INTEGER {
                    return None;
                }
                Some(Self(value))
            }
            pub const fn get(self) -> i64 {
                self.0
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_i64(self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let value = number(deserializer)?;
                if value.fract() != 0.0 || value.abs() > MAX_SAFE_INTEGER as f64 {
                    return Err(de::Error::custom(format!("Expected an integer, got {value}")));
                }
                $name::new(value as i64)
                    .ok_or_else(|| de::Error::custom(format!($message, value)))
            }
        }
    };
}

int_type!(
    /// `Schema.Int`: a safe integer.
    Int,
    -MAX_SAFE_INTEGER,
    "Expected an integer, got {}"
);
int_type!(
    /// `NonNegativeInt`: a safe integer `>= 0`.
    NonNegativeInt,
    0,
    "Expected a value greater than or equal to 0, got {}"
);
int_type!(
    /// `PositiveInt`: a safe integer `> 0`.
    PositiveInt,
    1,
    "Expected a value greater than 0, got {}"
);

impl Int {
    pub const ZERO: Int = Int(0);
}

impl NonNegativeInt {
    pub const ZERO: NonNegativeInt = NonNegativeInt(0);
}

/// `DateTimeUtcFromMillis`: a UTC instant encoded as epoch milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DateTimeUtc(i64);

impl DateTimeUtc {
    /// Returns `None` outside the JavaScript `Date` range.
    pub const fn from_millis(millis: i64) -> Option<Self> {
        if millis < -MAX_DATE_MILLIS || millis > MAX_DATE_MILLIS {
            return None;
        }
        Some(Self(millis))
    }
    pub const fn millis(self) -> i64 {
        self.0
    }
    /// Current wall-clock time, like `DateTime.nowUnsafe()`.
    pub fn now() -> Self {
        let millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis() as i64)
            .unwrap_or(0);
        Self(millis)
    }
}

impl Serialize for DateTimeUtc {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_i64(self.0)
    }
}

impl<'de> Deserialize<'de> for DateTimeUtc {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = number(deserializer)?;
        if !value.is_finite() {
            return Err(de::Error::custom("Expected a finite number"));
        }
        DateTimeUtc::from_millis(value.trunc() as i64)
            .filter(|_| value.abs() <= MAX_DATE_MILLIS as f64)
            .ok_or_else(|| de::Error::custom("Invalid date"))
    }
}

crate::brand!(
    /// `RelativePath` brand. No validation in the baseline.
    RelativePath
);
crate::brand!(
    /// `AbsolutePath` brand. No validation in the baseline.
    AbsolutePath
);

/// Deserializer for `optional(...)` fields: absent key -> `None`, explicit `null` is an
/// error (the baseline rejects `null` and `undefined`). Use together with
/// `#[serde(default, skip_serializing_if = "Option::is_none")]`.
pub mod optional {
    use serde::{Deserialize, Deserializer, de};

    pub fn deserialize<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
    where
        D: Deserializer<'de>,
        T: Deserialize<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        if value.is_null() {
            return Err(de::Error::custom("Expected a value, got null"));
        }
        T::deserialize(value).map(Some).map_err(de::Error::custom)
    }
}

/// `Schema.Record(Schema.String, Schema.Unknown)` and `Schema.Json` objects, insertion ordered.
pub type JsonObject = serde_json::Map<String, serde_json::Value>;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn encode<T: Serialize>(value: &T) -> String {
        serde_json::to_string(value).unwrap()
    }

    #[test]
    fn finite_encodes_like_json_stringify() {
        let cases = [
            ("1.0", "1"),
            ("-0.0", "0"),
            ("0.1", "0.1"),
            ("123456789012345680000", "123456789012345680000"),
            ("-1.5", "-1.5"),
        ];
        cases.iter().for_each(|(input, expected)| {
            let value: Finite = serde_json::from_str(input).unwrap();
            assert_eq!(encode(&value), *expected, "input {input}");
        });
    }

    #[test]
    fn number_spelling_matches_bun() {
        // fixture: captured from bun 1.3.14 JSON.stringify at baseline 907b3bc51.
        let input = "[1.5e21, 1e-7, 1.5e-8, 0.30000000000000004, 9007199254740992, -0.0, 1e300, 5e-324, 123.456]";
        let values: Vec<Finite> = serde_json::from_str(input).unwrap();
        assert_eq!(
            encode(&values),
            "[1.5e+21,1e-7,1.5e-8,0.30000000000000004,9007199254740992,0,1e+300,5e-324,123.456]"
        );
    }

    #[test]
    fn int_accepts_integral_floats_and_rejects_unsafe() {
        let value: Int = serde_json::from_str("3.0").unwrap();
        assert_eq!(value.get(), 3);
        assert!(serde_json::from_str::<Int>("9007199254740992").is_err());
        assert!(serde_json::from_str::<Int>("1.5").is_err());
        assert!(serde_json::from_str::<NonNegativeInt>("-1").is_err());
        assert!(serde_json::from_str::<PositiveInt>("0").is_err());
        assert!(serde_json::from_str::<PositiveInt>("\"1\"").is_err());
    }

    #[test]
    fn millis_truncate_toward_zero_and_check_range() {
        // Observed baseline: {t: 1.7} -> 1, {t: -1.7} -> -1, {t: 9e15} -> Invalid date.
        let up: DateTimeUtc = serde_json::from_value(json!(1.7)).unwrap();
        let down: DateTimeUtc = serde_json::from_value(json!(-1.7)).unwrap();
        assert_eq!((up.millis(), down.millis()), (1, -1));
        assert!(serde_json::from_value::<DateTimeUtc>(json!(9e15)).is_err());
        assert_eq!(encode(&up), "1");
    }

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct Probe {
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "optional::deserialize"
        )]
        o: Option<String>,
        n: Finite,
    }

    #[test]
    fn optional_omits_none_and_rejects_null() {
        let probe: Probe = serde_json::from_str(r#"{"n":1,"extra":true}"#).unwrap();
        assert_eq!(encode(&probe), r#"{"n":1}"#);
        assert!(serde_json::from_str::<Probe>(r#"{"o":null,"n":1}"#).is_err());
        let full: Probe = serde_json::from_str(r#"{"n":2,"o":"x"}"#).unwrap();
        assert_eq!(encode(&full), r#"{"o":"x","n":2}"#);
    }
}
