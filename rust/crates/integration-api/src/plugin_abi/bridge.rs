//! Purpose: generate the JS bridge script for the plugin compatibility host from the Rust ABI
//! method table, error codes and trigger names.
//! Scope: pure string generation over a static template (`bridge.js`).
//! Uses:
//! rust/crates/
//! |-- integration-api/src/plugin.rs
//! `-- integration-api/src/plugin_abi/mod.rs
//! External:
//! `-- serde_json::to_string
//! Used by:
//! rust/crates/
//! `-- integration-api/src/plugin_abi/mod.rs
//! Invariants: every host-to-bridge method gets a HANDLERS entry and every bridge-to-host method
//! gets a `host.<js>` stub, both derived from `METHODS`. The template never names a method string.
//! Contract tests: integration-api/src/plugin_abi/bridge.rs (tests module)

use super::{ABI_VERSION, CallKind, Direction, ErrorCode, METHODS};
use crate::plugin::TriggerName;

const TEMPLATE: &str = include_str!("bridge.js");
const MARKER: &str = "__GENERATED__";

/// The complete bridge module source (ES module, runs under bun or node).
pub fn bridge_script() -> String {
    TEMPLATE.replacen(MARKER, &generated(), 1)
}

fn generated() -> String {
    let quote = |s: &str| serde_json::to_string(s).unwrap_or_default();
    let mut out = format!("const ABI_VERSION = {ABI_VERSION}\n");
    out.push_str("const ErrorCode = Object.freeze({\n");
    for code in ErrorCode::ALL {
        out.push_str(&format!("  {}: {},\n", code.js_name(), code.code()));
    }
    out.push_str("})\nconst TRIGGERS = Object.freeze([\n");
    for name in TriggerName::ALL {
        out.push_str(&format!("  {},\n", quote(name.as_str())));
    }
    out.push_str("])\nconst HANDLERS = {\n");
    for spec in METHODS
        .iter()
        .filter(|s| s.direction == Direction::HostToBridge)
    {
        out.push_str(&format!(
            "  {}: (params) => {}(params),\n",
            quote(spec.name),
            spec.js
        ));
    }
    out.push_str("}\nconst host = {\n");
    for spec in METHODS
        .iter()
        .filter(|s| s.direction == Direction::BridgeToHost)
    {
        let call = match spec.kind {
            CallKind::Request => "request",
            CallKind::Notification => "notify",
        };
        out.push_str(&format!(
            "  {}: (params) => {call}({}, params),\n",
            spec.js,
            quote(spec.name)
        ));
    }
    out.push('}');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin_abi::method;

    #[test]
    fn script_embeds_every_method_and_the_abi_version() {
        let script = bridge_script();
        assert!(!script.contains(MARKER));
        assert!(script.contains(&format!("const ABI_VERSION = {ABI_VERSION}\n")));
        for spec in METHODS {
            let name = serde_json::to_string(spec.name).unwrap();
            match spec.direction {
                Direction::HostToBridge => {
                    assert!(script.contains(&format!("  {name}: (params) => {}(params)", spec.js)));
                    assert!(
                        script.contains(&format!("async function {}(params", spec.js))
                            || script.contains(&format!("async function {}()", spec.js)),
                        "{} handler missing",
                        spec.js
                    );
                }
                Direction::BridgeToHost => assert!(script.contains(&format!("({name}, params)"))),
            }
        }
        for t in TriggerName::ALL {
            assert!(script.contains(&format!("  \"{}\",\n", t.as_str())));
        }
        for c in ErrorCode::ALL {
            assert!(script.contains(&format!("  {}: {},\n", c.js_name(), c.code())));
            assert_eq!(ErrorCode::from_code(c.code()), Some(c));
        }
    }

    #[test]
    fn template_never_hardcodes_method_names() {
        for spec in METHODS {
            let quoted = serde_json::to_string(spec.name).unwrap();
            assert!(
                !TEMPLATE.contains(&quoted),
                "{} is hardcoded in bridge.js",
                spec.name
            );
        }
        assert_eq!(method("hook.trigger").map(|m| m.js), Some("onHookTrigger"));
        let names: std::collections::BTreeSet<_> = METHODS.iter().map(|m| m.name).collect();
        assert_eq!(names.len(), METHODS.len());
    }
}
