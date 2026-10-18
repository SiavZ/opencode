//! Purpose: generate and inspect sortable prefixed identifiers, byte-compatible with
//! `packages/schema/src/identifier.ts` and `packages/core/src/id/id.ts`. Does not own
//! any domain-specific ID brand validation beyond the prefix rule.
//! Scope: process-wide monotonic counter, pure helpers otherwise.
//! Uses:
//! `-- none
//! External:
//! |-- getrandom::fill
//! `-- std::sync::Mutex
//! Used by:
//! rust/crates/
//! `-- schema/src/lib.rs
//! Invariants: 12 lowercase hex chars encode the low 48 bits of `timestamp * 0x1000 + counter`
//! (bitwise NOT for descending), followed by 14 base62 chars chosen as `byte % 62`.
//! The counter resets whenever the millisecond timestamp changes, like the baseline.
//! Contract tests: schema/src/identifier.rs (unit tests)

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const LENGTH: usize = 26;
const CHARS: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

static STATE: Mutex<(u64, u64)> = Mutex::new((0, 0));

/// Errors produced while validating or decoding identifiers.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IdentifierError {
    #[error("ID {id} does not start with {prefix}")]
    WrongPrefix { id: String, prefix: String },
    #[error("ID {0} has no valid timestamp section")]
    InvalidTimestamp(String),
}

/// Sort direction of the encoded time section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Ascending,
    Descending,
}

/// Well-known prefixes from `packages/core/src/id/id.ts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prefix {
    Job,
    Event,
    Session,
    Message,
    Permission,
    Question,
    Part,
    Pty,
    Tool,
    Workspace,
}

impl Prefix {
    pub const fn as_str(self) -> &'static str {
        match self {
            Prefix::Job => "job",
            Prefix::Event => "evt",
            Prefix::Session => "ses",
            Prefix::Message => "msg",
            Prefix::Permission => "per",
            Prefix::Question => "que",
            Prefix::Part => "prt",
            Prefix::Pty => "pty",
            Prefix::Tool => "tool",
            Prefix::Workspace => "wrk",
        }
    }
}

/// Unprefixed 26-character identifier body, ascending order.
pub fn ascending() -> String {
    body(Direction::Ascending, now_millis())
}

/// Unprefixed 26-character identifier body, descending order.
pub fn descending() -> String {
    body(Direction::Descending, now_millis())
}

/// Equivalent of the baseline `create(descending, timestamp)`.
pub fn body(direction: Direction, timestamp: u64) -> String {
    let counter = {
        let mut state = STATE.lock().unwrap_or_else(|poison| poison.into_inner());
        if state.0 != timestamp {
            *state = (timestamp, 0);
        }
        state.1 += 1;
        state.1
    };
    encode(direction, timestamp, counter, &random_bytes())
}

/// Deterministic encoder used by `body`, exposed for fixture comparison.
pub fn encode(
    direction: Direction,
    timestamp: u64,
    counter: u64,
    random: &[u8; LENGTH - 12],
) -> String {
    let current = timestamp.wrapping_mul(0x1000).wrapping_add(counter);
    let value = match direction {
        Direction::Ascending => current,
        Direction::Descending => !current,
    };
    let mut out = String::with_capacity(LENGTH);
    (0..6).for_each(|index| {
        let byte = (value >> (40 - 8 * index)) & 0xff;
        out.push_str(&format!("{byte:02x}"));
    });
    random
        .iter()
        .for_each(|byte| out.push(CHARS[(*byte % 62) as usize] as char));
    out
}

/// `prefix_<body>` identifier, like `Identifier.create(prefix, direction)`.
pub fn create(prefix: &str, direction: Direction, timestamp: Option<u64>) -> String {
    format!(
        "{prefix}_{}",
        body(direction, timestamp.unwrap_or_else(now_millis))
    )
}

/// Validates a given identifier or creates a new one, like `ascending(prefix, given)`.
pub fn generate(
    prefix: Prefix,
    direction: Direction,
    given: Option<&str>,
) -> Result<String, IdentifierError> {
    match given {
        None => Ok(create(prefix.as_str(), direction, None)),
        Some(id) if id.starts_with(prefix.as_str()) => Ok(id.to_string()),
        Some(id) => Err(IdentifierError::WrongPrefix {
            id: id.to_string(),
            prefix: prefix.as_str().to_string(),
        }),
    }
}

