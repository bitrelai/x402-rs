use axum::{
    extract::Request,
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::sync::Arc;
use subtle::ConstantTimeEq;

#[derive(Clone)]
pub struct ApiKeyAuth {
    api_keys: Arc<Vec<String>>,
    log_events: bool,
}

impl ApiKeyAuth {
    pub fn from_env() -> Self {
        let api_keys = std::env::var("API_KEYS")
            .ok()
            .map(|keys| {
                keys.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        let enabled = !api_keys.is_empty();
        if enabled {
            tracing::info!(count = api_keys.len(), "API key authentication enabled");
        } else {
            tracing::info!("API key authentication disabled (no API_KEYS configured)");
        }

        Self {
            api_keys: Arc::new(api_keys),
            log_events: true,
        }
    }

    pub fn is_enabled(&self) -> bool {
        !self.api_keys.is_empty()
    }

    pub async fn middleware(&self, req: Request, next: Next) -> Response {
        if !self.is_enabled() {
            return next.run(req).await;
        }

        match self.validate_request_auth(req.headers()) {
            Ok(()) => next.run(req).await,
            Err(error) => {
                if self.log_events {
                    tracing::warn!("Authentication failed: {}", error);
                }
                (StatusCode::UNAUTHORIZED, error).into_response()
            }
        }
    }

    pub fn validate_request_auth(&self, headers: &HeaderMap) -> Result<(), String> {
        let auth_header = headers
            .get("authorization")
            .ok_or("Missing Authorization header")?
            .to_str()
            .map_err(|_| "Invalid Authorization header")?;

        let token = auth_header
            .strip_prefix("Bearer ")
            .ok_or("Invalid Authorization header format (expected 'Bearer <token>')")?;

        let token_bytes = token.as_bytes();
        if self
            .api_keys
            .iter()
            .any(|k| k.len() == token.len() && k.as_bytes().ct_eq(token_bytes).into())
        {
            Ok(())
        } else {
            Err("Invalid API key".to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn test_valid_api_key() {
        let api_keys = vec!["test-key-123".to_string()];

        let auth = ApiKeyAuth {
            api_keys: Arc::new(api_keys),
            log_events: false,
        };

        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            HeaderValue::from_static("Bearer test-key-123"),
        );

        assert!(auth.validate_request_auth(&headers).is_ok());
    }

    #[test]
    fn test_invalid_api_key() {
        let api_keys = vec!["test-key-123".to_string()];

        let auth = ApiKeyAuth {
            api_keys: Arc::new(api_keys),
            log_events: false,
        };

        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            HeaderValue::from_static("Bearer wrong-key"),
        );

        assert!(auth.validate_request_auth(&headers).is_err());
    }

    #[test]
    fn test_missing_bearer_prefix() {
        let api_keys = vec!["test-key-123".to_string()];

        let auth = ApiKeyAuth {
            api_keys: Arc::new(api_keys),
            log_events: false,
        };

        let mut headers = HeaderMap::new();
        headers.insert("authorization", HeaderValue::from_static("test-key-123"));

        assert!(auth.validate_request_auth(&headers).is_err());
    }
}
