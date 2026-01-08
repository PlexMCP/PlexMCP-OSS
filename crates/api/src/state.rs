//! Application state

use maxminddb::Reader;
use reqwest::Client;
use sqlx::PgPool;
use std::sync::Arc;

use crate::{
    alerting::AlertService,
    auth::{ApiKeyManager, AuthState, InFlightRequests, JwtManager, TokenCache},
    config::Config,
    email::SecurityEmailService,
    flyio::FlyClient,
    routing::HostResolver,
    websocket::WebSocketState,
};

// Import rate limiter from shared crate (available without billing feature)
use plexmcp_shared::RateLimiter;

/// Shared application state
#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub config: Config,
    pub jwt_manager: JwtManager,
    pub api_key_manager: ApiKeyManager,
    /// Billing service (only available when billing feature is enabled)
    #[cfg(feature = "billing")]
    pub billing: Option<Arc<plexmcp_billing::BillingService>>,
    pub http_client: Client,
    pub security_email: SecurityEmailService,
    pub host_resolver: HostResolver,
    pub fly_client: Option<FlyClient>,
    /// WebSocket state for real-time features
    pub ws_state: WebSocketState,
    /// GeoIP database reader for IP geolocation (thread-safe, shared across all requests)
    pub geoip_reader: Option<Arc<Reader<Vec<u8>>>>,
    /// Rate limiter for API key request throttling (from shared crate, available without billing)
    pub rate_limiter: RateLimiter,
    /// Shared MCP client for HTTP session caching across requests
    pub mcp_client: Arc<crate::mcp::client::McpClient>,
    /// Security alerting service for real-time threat detection
    pub alert_service: AlertService,
    /// Cache for Supabase token verification results to prevent rate limiting
    pub(crate) token_cache: TokenCache,
    /// Track in-flight Supabase verification requests for request coalescing
    pub(crate) in_flight_requests: InFlightRequests,
}

/// Load MaxMind GeoLite2-City database from disk
/// Returns None if file doesn't exist or fails to load
fn load_geoip_database() -> Option<Arc<Reader<Vec<u8>>>> {
    use std::path::Path;

    let db_path = "data/GeoLite2-City.mmdb";

    if !Path::new(db_path).exists() {
        tracing::warn!(
            path = db_path,
            "GeoIP database file not found - download from https://dev.maxmind.com/geoip/geolite2-free-geolocation-data"
        );
        return None;
    }

    match maxminddb::Reader::open_readfile(db_path) {
        Ok(reader) => {
            tracing::info!(
                database_type = reader.metadata.database_type,
                "GeoIP database loaded successfully"
            );
            Some(Arc::new(reader))
        }
        Err(e) => {
            tracing::error!(path = db_path, error = ?e, "Failed to load GeoIP database");
            None
        }
    }
}

