//! Purpose: legacy agent listing with baseline defaults, permissions and config overlays.
//! Scope: materialized legacy configuration plus native resource directories.
//! Uses:
//! rust/crates/
//! |-- config/src/value.rs
//! `-- runtime/src/global_services.rs
//! External:
//! `-- serde_json::Value
//! Used by:
//! rust/crates/
//! `-- runtime/src/global_services.rs
//! Invariants: disabled agents are removed; permissions retain ordered last-match semantics.
//! Contract tests: runtime/tests/global_services.rs

use super::{GlobalServices, LegacyError, RouteContext, Value, json};
use std::path::Path;

impl GlobalServices {
    pub(super) async fn agents(&self, context: &RouteContext) -> Result<Value, LegacyError> {
        let config = self.resolved_config(&context.directory).await?;
        let truncate = format!(
            "{}/*",
            self.options.paths.data.join("tool-output").display()
        );
        let temporary = format!("{}/*", self.options.paths.tmp.display());
        let mut whitelist = vec![truncate.clone(), temporary];
        for directory in opencode_config::legacy::directories(
            &self.options.environment,
            &opencode_config::legacy::Instance {
                directory: context.directory.clone().into(),
                worktree: context.directory.clone().into(),
            },
        ) {
            for name in ["skill", "skills"] {
                if directory.join(name).is_dir() {
                    whitelist.push(format!("{}/*", directory.join(name).display()));
                }
            }
        }
        let mut external = json!({"*":"ask"});
        for path in whitelist {
            external[path] = json!("allow");
        }
        let defaults = permission(
            &json!({"*":"allow","doom_loop":"ask","external_directory":external,"question":"deny","plan_enter":"deny","plan_exit":"deny","read":{"*":"allow","*.env":"ask","*.env.*":"ask","*.env.example":"allow"}}),
            &self.options.paths.home,
        );
        let user = permission(
            config.get("permission").unwrap_or(&json!({})),
            &self.options.paths.home,
        );
        let mut agents = serde_json::Map::new();
        for (name, description, mode, hidden, prompt, extra) in [
            (
                "build",
                Some("The default agent. Executes tools based on configured permissions."),
                "primary",
                false,
                None,
                json!({"question":"allow","plan_enter":"allow"}),
            ),
            (
                "plan",
                Some("Plan mode. Disallows all edit tools."),
                "primary",
                false,
                None,
                json!({"question":"allow","plan_exit":"allow","task":{"general":"deny"},"external_directory":{format!("{}/*",self.options.paths.data.join("plans").display()):"allow"},"edit":{"*":"deny",".opencode/plans/*.md":"allow",format!("{}/*.md",self.options.paths.data.join("plans").display()):"allow"}}),
            ),
            (
                "general",
                Some(
                    "General-purpose agent for researching complex questions and executing multi-step tasks. Use this agent to execute multiple units of work in parallel.",
                ),
                "subagent",
                false,
                None,
                json!({"todowrite":"deny"}),
            ),
            (
                "explore",
                Some(
                    "Fast agent specialized for exploring codebases. Use this when you need to quickly find files by patterns (eg. \"src/components/**/*.tsx\"), search code for keywords (eg. \"API endpoints\"), or answer questions about the codebase (eg. \"how do API endpoints work?\"). When calling this agent, specify the desired thoroughness level: \"quick\" for basic searches, \"medium\" for moderate exploration, or \"very thorough\" for comprehensive analysis across multiple locations and naming conventions.",
                ),
                "subagent",
                false,
                Some(include_str!(
                    "../../../../../packages/opencode/src/agent/prompt/explore.txt"
                )),
                json!({"*":"deny","grep":"allow","glob":"allow","list":"allow","bash":"allow","webfetch":"allow","websearch":"allow","read":"allow","external_directory":external}),
            ),
            (
                "compaction",
                None,
                "primary",
                true,
                Some(include_str!(
                    "../../../../../packages/opencode/src/agent/prompt/compaction.txt"
                )),
                json!({"*":"deny"}),
            ),
            (
                "title",
                None,
                "primary",
                true,
                Some(include_str!(
                    "../../../../../packages/opencode/src/agent/prompt/title.txt"
                )),
                json!({"*":"deny"}),
            ),
            (
                "summary",
                None,
                "primary",
                true,
                Some(include_str!(
                    "../../../../../packages/opencode/src/agent/prompt/summary.txt"
                )),
                json!({"*":"deny"}),
            ),
        ] {
            let rules = defaults
                .iter()
                .cloned()
                .chain(permission(&extra, &self.options.paths.home))
                .chain(user.clone())
                .collect::<Vec<_>>();
            let mut item =
                json!({"name":name,"mode":mode,"native":true,"options":{},"permission":rules});
            if hidden {
                item["hidden"] = json!(true);
            }
            if let Some(description) = description {
                item["description"] = json!(description);
            }
            if let Some(prompt) = prompt {
                item["prompt"] = json!(prompt);
            }
            if name == "title" {
                item["temperature"] = json!(0.5);
            }
            agents.insert(name.into(), item);
        }
        for (name, value) in config
            .get("agent")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
        {
            if value.get("disable").and_then(Value::as_bool) == Some(true) {
                agents.shift_remove(name);
                continue;
            }
            let item = agents.entry(name.clone()).or_insert_with(||json!({"name":name,"mode":"all","native":false,"options":{},"permission":defaults.iter().cloned().chain(user.clone()).collect::<Vec<_>>()}));
            if let Some(model) = value
                .get("model")
                .and_then(Value::as_str)
                .and_then(|model| model.split_once('/'))
            {
                item["model"] = json!({"providerID":model.0,"modelID":model.1});
            }
            for (source, target) in [
                ("variant", "variant"),
                ("prompt", "prompt"),
                ("description", "description"),
                ("temperature", "temperature"),
                ("top_p", "topP"),
                ("mode", "mode"),
                ("color", "color"),
                ("hidden", "hidden"),
                ("name", "name"),
                ("steps", "steps"),
            ] {
                if let Some(value) = value.get(source).filter(|value| !value.is_null()) {
                    item[target] = value.clone();
                }
            }
            item["options"] = opencode_config::value::merge_deep(
                &item["options"],
                value.get("options").unwrap_or(&json!({})),
            );
            item["permission"]
                .as_array_mut()
                .expect("rules array")
                .extend(permission(
                    value.get("permission").unwrap_or(&json!({})),
                    &self.options.paths.home,
                ));
        }
        for item in agents.values_mut() {
            let rules = item["permission"].as_array_mut().expect("rules array");
            if !rules.iter().any(|rule| {
                rule["permission"] == "external_directory"
                    && rule["action"] == "deny"
                    && rule["pattern"] == truncate
            }) {
                rules.push(
                    json!({"permission":"external_directory","pattern":truncate,"action":"allow"}),
                );
            }
        }
        let default = config
            .get("default_agent")
            .and_then(Value::as_str)
            .unwrap_or("build");
        let mut agents = agents.into_values().collect::<Vec<_>>();
        agents.sort_by(|a, b| {
            (b["name"] == default)
                .cmp(&(a["name"] == default))
                .then_with(|| a["name"].as_str().cmp(&b["name"].as_str()))
        });
        Ok(Value::Array(agents))
    }
}

fn permission(value: &Value, home: &Path) -> Vec<Value> {
    value
        .as_object()
        .into_iter()
        .flatten()
        .flat_map(|(name, action)| {
            let rules = action
                .as_object()
                .map(|patterns| {
                    patterns
                        .iter()
                        .map(|(pattern, action)| (pattern.as_str(), action))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_else(|| vec![("*", action)]);
            rules
                .into_iter()
                .map(|(pattern, action)| {
                    let pattern = pattern
                        .strip_prefix("~/")
                        .or_else(|| pattern.strip_prefix("$HOME/"))
                        .map(|path| home.join(path).display().to_string())
                        .unwrap_or_else(|| pattern.to_owned());
                    json!({"permission":name,"pattern":pattern,"action":action})
                })
                .collect::<Vec<_>>()
        })
        .collect()
}
