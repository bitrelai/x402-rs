use axum::{
    extract::Request,
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};
use subtle::ConstantTimeEq;

/// Constant-time string comparison to prevent timing side-channel attacks.
fn ct_eq(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.as_bytes().ct_eq(b.as_bytes()).into()
}

#[derive(Clone)]
pub struct AdminAuth {
    admin_key: Option<String>,
}

impl std::fmt::Debug for AdminAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdminAuth")
            .field("admin_key", &self.admin_key.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
}

impl AdminAuth {
    pub fn from_env() -> Self {
        let admin_key = std::env::var("ADMIN_API_KEY").ok();

        if admin_key.is_some() {
            tracing::info!("Admin API key authentication enabled");
        } else {
            tracing::info!("Admin API key not configured - admin endpoints disabled");
        }

        Self { admin_key }
    }

    pub async fn middleware(&self, req: Request, next: Next) -> Response {
        let Some(ref configured_key) = self.admin_key else {
            tracing::warn!("Admin endpoint accessed but ADMIN_API_KEY not configured");
            return (
                StatusCode::UNAUTHORIZED,
                "Admin access disabled - ADMIN_API_KEY not configured",
            )
                .into_response();
        };

        let provided_key = req
            .headers()
            .get("X-Admin-Key")
            .and_then(|v| v.to_str().ok());

        match provided_key {
            Some(key) if ct_eq(key, configured_key) => next.run(req).await,
            Some(_) => {
                tracing::warn!("Admin endpoint accessed with invalid key");
                (StatusCode::UNAUTHORIZED, "Invalid admin key").into_response()
            }
            None => {
                tracing::warn!("Admin endpoint accessed without X-Admin-Key header");
                (StatusCode::UNAUTHORIZED, "X-Admin-Key header required").into_response()
            }
        }
    }
}
