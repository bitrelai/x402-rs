use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;

use super::config::HookDefinition;
use super::manager::HookManager;
use crate::security::admin_auth::AdminAuth;

#[derive(Debug, Serialize)]
struct ListHooksResponse {
    hooks: HashMap<String, HookDefinition>,
}

#[derive(Debug, Serialize)]
struct ListMappingsResponse {
    mappings: HashMap<String, Vec<String>>,
}

#[derive(Debug, Serialize)]
struct SuccessResponse {
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

#[derive(Debug, Serialize)]
struct ErrorResponse {
    error: String,
}

async fn list_hooks(State(manager): State<Arc<HookManager>>) -> Response {
    let hooks = manager.get_all_hooks().await;
    Json(ListHooksResponse { hooks }).into_response()
}

async fn list_mappings(State(manager): State<Arc<HookManager>>) -> Response {
    let mappings = manager.get_all_mappings().await;
    Json(ListMappingsResponse { mappings }).into_response()
}

async fn reload_hooks(State(manager): State<Arc<HookManager>>) -> Response {
    match manager.reload().await {
        Ok(()) => {
            let hooks = manager.get_all_hooks().await;
            Json(SuccessResponse {
                success: true,
                message: Some(format!("Reloaded {} hook definitions", hooks.len())),
            })
            .into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: format!("Failed to reload hooks: {}", e),
            }),
        )
            .into_response(),
    }
}

async fn enable_hook(
    State(manager): State<Arc<HookManager>>,
    Path(name): Path<String>,
) -> Response {
    match manager.enable_hook(&name).await {
        Ok(()) => Json(SuccessResponse {
            success: true,
            message: Some(format!("Hook '{}' enabled", name)),
        })
        .into_response(),
        Err(e) => (StatusCode::NOT_FOUND, Json(ErrorResponse { error: e })).into_response(),
    }
}

async fn disable_hook(
    State(manager): State<Arc<HookManager>>,
    Path(name): Path<String>,
) -> Response {
    match manager.disable_hook(&name).await {
        Ok(()) => Json(SuccessResponse {
            success: true,
            message: Some(format!("Hook '{}' disabled", name)),
        })
        .into_response(),
        Err(e) => (StatusCode::NOT_FOUND, Json(ErrorResponse { error: e })).into_response(),
    }
}

#[derive(Debug, Serialize)]
struct HookStatusResponse {
    enabled: bool,
    hooks_count: usize,
    mappings_count: usize,
}

async fn hook_status(State(manager): State<Arc<HookManager>>) -> Response {
    let enabled = manager.is_enabled().await;
    let hooks = manager.get_all_hooks().await;
    let mappings = manager.get_all_mappings().await;

    Json(HookStatusResponse {
        enabled,
        hooks_count: hooks.len(),
        mappings_count: mappings.len(),
    })
    .into_response()
}

pub fn admin_hook_routes(manager: Arc<HookManager>, admin_auth: AdminAuth) -> Router {
    let admin_auth = Arc::new(admin_auth);

    Router::new()
        .route("/admin/hooks", get(list_hooks))
        .route("/admin/hooks/mappings", get(list_mappings))
        .route("/admin/hooks/status", get(hook_status))
        .route("/admin/hooks/reload", post(reload_hooks))
        .route("/admin/hooks/{name}/enable", post(enable_hook))
        .route("/admin/hooks/{name}/disable", post(disable_hook))
        .with_state(manager)
        .layer(axum::middleware::from_fn(move |req, next| {
            let auth = Arc::clone(&admin_auth);
            async move { auth.middleware(req, next).await }
        }))
}