/// Millisecond timestamp of an ascending identifier. Not meaningful for descending IDs.
pub fn timestamp(id: &str) -> Result<u64, IdentifierError> {
    let prefix_len = id.split('_').next().map(str::len).unwrap_or(0);
    let hex = id
        .get(prefix_len + 1..prefix_len + 13)
        .ok_or_else(|| IdentifierError::InvalidTimestamp(id.to_string()))?;
    u64::from_str_radix(hex, 16)
        .map(|encoded| encoded / 0x1000)
        .map_err(|_| IdentifierError::InvalidTimestamp(id.to_string()))
}

fn random_bytes() -> [u8; LENGTH - 12] {
    let mut bytes = [0u8; LENGTH - 12];
    // A failing OS random source is unrecoverable for ID generation, like crypto.getRandomValues.
    getrandom::fill(&mut bytes).expect("operating system random source unavailable");
    bytes
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_ascending_time_section() {
        // 1700000000000 * 0x1000 + 1 = 0x18bcfe56800_001 -> low 48 bits.
        let id = encode(Direction::Ascending, 1_700_000_000_000, 1, &[0; 14]);
        assert_eq!(id.len(), 26);
        assert_eq!(&id[12..], "00000000000000");
        let expected = (1_700_000_000_000u64 * 0x1000 + 1) & 0xffff_ffff_ffff;
        assert_eq!(&id[..12], format!("{expected:012x}"));
    }

    #[test]
    fn descending_is_bitwise_not_of_ascending() {
        let up = encode(Direction::Ascending, 42, 3, &[61; 14]);
        let down = encode(Direction::Descending, 42, 3, &[61; 14]);
        let up_bits = u64::from_str_radix(&up[..12], 16).unwrap();
        let down_bits = u64::from_str_radix(&down[..12], 16).unwrap();
        assert_eq!(up_bits ^ down_bits, 0xffff_ffff_ffff);
        assert!(down.ends_with("zzzzzzzzzzzzzz"));
    }

    #[test]
    fn random_section_uses_modulo_62() {
        let id = encode(
            Direction::Ascending,
            0,
            0,
            &[62, 63, 123, 255, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        );
        assert_eq!(&id[12..16], "01z7");
    }

    #[test]
    fn counter_increments_within_same_millisecond() {
        // Other tests generate IDs concurrently and may reset the shared counter between the
        // two calls, so retry until two consecutive calls share the millisecond state.
        let ordered = (0..100).any(|_| {
            let a = body(Direction::Ascending, 5);
            let b = body(Direction::Ascending, 5);
            a[..12] < b[..12]
        });
        assert!(ordered);
    }

    #[test]
    fn matches_baseline_observed_time_sections() {
        // Observed with bun 1.3.14 from packages/schema/src/identifier.ts at 907b3bc51.
        let id = encode(Direction::Ascending, 1_700_000_000_000, 1, &[0; 14]);
        assert_eq!(&id[..12], "bcfe56800001");
    }

    #[test]
    fn timestamp_keeps_baseline_48_bit_truncation() {
        // The 48-bit section truncates `timestamp * 0x1000`, so the baseline decodes
        // `timestamp mod 2^36`. Observed baseline result for 1759000000123: 41013081723.
        let id = create("ses", Direction::Ascending, Some(1_759_000_000_123));
        assert_eq!(timestamp(&id).unwrap(), 41_013_081_723);
    }

    #[test]
    fn generate_validates_prefix() {
        assert!(generate(Prefix::Session, Direction::Descending, Some("ses_abc")).is_ok());
        let error = generate(Prefix::Session, Direction::Descending, Some("msg_abc")).unwrap_err();
        assert_eq!(error.to_string(), "ID msg_abc does not start with ses");
        assert!(
            generate(Prefix::Tool, Direction::Ascending, None)
                .unwrap()
                .starts_with("tool_")
        );
    }
}
