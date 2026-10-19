//! Purpose: persist baseline-compatible prompt history without dropping attachment or mode fields.
//! Scope: terminal application's state directory.
//! Uses:
//! `-- none
//! External:
//! |-- serde_json::Value
//! `-- tokio::fs
//! Used by:
//! rust/bins/
//! `-- opencode/src/cli/commands/terminal.rs
//! Invariants: retain the last 50 valid JSON entries; adjacent exact duplicates do not append.
//! Contract tests: unit tests in this file and terminal_app PTY history test

use serde_json::Value;
use std::path::PathBuf;

pub struct History {
    path: PathBuf,
    entries: Vec<Value>,
}

impl History {
    pub async fn load(path: PathBuf) -> Self {
        let text = tokio::fs::read_to_string(&path).await.unwrap_or_default();
        let entries = text
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .collect::<Vec<_>>();
        let history = Self {
            path,
            entries: entries
                .into_iter()
                .rev()
                .take(50)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect(),
        };
        if !history.entries.is_empty() {
            let _ = history.save().await;
        }
        history
    }

    pub fn inputs(&self) -> Vec<String> {
        self.entries
            .iter()
            .filter_map(|entry| entry["input"].as_str().map(str::to_owned))
            .collect()
    }

    pub async fn append(&mut self, input: &str) -> std::io::Result<()> {
        let entry = serde_json::json!({"input":input,"parts":[]});
        if self.entries.last() == Some(&entry) {
            return Ok(());
        }
        self.entries.push(entry);
        if self.entries.len() > 50 {
            self.entries.remove(0);
        }
        self.save().await
    }

    async fn save(&self) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let text = self
            .entries
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        tokio::fs::write(&self.path, text).await
    }
}

#[cfg(test)]
mod tests {
    use super::History;
    use serde_json::Value;

    #[tokio::test]
    async fn repairs_corruption_preserves_parts_and_deduplicates_exact_entries() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("history.jsonl");
        let entry = serde_json::json!({"input":"kept","mode":"shell","parts":[{"type":"file","url":"file:///tmp/example","mime":"text/plain"}]});
        tokio::fs::write(&path, format!("broken\n{entry}\n"))
            .await
            .unwrap();
        let mut history = History::load(path.clone()).await;
        assert_eq!(history.inputs(), ["kept"]);
        history.append("new").await.unwrap();
        history.append("new").await.unwrap();
        let lines = tokio::fs::read_to_string(path).await.unwrap();
        let entries = lines
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            entries,
            [entry, serde_json::json!({"input":"new","parts":[]})]
        );
    }

    #[tokio::test]
    async fn caps_history_at_fifty_entries() {
        let root = tempfile::tempdir().unwrap();
        let mut history = History::load(root.path().join("history.jsonl")).await;
        for index in 0..55 {
            history.append(&format!("entry {index}")).await.unwrap();
        }
        let history = History::load(history.path).await;
        assert_eq!(history.inputs().len(), 50);
        assert_eq!(history.inputs()[0], "entry 5");
        assert_eq!(history.inputs()[49], "entry 54");
    }
}
