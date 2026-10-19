//! Purpose: declarative macros that generate branded string newtypes (`Schema.brand`) with
//! optional prefix checks (`Schema.isStartsWith`) and literal string tags.
//! Scope: compile-time code generation only.
//! Uses:
//! `-- none
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! `-- schema/src/lib.rs
//! Invariants: a branded value serializes as a bare JSON string. Prefix-checked brands fail
//! decoding with `Expected a string starting with "<prefix>", got "<value>"` like the baseline.
//! Contract tests: schema/src/ids.rs (unit tests)

/// Declares a branded string newtype.
///
/// `brand!(Name)` accepts any string. `brand!(Name, "ses")` additionally requires the prefix,
/// matching `Schema.String.check(Schema.isStartsWith("ses")).pipe(Schema.brand(...))`.
#[macro_export]
macro_rules! brand {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(
            Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, ::serde::Serialize, ::serde::Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Wraps a string without validation (the baseline brand has no check).
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }
            pub fn as_str(&self) -> &str {
                &self.0
            }
            pub fn into_inner(self) -> String {
                self.0
            }
        }

        $crate::brand!(@common $name);
    };
    ($(#[$meta:meta])* $name:ident, $prefix:literal) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, ::serde::Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Prefix required by the baseline `isStartsWith` check.
            pub const PREFIX: &'static str = $prefix;

            /// Validates the baseline prefix check.
            pub fn new(value: impl Into<String>) -> Result<Self, $crate::ids::BrandError> {
                let value = value.into();
                if value.starts_with($prefix) {
                    return Ok(Self(value));
                }
                Err($crate::ids::BrandError { prefix: $prefix, value })
            }
            pub fn as_str(&self) -> &str {
                &self.0
            }
            pub fn into_inner(self) -> String {
                self.0
            }
        }

        impl<'de> ::serde::Deserialize<'de> for $name {
            fn deserialize<D: ::serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let value = String::deserialize(deserializer)?;
                $name::new(value).map_err(::serde::de::Error::custom)
            }
        }

        impl TryFrom<String> for $name {
            type Error = $crate::ids::BrandError;
            fn try_from(value: String) -> Result<Self, Self::Error> {
                $name::new(value)
            }
        }

        impl TryFrom<&str> for $name {
            type Error = $crate::ids::BrandError;
            fn try_from(value: &str) -> Result<Self, Self::Error> {
                $name::new(value)
            }
        }

        $crate::brand!(@common $name);
    };
    (@common $name:ident) => {
        impl ::std::fmt::Display for $name {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl ::std::borrow::Borrow<str> for $name {
            fn borrow(&self) -> &str {
                &self.0
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> String {
                value.0
            }
        }
    };
}

/// Declares a unit struct that serializes as one fixed string (`Schema.Literal("x")`).
/// Used for single-literal fields such as `type: "aisdk"` inside non-tagged structs.
#[macro_export]
macro_rules! literal {
    ($(#[$meta:meta])* $name:ident = $value:literal) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
        pub struct $name;

        impl $name {
            pub const VALUE: &'static str = $value;
        }

        impl ::serde::Serialize for $name {
            fn serialize<S: ::serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str($value)
            }
        }

        impl<'de> ::serde::Deserialize<'de> for $name {
            fn deserialize<D: ::serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let value = String::deserialize(deserializer)?;
                if value == $value {
                    return Ok($name);
                }
                Err(::serde::de::Error::custom(format!(
                    "Expected \"{}\", got \"{}\"",
                    $value, value
                )))
            }
        }
    };
}