impl AppState {
    pub fn new(pool: PgPool, config: Config) -> Self {
        // Use Supabase JWT secret if available, otherwise fallback to basic JWT manager
        let jwt_manager = if !config.supabase_jwt_secret.is_empty() {
            tracing::info!("Supabase JWT validation enabled");
            JwtManager::with_supabase_secret(
                &config.jwt_secret,
                &config.supabase_jwt_secret,
                config.jwt_expiry_hours,
            )
        } else {
            tracing::warn!("Supabase JWT validation not configured (missing SUPABASE_JWT_SECRET)");
            JwtManager::new(&config.jwt_secret, config.jwt_expiry_hours)
        };
        let api_key_manager = ApiKeyManager::new(&config.api_key_hmac_secret);

        // Try to initialize billing if Stripe env vars are set (only when feature is enabled)
        #[cfg(feature = "billing")]
        let billing = if config.enable_billing {
            match plexmcp_billing::BillingService::from_env(pool.clone()) {
                Ok(svc) => {
                    tracing::info!("Stripe billing service initialized");
                    Some(Arc::new(svc))
                }
                Err(e) => {
                    tracing::warn!("Stripe billing not configured: {}", e);
                    None
                }
            }
        } else {
            tracing::info!("Billing disabled via config (ENABLE_BILLING=false)");
            None
        };

        #[cfg(not(feature = "billing"))]
        tracing::info!("Billing feature not compiled in (build without --features billing)");

        // Create HTTP client for external API calls (e.g., Supabase token verification)
        let http_client = Client::new();

        if !config.supabase_url.is_empty() {
            if config.supabase_anon_key.is_empty() {
                tracing::warn!("Supabase URL configured but SUPABASE_ANON_KEY is missing - API token verification will fail");
            } else {
                tracing::info!(
                    "Supabase API verification enabled via {}",
                    config.supabase_url
                );
            }
        }

        // Initialize security email service
        let security_email = SecurityEmailService::from_env();
        if security_email.is_enabled() {
            tracing::info!("Security email notifications enabled");
        } else {
            tracing::warn!("Security email notifications not configured (missing RESEND_API_KEY)");
        }

        // Initialize host resolver for subdomain/custom domain routing
        let host_resolver = HostResolver::new(pool.clone(), config.base_domain.clone());
        tracing::info!(
            "Host resolver initialized for base domain: {}",
            config.base_domain
        );

        // Initialize Fly.io client for SSL provisioning
        let fly_client =
            FlyClient::from_config(config.fly_api_token.clone(), config.fly_app_name.clone());
        if fly_client.is_some() {
            tracing::info!("Fly.io SSL provisioning enabled");
        } else {
            tracing::warn!(
                "Fly.io SSL provisioning not configured (missing FLY_API_TOKEN or FLY_APP_NAME)"
            );
        }

        // Initialize WebSocket state
        let ws_state = WebSocketState::new();
        tracing::info!("WebSocket state initialized for real-time support features");

        // Initialize GeoIP database reader
        let geoip_reader = load_geoip_database();
        if geoip_reader.is_none() {
            tracing::warn!("Location tracking disabled (GeoIP database not available)");
        }

        // Initialize rate limiter from shared crate (always available, no billing dependency)
        let rate_limiter = RateLimiter::new_in_memory();
        tracing::info!("Rate limiter initialized");

        // Initialize shared MCP client for HTTP session caching
        let mcp_client = Arc::new(crate::mcp::client::McpClient::new());
        tracing::info!("Shared MCP client initialized");

        // Start session cleanup task (runs every 5 minutes)
        let client_for_cleanup = mcp_client.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(300));
            loop {
                interval.tick().await;
                client_for_cleanup.cleanup_stale_sessions().await;
            }
        });

        // Initialize security alerting service
        let slack_webhook_url = std::env::var("SLACK_SECURITY_WEBHOOK_URL").ok();
        let alert_service = AlertService::new(pool.clone(), slack_webhook_url.clone());
        if slack_webhook_url.is_some() {
            tracing::info!("Security alerting service initialized with Slack notifications");
        } else {
            tracing::warn!("Security alerting service initialized without Slack (missing SLACK_SECURITY_WEBHOOK_URL)");
        }

        // Initialize token cache for Supabase verification (prevents rate limiting)
        let token_cache = Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new()));
        tracing::info!("Supabase token cache initialized");

        // Initialize in-flight requests map for request coalescing
        let in_flight_requests =
            Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
        tracing::info!("Request coalescing initialized for Supabase verification");

        Self {
            pool,
            config,
            jwt_manager,
            api_key_manager,
            #[cfg(feature = "billing")]
            billing,
            http_client,
            security_email,
            host_resolver,
            fly_client,
            ws_state,
            geoip_reader,
            rate_limiter,
            mcp_client,
            alert_service,
            token_cache,
            in_flight_requests,
        }
    }

    /// Get auth state for middleware
    pub fn auth_state(&self) -> AuthState {
        AuthState {
            jwt_manager: self.jwt_manager.clone(),
            api_key_manager: self.api_key_manager.clone(),
            pool: self.pool.clone(),
            supabase_url: self.config.supabase_url.clone(),
            supabase_anon_key: self.config.supabase_anon_key.clone(),
            http_client: self.http_client.clone(),
            token_cache: self.token_cache.clone(),
            in_flight_requests: self.in_flight_requests.clone(),
        }
    }

    /// Get billing service reference (returns None when billing feature is disabled)
    #[cfg(feature = "billing")]
    pub fn billing_service(&self) -> Option<&Arc<plexmcp_billing::BillingService>> {
        self.billing.as_ref()
    }

    /// Get billing service reference (always returns None when billing feature is disabled)
    #[cfg(not(feature = "billing"))]
    pub fn billing_service(&self) -> Option<&std::sync::Arc<()>> {
        None
    }
}
