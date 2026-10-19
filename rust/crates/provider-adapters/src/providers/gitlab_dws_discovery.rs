//! Purpose: GitLab duo-workflow model discovery from `gitlab-ai-provider`:
//! `GitLabProjectDetector` (git remote parsing for SSH/HTTPS URLs), `GitLabModelDiscovery`
//! (GraphQL `aiChatAvailableModels` query with namespace fallback), `GitLabModelCache`
//! (file-based cache with 10-minute TTL), and `discover_workflow_models`/`build_models` entry
//! points that map discovered models to route IDs using the existing `MODEL_MAPPINGS` concept.
//! Scope: project detection, GraphQL discovery, file cache, model mapping.
//! Uses:
//! rust/crates/
//! |-- llm-api/src/lib.rs
//! `-- provider-adapters/src/providers/gitlab_dws_discovery/cache.rs
//! External:
//! |-- reqwest::Client
//! |-- reqwest::Client::new
//! |-- serde_json::Map
//! |-- serde_json::Value
//! |-- serde_json::json
//! |-- str::to_owned
//! |-- tokio::process::Command::new
//! |-- url::Url::parse
//! `-- url::form_urlencoded::byte_serialize
//! Used by:
//! rust/crates/
//! |-- provider-adapters/src/providers/gitlab_dws_discovery/cache.rs
//! `-- provider-adapters/src/providers/mod.rs
//! Invariants: the cache file lives at `$XDG_CACHE_HOME/opencode/gitlab-workflow-model-cache.json`
//! (or `~/.cache/opencode/...`); discovery results are cached per scope with a 10-minute TTL;
//! when project-scoped discovery fails, it falls back to namespace-scoped discovery.
//! Contract tests: child cache module tests cover calendar rollover, TTL and persisted selections; no live GitLab discovery test.

mod cache;

use cache::FileCache;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use opencode_llm_api::LlmError;
use serde_json::{Map, Value};
const VERSION: &str = "6.21.0";

/// `GitLabProjectDetector` parsed project.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct GitLabProject {
    pub id: u64,
    pub path: String,
    pub path_with_namespace: String,
    pub name: String,
    pub namespace_id: Option<u64>,
}

/// `GitLabModelDiscovery` discovered model.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct DiscoveredModel {
    pub name: String,
    pub r#ref: String,
}

/// `GitLabModelDiscovery` discovery result.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct DiscoveryResult {
    pub default_model: Option<DiscoveredModel>,
    pub selectable_models: Vec<DiscoveredModel>,
    pub pinned_model: Option<DiscoveredModel>,
    pub model_switching_enabled: bool,
    pub instance_version: Option<String>,
}

/// Configuration for workflow discovery.
pub struct WorkflowDiscoveryConfig {
    pub instance_url: String,
    pub get_headers: Arc<dyn Fn() -> HashMap<String, String> + Send + Sync>,
}

/// Options for workflow discovery.
pub struct WorkflowDiscoveryOptions {
    pub working_directory: String,
    pub cache_key: Option<String>,
}

/// Result of workflow discovery.
pub struct WorkflowDiscoveryResult {
    pub models: Vec<DiscoveredWorkflowModel>,
    pub project: Option<GitLabProject>,
}

/// `GitLabProjectDetector` + `GitLabModelDiscovery` + `GitLabModelCache` combined.
pub struct ModelDiscovery {
    instance_url: String,
    get_headers: Arc<dyn Fn() -> HashMap<String, String> + Send + Sync>,
    working_dir: String,
    cache_key: Option<String>,
    client: reqwest::Client,
}

impl ModelDiscovery {
    pub fn new(config: WorkflowDiscoveryConfig, options: WorkflowDiscoveryOptions) -> Self {
        Self {
            instance_url: config.instance_url,
            get_headers: config.get_headers,
            working_dir: options.working_directory,
            cache_key: options.cache_key,
            client: reqwest::Client::new(),
        }
    }

