use axum::http::Method;
use axum::middleware;
use axum::routing::get;
use axum::Router;
use dotenvy::dotenv;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tower_http::cors;
use x402_facilitator_local::util::SigDown;
use x402_facilitator_local::{handlers, FacilitatorLocal};
use x402_types::chain::{ChainRegistry, FromConfig};
use x402_types::scheme::{SchemeBlueprints, SchemeRegistry};

#[cfg(feature = "chain-aptos")]
use x402_chain_aptos::V2AptosExact;
#[cfg(feature = "chain-eip155")]
use x402_chain_eip155::{V1Eip155Exact, V2Eip155Exact, V2Eip155Upto};
#[cfg(feature = "chain-solana")]
use x402_chain_solana::{V1SolanaExact, V2SolanaExact};

use crate::batch::queue::BatchQueueManager;
use crate::batch::BatchFacilitator;
use crate::config::Config;
use crate::enterprise_config::EnterpriseConfig;
use crate::hooks::admin::admin_hook_routes;
use crate::hooks::HookManager;
use crate::security::abuse::{AbuseDetector, AbuseDetectorConfig};
use crate::security::ip_filter::{IpFilter, IpFilterConfig};
use crate::security::rate_limit::{RateLimiter, RateLimiterConfig};
use crate::security::{AdminAuth, ApiKeyAuth};
use crate::tokens::TokenManager;

pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    rustls::crypto::CryptoProvider::install_default(rustls::crypto::ring::default_provider())
        .expect("Failed to initialize rustls crypto provider");

    dotenv().ok();

    #[cfg(feature = "telemetry")]
    let telemetry_providers = x402_facilitator_local::util::Telemetry::new()
        .with_name(env!("CARGO_PKG_NAME"))
        .with_version(env!("CARGO_PKG_VERSION"))
        .register();
    #[cfg(feature = "telemetry")]
    let telemetry_layer = telemetry_providers.http_tracing();

    #[cfg(not(feature = "telemetry"))]
    tracing_subscriber::fmt::init();

    // Load upstream chain/scheme config (JSON)
    let config = Config::load()?;

    // Load enterprise config (TOML)
    let enterprise_config = EnterpriseConfig::from_env().unwrap_or_default();

    // Build chain registry
    let chain_registry = ChainRegistry::from_config(config.chains()).await?;

    // Extract EVM provider Arcs BEFORE SchemeRegistry consumes the registry.
    // These are the same Arc<Eip155ChainProvider> instances used by scheme handlers,
    // ensuring shared nonce/signer/RPC state.
    #[cfg(feature = "chain-eip155")]
    let evm_providers = extract_evm_providers(&chain_registry);

    // Build scheme registry (consumes chain_registry)
    let scheme_blueprints = {
        #[allow(unused_mut)]
        let mut scheme_blueprints = SchemeBlueprints::new();
        #[cfg(feature = "chain-eip155")]
        {
            scheme_blueprints.register(V1Eip155Exact);
            scheme_blueprints.register(V2Eip155Exact);
            scheme_blueprints.register(V2Eip155Upto);
        }
        #[cfg(feature = "chain-solana")]
        {
            scheme_blueprints.register(V1SolanaExact);
            scheme_blueprints.register(V2SolanaExact);
        }
        #[cfg(feature = "chain-aptos")]
        {
            scheme_blueprints.register(V2AptosExact);
        }
        scheme_blueprints
    };
    let scheme_registry =
        SchemeRegistry::build(chain_registry, scheme_blueprints, config.schemes());

    let facilitator = Arc::new(FacilitatorLocal::new(scheme_registry));

    // Initialize token manager (optional — loads from TOKENS_FILE or tokens.toml)
    let token_manager = {
        let tokens_path =
            std::env::var("TOKENS_FILE").unwrap_or_else(|_| "tokens.toml".to_string());
        match TokenManager::new(&tokens_path) {
            Ok(tm) => {
                tracing::info!(path = %tokens_path, "Token manager initialized");
                Some(Arc::new(tm))
            }
            Err(e) => {
                tracing::info!(
                    path = %tokens_path,
                    error = %e,
                    "Token manager not initialized (file not found or invalid — continuing without token filtering)"
                );
                None
            }
        }
    };

    // Initialize hook manager (optional — loads from HOOKS_FILE or hooks.toml)
    let hook_manager = {
        let hooks_path =
            std::env::var("HOOKS_FILE").unwrap_or_else(|_| "hooks.toml".to_string());
        if let Some(ref tm) = token_manager {
            match HookManager::new_with_tokens(&hooks_path, tm.as_ref().clone()) {
                Ok(hm) => {
                    tracing::info!(path = %hooks_path, "Hook manager initialized with token filtering");
                    Some(Arc::new(hm))
                }
                Err(e) => {
                    tracing::info!(
                        path = %hooks_path,
                        error = %e,
                        "Hook manager not initialized (file not found or invalid — continuing without hooks)"
                    );
                    None
                }
            }
        } else {
            match HookManager::new(&hooks_path) {
                Ok(hm) => {
                    tracing::info!(path = %hooks_path, "Hook manager initialized (no token filtering)");
                    Some(Arc::new(hm))
                }
                Err(e) => {
                    tracing::info!(
                        path = %hooks_path,
                        error = %e,
                        "Hook manager not initialized (file not found or invalid — continuing without hooks)"
                    );
                    None
                }
            }
        }
    };

    // Build batch queue manager if batch settlement is enabled
    let batch_queue = if enterprise_config.batch_settlement.is_enabled_anywhere() {
        tracing::info!("Batch settlement enabled");
        Some(Arc::new(BatchQueueManager::new(
            enterprise_config.batch_settlement.clone(),
            hook_manager.clone(),
        )))
    } else {
        None
    };

    // Build the enterprise facilitator (wraps upstream with batch support)
    let batch_facilitator = BatchFacilitator {
        inner: facilitator,
        batch_queue,
        #[cfg(feature = "chain-eip155")]
        evm_providers,
    };
    let axum_state = Arc::new(batch_facilitator);

    // Initialize security middleware
    let ip_filter = IpFilter::new(IpFilterConfig {
        allowed_ips: enterprise_config.ip_filtering.allowed_ips.clone(),
        blocked_ips: enterprise_config.ip_filtering.blocked_ips.clone(),
        log_events: enterprise_config.security.log_security_events,
    });

    let rate_limiter = Arc::new(RateLimiter::new(RateLimiterConfig {
        enabled: enterprise_config.rate_limiting.enabled,
        requests_per_second: enterprise_config.rate_limiting.requests_per_second,
        ban_duration: Duration::from_secs(enterprise_config.rate_limiting.ban_duration_seconds),
        ban_threshold: enterprise_config.rate_limiting.ban_threshold,
        whitelisted_ips: enterprise_config.rate_limiting.whitelisted_ips.clone(),
    }));

    let abuse_detector = Arc::new(AbuseDetector::new(AbuseDetectorConfig {
        enabled: true,
        invalid_signature_threshold: 10,
        tracking_window: Duration::from_secs(300),
        log_events: enterprise_config.security.log_security_events,
    }));

    let api_key_auth = Arc::new(ApiKeyAuth::from_env());
    let admin_auth = Arc::new(AdminAuth::from_env());

    // Build protocol routes with BatchFacilitator as the Axum state
    let protocol_routes = handlers::routes::<Arc<BatchFacilitator>>()
        .with_state(axum_state.clone());

    // Admin stats route
    let ad_for_stats = abuse_detector.clone();
    let batch_for_stats = axum_state.batch_queue.clone();
    let admin_stats_routes = {
        let admin_auth_clone = admin_auth.clone();
        Router::new()
            .route(
                "/admin/stats",
                get(move || async move {
                    let abuse_stats = ad_for_stats.get_stats();
                    let batch_queues = batch_for_stats
                        .as_ref()
                        .map(|bq| bq.active_queues())
                        .unwrap_or(0);
                    axum::Json(serde_json::json!({
                        "abuse": {
                            "total_ips_tracked": abuse_stats.total_ips_tracked,
                            "suspicious_ips": abuse_stats.suspicious_ips,
                        },
                        "batch": {
                            "enabled": batch_for_stats.is_some(),
                            "active_queues": batch_queues,
                        }
                    }))
                }),
            )
            .layer(middleware::from_fn(move |req, next| {
                let auth = admin_auth_clone.clone();
                async move { auth.middleware(req, next).await }
            }))
    };

    // Hook admin routes (optional — only if hook manager is initialized)
    let hook_admin_routes = hook_manager.as_ref().map(|hm| {
        admin_hook_routes(Arc::clone(hm), admin_auth.as_ref().clone())
    });

    // Token admin reload route (optional — only if token manager is initialized)
    let token_admin_routes = token_manager.as_ref().map(|tm| {
        let tm_clone = Arc::clone(tm);
        let admin_auth_clone = admin_auth.clone();
        Router::new()
            .route(
                "/admin/tokens/reload",
                axum::routing::post(move || {
                    let tm = tm_clone.clone();
                    async move {
                        match tm.reload().await {
                            Ok(()) => axum::Json(serde_json::json!({
                                "success": true,
                                "message": "Token configuration reloaded"
                            })),
                            Err(e) => axum::Json(serde_json::json!({
                                "success": false,
                                "error": e
                            })),
                        }
                    }
                }),
            )
            .layer(middleware::from_fn(move |req, next| {
                let auth = admin_auth_clone.clone();
                async move { auth.middleware(req, next).await }
            }))
    });

    // Compose all routes (upstream handlers::routes() already registers GET /)
    let mut app = Router::new()
        .merge(protocol_routes)
        .merge(admin_stats_routes);

    if let Some(hook_routes) = hook_admin_routes {
        app = app.merge(hook_routes);
    }
    if let Some(token_routes) = token_admin_routes {
        app = app.merge(token_routes);
    }

    // Apply middleware layers (outermost first)
    let abuse_for_mw = abuse_detector.clone();
    let api_key_for_mw = api_key_auth.clone();
    let rate_limiter_for_mw = rate_limiter.clone();
    let ip_filter_for_mw = ip_filter.clone();

    #[cfg(feature = "telemetry")]
    let app = app.layer(telemetry_layer);

    let app = app
        .layer(middleware::from_fn(move |req, next| {
            let detector = abuse_for_mw.clone();
            async move { detector.middleware(req, next).await }
        }))
        .layer(middleware::from_fn(move |req, next| {
            let auth = api_key_for_mw.clone();
            async move { auth.middleware(req, next).await }
        }))
        .layer(middleware::from_fn(move |req, next| {
            let limiter = rate_limiter_for_mw.clone();
            async move { limiter.middleware(req, next).await }
        }))
        .layer(middleware::from_fn(move |req, next| {
            let filter = ip_filter_for_mw.clone();
            async move { filter.middleware(req, next).await }
        }))
        .layer(
            cors::CorsLayer::new()
                .allow_origin(cors::Any)
                .allow_methods([Method::GET, Method::POST])
                .allow_headers(cors::Any),
        );

    // Spawn background cleanup task
    let rl_cleanup = rate_limiter.clone();
    let ad_cleanup = abuse_detector.clone();
    let cleanup_interval = enterprise_config.security.cleanup_interval_seconds;
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(cleanup_interval));
        loop {
            interval.tick().await;
            rl_cleanup.cleanup_expired_bans();
            ad_cleanup.cleanup_old_data();
        }
    });

    let addr = SocketAddr::new(config.host(), config.port());
    #[cfg(feature = "telemetry")]
    tracing::info!("Starting enterprise facilitator at http://{}", addr);
    #[cfg(not(feature = "telemetry"))]
    println!("Starting enterprise facilitator at http://{}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    #[cfg(feature = "telemetry")]
    let listener = listener;  // no-op, just for symmetry with upstream's inspect_err pattern

    let sig_down = SigDown::try_new()?;
    let axum_cancellation_token = sig_down.cancellation_token();
    let axum_graceful_shutdown = async move { axum_cancellation_token.cancelled().await };
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(axum_graceful_shutdown)
    .await?;

    Ok(())
}

/// Extract EVM provider Arcs from the chain registry before it's consumed.
/// These are the same Arc instances that will be used by scheme handlers.
#[cfg(feature = "chain-eip155")]
fn extract_evm_providers(
    registry: &ChainRegistry<crate::chain::ChainProvider>,
) -> HashMap<x402_types::chain::ChainId, Arc<x402_chain_eip155::chain::Eip155ChainProvider>> {
    use crate::chain::ChainProvider;
    use x402_types::chain::ChainIdPattern;

    let mut providers = HashMap::new();
    let evm_pattern = ChainIdPattern::wildcard("eip155");
    for chain_provider in registry.by_chain_id_pattern(&evm_pattern) {
        if let ChainProvider::Eip155(provider) = chain_provider {
            let chain_id = x402_types::chain::ChainProviderOps::chain_id(chain_provider);
            providers.insert(chain_id, Arc::clone(provider));
        }
    }
    providers
}
