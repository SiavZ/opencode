//! Purpose: expose workspace provider, model, and usage graph settings actions.
//! Scope: authenticated JSON alternatives to Solid server functions.
//! Uses:
//! rust/bins/
//! |-- hosted/src/console/auth.rs
//! `-- hosted/src/console/workspace.rs
//! rust/crates/
//! `-- hosted-adapters/src/console/tenant.rs
//! External:
//! |-- axum::Router
//! `-- serde_json::Value
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/workspace.rs
//! Invariants: mutations require same-origin and actor-scoped admin repository checks.
//! Contract tests: workspace/settings/tests.rs

use super::*;
use serde_json::Value;

pub(super) fn routes() -> Router<ConsoleState> {
    Router::new()
        .route(
            "/workspaces/{id}/providers",
            get(providers).post(save_provider),
        )
        .route(
            "/workspaces/{id}/providers/{provider}",
            axum::routing::delete(remove_provider),
        )
        .route("/workspaces/{id}/models", get(models))
        .route("/workspaces/{id}/models/toggle", post(toggle_model))
        .route("/workspaces/{id}/usage/graph", get(usage_graph))
}

#[derive(Deserialize)]
struct GraphInput {
    year: i32,
    month: i32,
    #[serde(rename = "tzOffset")]
    tz_offset: String,
}

async fn usage_graph(
    State(state): State<ConsoleState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    axum::extract::Query(input): axum::extract::Query<GraphInput>,
) -> Result<Response, Response> {
    let mut session = auth::read_session(&state, &headers)?;
    let actor = auth::resolve_actor(&state, &mut session, Some(&id), &Method::GET).await?;
    require_workspace(&actor, &id)?;
    let graph = TenantStore::new(state.db.clone())
        .usage_graph(&actor, input.year, input.month, &input.tz_offset)
        .await
        .map_err(tenant_error)?;
    auth::with_session(&state, &session, Json(graph).into_response())
}

#[derive(Deserialize)]
struct ProviderInput {
    provider: String,
    credentials: String,
}

#[derive(Deserialize)]
struct ModelInput {
    model: String,
    // This is the current state from model.toggle, not the requested state.
    enabled: bool,
}

async fn providers(
    State(state): State<ConsoleState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    let mut session = auth::read_session(&state, &headers)?;
    let actor = auth::resolve_actor(&state, &mut session, Some(&id), &Method::GET).await?;
    require_workspace(&actor, &id)?;
    let rows = TenantStore::new(state.db.clone())
        .list_providers(&actor)
        .await
        .map_err(tenant_error)?;
    auth::with_session(&state, &session, Json(rows).into_response())
}

async fn save_provider(
    State(state): State<ConsoleState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    input: Result<Json<ProviderInput>, JsonRejection>,
) -> Result<Response, Response> {
    auth::require_same_origin(&state, &headers)?;
    let mut session = auth::read_session(&state, &headers)?;
    let actor = auth::resolve_actor(&state, &mut session, Some(&id), &Method::POST).await?;
    require_workspace(&actor, &id)?;
    let Json(input) = input.map_err(json_error)?;
    if input.provider.is_empty() || input.credentials.is_empty() {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "Provider and API key are required",
        ));
    }
    TenantStore::new(state.db.clone())
        .save_provider(&actor, &input.provider, &input.credentials)
        .await
        .map_err(tenant_error)?;
    auth::with_session(&state, &session, StatusCode::NO_CONTENT.into_response())
}

async fn remove_provider(
    State(state): State<ConsoleState>,
    headers: HeaderMap,
    Path((id, provider)): Path<(String, String)>,
) -> Result<Response, Response> {
    auth::require_same_origin(&state, &headers)?;
    let mut session = auth::read_session(&state, &headers)?;
    let actor = auth::resolve_actor(&state, &mut session, Some(&id), &Method::DELETE).await?;
    require_workspace(&actor, &id)?;
    TenantStore::new(state.db.clone())
        .remove_provider(&actor, &provider)
        .await
        .map_err(tenant_error)?;
    auth::with_session(&state, &session, StatusCode::NO_CONTENT.into_response())
}

async fn models(
    State(state): State<ConsoleState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    let mut session = auth::read_session(&state, &headers)?;
    let actor = auth::resolve_actor(&state, &mut session, Some(&id), &Method::GET).await?;
    require_workspace(&actor, &id)?;
    let catalog = state.config.zen.workspace_models().ok_or_else(|| {
        error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Zen model catalog is not configured",
        )
    })?;
    let all = model_choices(catalog);
    let disabled = TenantStore::new(state.db.clone())
        .disabled_models(&actor)
        .await
        .map_err(tenant_error)?;
    auth::with_session(
        &state,
        &session,
        Json(json!({"all": all, "disabled": disabled})).into_response(),
    )
}

fn model_choices(catalog: &serde_json::Map<String, Value>) -> Vec<Value> {
    let priority = [
        "big-pickle",
        "claude",
        "gpt",
        "gemini",
        "deepseek",
        "glm",
        "kimi",
        "qwen",
        "grok",
        "minimax",
        "mimo",
    ];
    let mut models = catalog
        .iter()
        .filter(|(id, _)| {
            id.as_str() != "claude-3-5-haiku"
                && !id.starts_with("alpha-")
                && !id.ends_with(":global")
        })
        .filter_map(|(id, model)| {
            let model = model
                .as_array()
                .and_then(|models| models.first())
                .unwrap_or(model);
            model
                .get("name")
                .and_then(Value::as_str)
                .map(|name| (id, name))
        })
        .collect::<Vec<_>>();
    models.sort_by(|(id_a, name_a), (id_b, name_b)| {
        let rank = |id: &str| {
            priority
                .iter()
                .position(|prefix| id.starts_with(prefix))
                .unwrap_or(usize::MAX)
        };
        rank(id_a)
            .cmp(&rank(id_b))
            .then_with(|| name_a.to_lowercase().cmp(&name_b.to_lowercase()))
    });
    models
        .into_iter()
        .map(|(id, name)| json!({"id": id, "name": name}))
        .collect()
}

async fn toggle_model(
    State(state): State<ConsoleState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    input: Result<Json<ModelInput>, JsonRejection>,
) -> Result<Response, Response> {
    auth::require_same_origin(&state, &headers)?;
    let mut session = auth::read_session(&state, &headers)?;
    let actor = auth::resolve_actor(&state, &mut session, Some(&id), &Method::POST).await?;
    require_workspace(&actor, &id)?;
    let Json(input) = input.map_err(json_error)?;
    if input.model.is_empty() {
        return Err(error(StatusCode::BAD_REQUEST, "Model is required"));
    }
    let store = TenantStore::new(state.db.clone());
    if input.enabled {
        store
            .disable_model(&actor, &input.model)
            .await
            .map_err(tenant_error)?;
    } else {
        store
            .enable_model(&actor, &input.model)
            .await
            .map_err(tenant_error)?;
    }
    auth::with_session(&state, &session, StatusCode::NO_CONTENT.into_response())
}

#[cfg(test)]
mod tests;