    /// `discoverWorkflowModels`: check cache, detect project, discover models, build result.
    pub async fn discover_workflow_models(&self) -> Result<WorkflowDiscoveryResult, LlmError> {
        let cache = self.file_cache();
        if !cache.is_discovery_expired()
            && let Some(entry) = cache.load()
            && let Some(discovery) = entry.discovery
        {
            let models = build_models(&discovery, entry.project.as_ref());
            return Ok(WorkflowDiscoveryResult {
                models,
                project: entry.project,
            });
        }
        let project = match self.detect_project().await {
            Ok(p) => p,
            Err(_) => {
                return Ok(WorkflowDiscoveryResult {
                    models: vec![],
                    project: None,
                });
            }
        };
        let Some(project) = project else {
            return Ok(WorkflowDiscoveryResult {
                models: vec![],
                project: None,
            });
        };
        let scope = DiscoveryScope {
            project_id: Some(format!("gid://gitlab/Project/{}", project.id)),
            root_namespace_id: project
                .namespace_id
                .map(|id| format!("gid://gitlab/Group/{}", id)),
        };
        let discovery = match self.discover(&scope).await {
            Ok(d) => d,
            Err(_) => {
                return Ok(WorkflowDiscoveryResult {
                    models: vec![],
                    project: Some(project.clone()),
                });
            }
        };
        cache.save_discovery(&discovery, &project);
        if cache.get_selected_model_ref().is_none() {
            let pick = discovery
                .pinned_model
                .as_ref()
                .or(discovery.default_model.as_ref());
            if let Some(model) = pick {
                cache.save_selection(&model.r#ref, Some(&model.name));
            }
        }
        let models = build_models(&discovery, Some(&project));
        Ok(WorkflowDiscoveryResult {
            models,
            project: Some(project),
        })
    }

    /// `GitLabProjectDetector.detectProject`.
    async fn detect_project(&self) -> Result<Option<GitLabProject>, LlmError> {
        let remote_url = self.get_git_remote_url().await?;
        let Some(remote_url) = remote_url else {
            return Ok(None);
        };
        let project_path = match self.parse_git_remote_url(&remote_url) {
            Some(p) => p,
            None => return Ok(None),
        };
        self.get_project_by_path(&project_path).await.map(Some)
    }

    /// `getGitRemoteUrl`: run `git config --get remote.origin.url`.
    async fn get_git_remote_url(&self) -> Result<Option<String>, LlmError> {
        let output = tokio::process::Command::new("git")
            .args(["config", "--get", "remote.origin.url"])
            .current_dir(&self.working_dir)
            .output()
            .await
            .map_err(|e| LlmError::invalid_request(format!("git config failed: {e}")))?;
        if output.status.success() {
            let url = String::from_utf8_lossy(&output.stdout).trim().to_owned();
            if url.is_empty() {
                Ok(None)
            } else {
                Ok(Some(url))
            }
        } else {
            Ok(None)
        }
    }

    /// `parseGitRemoteUrl`: SSH (`git@host:namespace/project.git`), HTTPS, HTTP.
    fn parse_git_remote_url(&self, remote_url: &str) -> Option<String> {
        let instance_host = url::Url::parse(&self.instance_url)
            .ok()
            .map(|u| u.host_str().map(str::to_owned))?
            .unwrap_or_default();
        if remote_url.starts_with("ssh://") {
            if let Ok(ssh_url) = url::Url::parse(remote_url)
                && ssh_url.host_str() == Some(&instance_host)
            {
                let path = ssh_url.path().trim_start_matches('/');
                return Some(path.strip_suffix(".git").unwrap_or(path).to_owned());
            }
            return None;
        }
        if let Some((host, path)) = remote_url.strip_prefix("git@").and_then(|rest| {
            rest.split_once(':').map(|(h, p)| {
                let path = p.strip_suffix(".git").unwrap_or(p);
                (h.to_owned(), path.to_owned())
            })
        }) && host == instance_host
        {
            return Some(path);
        }
        if let Ok(https_url) = url::Url::parse(remote_url)
            && matches!(https_url.scheme(), "https" | "http")
            && https_url.host_str() == Some(&instance_host)
        {
            let path = https_url.path().trim_start_matches('/');
            return Some(path.strip_suffix(".git").unwrap_or(path).to_owned());
        }
        None
    }

    /// `getProjectByPath`: REST API call.
    async fn get_project_by_path(&self, project_path: &str) -> Result<GitLabProject, LlmError> {
        let encoded =
            url::form_urlencoded::byte_serialize(project_path.as_bytes()).collect::<String>();
        let url = format!(
            "{}/api/v4/projects/{}",
            self.instance_url.trim_end_matches('/'),
            encoded
        );
        let headers = (self.get_headers)();
        let mut request = self
            .client
            .get(&url)
            .header("user-agent", format!("ai-sdk-gitlab/{VERSION}"));
        for (key, value) in headers {
            request = request.header(key, value);
        }
        let response = request
            .send()
            .await
            .map_err(|e| LlmError::invalid_request(format!("Project fetch failed: {e}")))?;
        if !response.status().is_success() {
            return Err(LlmError::invalid_request(format!(
                "Failed to fetch project '{}': {} {}",
                project_path,
                response.status().as_u16(),
                response.status().canonical_reason().unwrap_or("")
            )));
        }
        let data: Value = response
            .json()
            .await
            .map_err(|e| LlmError::invalid_request(format!("Project fetch failed: {e}")))?;
        Ok(GitLabProject {
            id: data["id"].as_u64().unwrap_or(0),
            path: data["path"].as_str().unwrap_or("").to_owned(),
            path_with_namespace: data["path_with_namespace"]
                .as_str()
                .unwrap_or("")
                .to_owned(),
            name: data["name"].as_str().unwrap_or("").to_owned(),
            namespace_id: data["namespace"]["id"].as_u64(),
        })
    }

    /// `GitLabModelDiscovery.discover` with namespace fallback.
    async fn discover(&self, scope: &DiscoveryScope) -> Result<DiscoveryResult, LlmError> {
        match self.query(scope).await {
            Ok(result) => Ok(result),
            Err(error) => {
                if scope.project_id.is_some() && scope.root_namespace_id.is_some() {
                    let fallback = DiscoveryScope {
                        project_id: None,
                        root_namespace_id: scope.root_namespace_id.clone(),
                    };
                    self.query(&fallback).await
                } else {
                    Err(error)
                }
            }
        }
    }

    /// `GitLabModelDiscovery.query`: GraphQL `aiChatAvailableModels`.
    async fn query(&self, scope: &DiscoveryScope) -> Result<DiscoveryResult, LlmError> {
        let url = format!("{}/api/graphql", self.instance_url.trim_end_matches('/'));
        let headers = (self.get_headers)();
        let query = r#"
          query aiChatAvailableModels($projectId: ProjectID, $rootNamespaceId: GroupID) {
            metadata {
              featureFlags(names: ["ai_user_model_switching"]) {
                enabled
                name
              }
              version
            }
            aiChatAvailableModels(projectId: $projectId, rootNamespaceId: $rootNamespaceId) {
              defaultModel {
                name
                ref
              }
              selectableModels {
                name
                ref
              }
              pinnedModel {
                name
                ref
              }
            }
          }
        "#;
        let mut variables = Map::new();
        if let Some(project_id) = &scope.project_id {
            variables.insert("projectId".into(), Value::String(project_id.clone()));
        }
        if let Some(root_namespace_id) = &scope.root_namespace_id {
            variables.insert(
                "rootNamespaceId".into(),
                Value::String(root_namespace_id.clone()),
            );
        }
        let body = serde_json::json!({
            "query": query,
            "variables": variables,
        });
        let mut request = self
            .client
            .post(&url)
            .header("user-agent", format!("ai-sdk-gitlab/{VERSION}"))
            .header("content-type", "application/json");
        for (key, value) in headers {
            request = request.header(key, value);
        }
        let response = request.json(&body).send().await.map_err(|e| {
            LlmError::invalid_request(format!("Model discovery GraphQL request failed: {e}"))
        })?;
        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(LlmError::invalid_request(format!(
                "Model discovery GraphQL request failed: {} {} - {}",
                status.as_u16(),
                status.canonical_reason().unwrap_or(""),
                text
            )));
        }
        let json: Value = response
            .json()
            .await
            .map_err(|e| LlmError::invalid_request(format!("Model discovery failed: {e}")))?;
        if let Some(errors) = json["errors"].as_array()
            && !errors.is_empty()
        {
            let messages: Vec<String> = errors
                .iter()
                .filter_map(|e| e["message"].as_str().map(str::to_owned))
                .collect();
            return Err(LlmError::invalid_request(format!(
                "Model discovery GraphQL errors: {}",
                messages.join(", ")
            )));
        }
        let models = &json["data"]["aiChatAvailableModels"];
        let metadata = &json["data"]["metadata"];
        let flag = metadata["featureFlags"].as_array().and_then(|flags| {
            flags
                .iter()
                .find(|f| f["name"].as_str() == Some("ai_user_model_switching"))
        });
        let model_switching_enabled = flag.is_none_or(|f| f["enabled"].as_bool().unwrap_or(true));
        let parse_model = |v: &Value| -> Option<DiscoveredModel> {
            let name = v["name"].as_str()?.to_owned();
            let r#ref = v["ref"].as_str()?.to_owned();
            Some(DiscoveredModel { name, r#ref })
        };
        Ok(DiscoveryResult {
            default_model: parse_model(&models["defaultModel"]),
            selectable_models: models["selectableModels"]
                .as_array()
                .map(|arr| arr.iter().filter_map(parse_model).collect())
                .unwrap_or_default(),
            pinned_model: parse_model(&models["pinnedModel"]),
            model_switching_enabled,
            instance_version: metadata["version"].as_str().map(str::to_owned),
        })
    }

    /// `GitLabModelCache` file path and key.
    fn file_cache(&self) -> FileCache {
        FileCache::new(
            &self.working_dir,
            &self.instance_url,
            self.cache_key.as_deref(),
        )
    }
}

struct DiscoveryScope {
    project_id: Option<String>,
    root_namespace_id: Option<String>,
}

/// `buildModels`: map discovered models to route IDs using `MODEL_MAPPINGS` concept.
pub fn build_models(
    discovered: &DiscoveryResult,
    _project: Option<&GitLabProject>,
) -> Vec<DiscoveredWorkflowModel> {
    const WORKFLOW_MAPPINGS: &[(&str, &str)] = &[
        ("default", "duo-workflow-default"),
        ("claude_sonnet_4_5_20250929", "duo-workflow-sonnet-4-5"),
        ("claude_sonnet_5", "duo-workflow-sonnet-5"),
        ("claude_opus_5_5", "duo-workflow-opus-5-5"),
        ("claude_opus_5", "duo-workflow-opus-5"),
        ("claude_sonnet_4_6", "duo-workflow-sonnet-4-6"),
        ("claude_opus_4_5_20251101", "duo-workflow-opus-4-5"),
        ("claude_haiku_4_5_20251001", "duo-workflow-haiku-4-5"),
        ("claude_opus_4_6_20260205", "duo-workflow-opus-4-6"),
    ];
    let ref_to_id: HashMap<String, String> = WORKFLOW_MAPPINGS
        .iter()
        .map(|(r, id)| (r.to_string(), id.to_string()))
        .collect();
    let all: Vec<&DiscoveredModel> = if let Some(pinned) = &discovered.pinned_model {
        vec![pinned]
    } else {
        discovered
            .selectable_models
            .iter()
            .chain(discovered.default_model.iter())
            .collect()
    };
    let mut seen = HashSet::new();
    let mut models = Vec::new();
    for model in all {
        if model.r#ref.is_empty() || !seen.insert(&model.r#ref) {
            continue;
        }
        let id = ref_to_id.get(&model.r#ref).cloned().unwrap_or_else(|| {
            if model.r#ref.starts_with("__default__") {
                "duo-workflow-default".to_owned()
            } else {
                let sanitized = model.r#ref.replace(['/', '_'], "-");
                format!("duo-workflow-{}", sanitized)
            }
        });
        let pinned = discovered
            .pinned_model
            .as_ref()
            .is_some_and(|p| p.r#ref == model.r#ref);
        models.push(DiscoveredWorkflowModel {
            id,
            r#ref: model.r#ref.clone(),
            name: model.name.clone(),
            context: 200_000,
            output: 64_000,
            pinned,
        });
    }
    models
}

/// One model from `buildModels`.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct DiscoveredWorkflowModel {
    pub id: String,
    pub r#ref: String,
    pub name: String,
    pub context: usize,
    pub output: usize,
    pub pinned: bool,
}

/// Top-level entry point matching TypeScript `discoverWorkflowModels`.
pub async fn discover_workflow_models(
    config: WorkflowDiscoveryConfig,
    options: WorkflowDiscoveryOptions,
) -> Result<WorkflowDiscoveryResult, LlmError> {
    let discovery = ModelDiscovery::new(config, options);
    discovery.discover_workflow_models().await
}
