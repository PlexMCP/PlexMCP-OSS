//! Authentication middleware for Axum

use axum::{
    extract::{Request, State},
    http::{
        header::{AUTHORIZATION, COOKIE},
        Method, StatusCode,
    },
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;
use sqlx::{FromRow, PgPool};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, Mutex, RwLock};
use uuid::Uuid;

/// Token cache TTL - cache Supabase verification results for 60 seconds
/// This prevents hitting Supabase rate limits when dashboard makes parallel requests
const TOKEN_CACHE_TTL: Duration = Duration::from_secs(60);

/// SOC 2 CC6.1: Maximum cache entries to prevent memory exhaustion attacks
/// If an attacker sends many unique tokens, we evict oldest entries to stay bounded
const MAX_CACHE_ENTRIES: usize = 10_000;

/// In-flight verification result - just the user ID and email (public-safe)
#[derive(Clone, Debug)]
pub(crate) struct InFlightVerificationResult {
    pub user_id: String,
    pub email: Option<String>,
}

/// In-flight request result type for request coalescing
type InFlightResult = Result<InFlightVerificationResult, ()>;

/// Type for tracking in-flight token verification requests
/// This prevents multiple parallel requests from all hitting Supabase simultaneously
pub(crate) type InFlightRequests = Arc<Mutex<HashMap<String, broadcast::Sender<InFlightResult>>>>;

/// Cached authentication result from Supabase
#[derive(Clone, Debug)]
pub(crate) struct CachedSupabaseAuth {
    user: SupabaseUserResponse,
    cached_at: Instant,
}

/// Thread-safe token cache type (crate-internal, not part of public API)
pub(crate) type TokenCache = Arc<RwLock<HashMap<String, CachedSupabaseAuth>>>;

use super::{api_key::ApiKeyManager, jwt::JwtManager, password, sessions};

/// Database row type for API key lookup
#[derive(Debug, FromRow)]
#[allow(dead_code)] // Fields read from DB via FromRow
struct ApiKeyRow {
    id: Uuid,
    org_id: Uuid,
    scopes: Option<serde_json::Value>,
    subscription_tier: String,
}

/// Database row type for organization membership lookup
#[derive(Debug, FromRow)]
struct OrgMembershipRow {
    org_id: Uuid,
    role: String,
}

/// Authenticated user information extracted from JWT or API key
#[derive(Debug, Clone)]
pub struct AuthUser {
    pub user_id: Option<Uuid>,
    pub org_id: Option<Uuid>,
    pub role: String,
    pub email: Option<String>,
    pub auth_method: AuthMethod,
    /// Session ID for linking JWT sessions to audit logs (SOC 2 compliance)
    /// None for API key authentication
    pub session_id: Option<Uuid>,
}

impl AuthUser {
    /// Get org_id, returning an error if not available
    pub fn require_org_id(&self) -> Result<Uuid, AuthError> {
        self.org_id.ok_or(AuthError::NoOrganization)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum AuthMethod {
    Jwt,
    SupabaseJwt,
    ApiKey { key_id: Uuid },
}

/// Response from Supabase /auth/v1/user endpoint
#[derive(Debug, Clone, Deserialize)]
struct SupabaseUserResponse {
    id: String,
    email: Option<String>,
}

/// State needed for authentication
#[derive(Clone)]
pub struct AuthState {
    pub jwt_manager: JwtManager,
    pub api_key_manager: ApiKeyManager,
    pub pool: PgPool,
    pub supabase_url: String,
    pub supabase_anon_key: String,
    pub http_client: Client,
    /// Cache for Supabase token verification results to prevent rate limiting
    pub(crate) token_cache: TokenCache,
    /// Track in-flight Supabase verification requests for request coalescing
    /// This prevents multiple parallel requests from all hitting Supabase simultaneously
    pub(crate) in_flight_requests: InFlightRequests,
}

/// SOC 2 CC6.1: Extract bearer token from HttpOnly cookies
/// This enables secure cookie-based authentication that protects against XSS attacks.
/// The cookie name matches what the frontend set-cookie API route uses.
fn extract_token_from_cookie(request: &Request) -> Option<String> {
    request
        .headers()
        .get(COOKIE)
        .and_then(|h| h.to_str().ok())
        .and_then(|cookies| {
            for cookie in cookies.split(';') {
                let cookie = cookie.trim();
                // Try plexmcp_auth_token (set by Next.js API route)
                if let Some(token) = cookie.strip_prefix("plexmcp_auth_token=") {
                    return Some(token.to_string());
                }
            }
            None
        })
}

/// Extract bearer token from Authorization header or HttpOnly cookie
/// Prefers Authorization header but falls back to cookie for SPA clients using HttpOnly cookies
fn extract_bearer_token(request: &Request) -> Option<String> {
    // Try Authorization header first
    if let Some(header) = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
    {
        if let Some(token) = header.strip_prefix("Bearer ") {
            return Some(token.to_string());
        }
    }

    // Fall back to HttpOnly cookie
    extract_token_from_cookie(request)
}

/// Extract API key from Authorization header (no cookie fallback for API keys)
fn extract_api_key(request: &Request) -> Option<String> {
    request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|header| header.strip_prefix("ApiKey "))
        .map(String::from)
}

/// Extract IP address from request headers (X-Forwarded-For, X-Real-IP, or CF-Connecting-IP)
fn extract_ip_address(request: &Request) -> Option<String> {
    // Try X-Forwarded-For first (may contain multiple IPs, take first)
    if let Some(xff) = request.headers().get("X-Forwarded-For") {
        if let Ok(xff_str) = xff.to_str() {
            return xff_str.split(',').next().map(|s| s.trim().to_string());
        }
    }
    // Try Cloudflare's header
    if let Some(cf_ip) = request.headers().get("CF-Connecting-IP") {
        if let Ok(ip) = cf_ip.to_str() {
            return Some(ip.to_string());
        }
    }
    // Try X-Real-IP
    if let Some(real_ip) = request.headers().get("X-Real-IP") {
        if let Ok(ip) = real_ip.to_str() {
            return Some(ip.to_string());
        }
    }
    None
}

/// Middleware that requires authentication
pub async fn require_auth(
    State(auth_state): State<AuthState>,
    mut request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path().to_string();

    // Extract IP and User-Agent for audit logging
    let ip_address = extract_ip_address(&request);
    let user_agent = request
        .headers()
        .get("User-Agent")
        .and_then(|h| h.to_str().ok())
        .map(String::from);

    // SOC 2 CC6.1: Try Bearer token from header or HttpOnly cookie
    let has_token = extract_bearer_token(&request).is_some();
    let has_api_key = extract_api_key(&request).is_some();
    tracing::info!(path = %path, has_token = %has_token, has_api_key = %has_api_key, "require_auth: checking authentication");

    let auth_result = if let Some(token) = extract_bearer_token(&request) {
        authenticate_jwt(&auth_state, &token).await
    } else if let Some(key) = extract_api_key(&request) {
        authenticate_api_key(&auth_state, &key, ip_address, user_agent).await
    } else {
        tracing::warn!(path = %path, "require_auth: no valid auth found (header or cookie)");
        Err(AuthError::MissingAuth)
    };

    match auth_result {
        Ok(auth_user) => {
            tracing::info!(
                path = %path,
                user_id = ?auth_user.user_id,
                org_id = ?auth_user.org_id,
                role = %auth_user.role,
                auth_method = ?auth_user.auth_method,
                "require_auth: authentication successful"
            );
            request.extensions_mut().insert(auth_user);
            next.run(request).await
        }
        Err(err) => {
            tracing::warn!(path = %path, error = ?err, "require_auth: authentication failed");
            err.into_response()
        }
    }
}

/// Middleware that optionally authenticates (for public endpoints that benefit from auth)
pub async fn optional_auth(
    State(auth_state): State<AuthState>,
    mut request: Request,
    next: Next,
) -> Response {
    // Extract IP and User-Agent for audit logging
    let ip_address = extract_ip_address(&request);
    let user_agent = request
        .headers()
        .get("User-Agent")
        .and_then(|h| h.to_str().ok())
        .map(String::from);

    // SOC 2 CC6.1: Try Bearer token from header or HttpOnly cookie
    if let Some(token) = extract_bearer_token(&request) {
        if let Ok(auth_user) = authenticate_jwt(&auth_state, &token).await {
            request.extensions_mut().insert(auth_user);
            return next.run(request).await;
        }
    }

    // Try API key (header only, no cookie fallback for API keys)
    if let Some(key) = extract_api_key(&request) {
        if let Ok(auth_user) = authenticate_api_key(&auth_state, &key, ip_address, user_agent).await
        {
            request.extensions_mut().insert(auth_user);
        }
    }

    next.run(request).await
}

/// Middleware that requires a specific role
pub async fn require_role(
    required_roles: &[&str],
    State(auth_state): State<AuthState>,
    request: Request,
    next: Next,
) -> Response {
    // Extract IP and User-Agent for audit logging
    let ip_address = extract_ip_address(&request);
    let user_agent = request
        .headers()
        .get("User-Agent")
        .and_then(|h| h.to_str().ok())
        .map(String::from);

    // SOC 2 CC6.1: Try Bearer token from header or HttpOnly cookie
    let auth_result = if let Some(token) = extract_bearer_token(&request) {
        authenticate_jwt(&auth_state, &token).await
    } else if let Some(key) = extract_api_key(&request) {
        authenticate_api_key(&auth_state, &key, ip_address, user_agent).await
    } else {
        return AuthError::MissingAuth.into_response();
    };

    match auth_result {
        Ok(auth_user) => {
            // Check role
            if !required_roles.contains(&auth_user.role.as_str()) {
                return AuthError::InsufficientPermissions.into_response();
            }

            let mut request = request;
            request.extensions_mut().insert(auth_user);
            next.run(request).await
        }
        Err(err) => err.into_response(),
    }
}

async fn authenticate_jwt(auth_state: &AuthState, token: &str) -> Result<AuthUser, AuthError> {
    // Log the start of JWT validation for debugging
    let token_prefix = if token.len() > 20 {
        &token[..20]
    } else {
        token
    };
    tracing::info!(token_prefix = %token_prefix, "authenticate_jwt: starting validation");

    // First try to validate as a PlexMCP-issued token
    if let Ok(claims) = auth_state.jwt_manager.validate_access_token(token) {
        // SOC 2 CC6.1: Validate session ownership along with revocation status
        let session_valid = sessions::is_session_valid(&auth_state.pool, &claims.jti, claims.sub)
            .await
            .map_err(|_| AuthError::DatabaseError)?;

        if !session_valid {
            tracing::warn!(jti = %claims.jti, user_id = %claims.sub, "Session revoked or expired");
            return Err(AuthError::InvalidToken);
        }

        // Get session ID for audit logging
        let session_id: Option<Uuid> =
            sqlx::query_scalar("SELECT id FROM user_sessions WHERE jti = $1")
                .bind(&claims.jti)
                .fetch_optional(&auth_state.pool)
                .await
                .ok()
                .flatten();

        // Verify the user still exists in the database (handles stale tokens after user ID changes)
        let user_exists: Option<(bool,)> = sqlx::query_as("SELECT TRUE FROM users WHERE id = $1")
            .bind(claims.sub)
            .fetch_optional(&auth_state.pool)
            .await
            .ok()
            .flatten();

        if user_exists.is_some() {
            // User exists with the ID in the token - use it
            return Ok(AuthUser {
                user_id: Some(claims.sub),
                org_id: Some(claims.org_id),
                role: claims.role,
                email: Some(claims.email),
                auth_method: AuthMethod::Jwt,
                session_id,
            });
        }

        // User ID in token doesn't exist - try to find by email (handles stale tokens)
        tracing::warn!(
            token_user_id = %claims.sub,
            email = %claims.email,
            "JWT user_id not found in database, attempting email fallback"
        );

        let existing_user: Option<ExistingUserWithOrgRow> =
            sqlx::query_as("SELECT id, org_id FROM users WHERE email = $1")
                .bind(&claims.email)
                .fetch_optional(&auth_state.pool)
                .await
                .map_err(|_| AuthError::DatabaseError)?;

        if let Some(user) = existing_user {
            // Found user by email - get their role from org_members
            let role: String = sqlx::query_scalar(
                "SELECT role FROM organization_members WHERE user_id = $1 AND org_id = $2",
            )
            .bind(user.id)
            .bind(user.org_id)
            .fetch_optional(&auth_state.pool)
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| "member".to_string());

            tracing::info!(
                old_user_id = %claims.sub,
                new_user_id = %user.id,
                email = %claims.email,
                "Resolved stale JWT to existing user via email"
            );

            return Ok(AuthUser {
                user_id: Some(user.id),
                org_id: Some(user.org_id),
                role,
                email: Some(claims.email),
                auth_method: AuthMethod::Jwt,
                session_id,
            });
        }

        // Neither user_id nor email found - token is invalid
        tracing::error!(
            token_user_id = %claims.sub,
            email = %claims.email,
            "JWT user not found by ID or email"
        );
        return Err(AuthError::InvalidToken);
    }

    // If PlexMCP JWT validation fails, try Supabase
    tracing::info!("authenticate_jwt: PlexMCP JWT validation failed, trying Supabase API");

    // If that fails and we have Supabase configured, verify via Supabase API
    if !auth_state.supabase_url.is_empty() {
        // Verify token by calling Supabase's user endpoint
        let supabase_user = match verify_supabase_token_via_api(auth_state, token).await {
            Ok(user) => user,
            Err(e) => {
                tracing::warn!("authenticate_jwt: Supabase verification failed: {:?}", e);
                return Err(e);
            }
        };

        // Parse the user ID from Supabase response
        let user_id = Uuid::parse_str(&supabase_user.id).map_err(|_| AuthError::InvalidToken)?;

        // Ensure the OAuth user exists in our users table (for foreign key constraints)
        // This handles users who authenticate via OAuth but don't have a record yet
        // Returns resolved_user_id which may differ from OAuth user_id if there's an existing user
        let (org_id, role, resolved_user_id) = ensure_oauth_user_exists(
            &auth_state.pool,
            user_id,
            supabase_user.email.as_deref(),
        )
        .await
        .map_err(|e| {
            tracing::error!(user_id = %user_id, error = %e, "Failed to ensure OAuth user exists");
            AuthError::DatabaseError
        })?;

        // Use resolved_user_id for AuthUser - this ensures FK constraints work
        // because it references a user that actually exists in the users table
        return Ok(AuthUser {
            user_id: Some(resolved_user_id),
            org_id: Some(org_id),
            role,
            email: supabase_user.email,
            auth_method: AuthMethod::SupabaseJwt,
            session_id: None, // Supabase JWTs are externally managed (no session tracking)
        });
    }

    Err(AuthError::InvalidToken)
}

/// Public function to verify Supabase auth from AppState (for WebSocket handler)
pub async fn verify_supabase_auth_from_app_state(
    app_state: &crate::state::AppState,
    token: &str,
) -> Result<AuthUser, AuthError> {
    authenticate_jwt(&app_state.auth_state(), token).await
}

/// Verify a Supabase token by calling the Supabase Auth API
/// Uses caching and request coalescing to prevent Supabase rate limiting
/// when dashboard makes parallel requests
async fn verify_supabase_token_via_api(
    auth_state: &AuthState,
    token: &str,
) -> Result<SupabaseUserResponse, AuthError> {
    // Must have anon key configured to verify via API
    if auth_state.supabase_anon_key.is_empty() {
        tracing::warn!("Supabase anon key not configured, cannot verify token via API");
        return Err(AuthError::InvalidToken);
    }

    // Check cache first to avoid hitting Supabase rate limits
    {
        let cache = auth_state.token_cache.read().await;
        if let Some(cached) = cache.get(token) {
            if cached.cached_at.elapsed() < TOKEN_CACHE_TTL {
                tracing::debug!("Using cached Supabase auth for user {}", cached.user.id);
                return Ok(cached.user.clone());
            }
        }
    }

    // Request coalescing: check if there's already an in-flight request for this token
    // If so, subscribe to it instead of making another API call
    let token_key = token.to_string();
    let mut rx_opt = None;

    {
        let mut in_flight = auth_state.in_flight_requests.lock().await;
        if let Some(tx) = in_flight.get(&token_key) {
            // Another request is already verifying this token, subscribe to it
            rx_opt = Some(tx.subscribe());
            tracing::debug!("Joining in-flight Supabase verification request");
        } else {
            // We're the first request for this token, create a broadcast channel
            let (tx, _) = broadcast::channel(1);
            in_flight.insert(token_key.clone(), tx);
            tracing::debug!("Starting new Supabase verification request");
        }
    }

    // If we found an in-flight request, wait for its result
    if let Some(mut rx) = rx_opt {
        match rx.recv().await {
            Ok(Ok(inflight_result)) => {
                // Convert InFlightVerificationResult back to SupabaseUserResponse
                return Ok(SupabaseUserResponse {
                    id: inflight_result.user_id,
                    email: inflight_result.email,
                });
            }
            Ok(Err(())) | Err(_) => return Err(AuthError::InvalidToken),
        }
    }

    // We're the leader - make the actual API call
    let result = verify_supabase_token_api_call(auth_state, token).await;

    // Broadcast result to any waiting requests and clean up
    {
        let mut in_flight = auth_state.in_flight_requests.lock().await;
        if let Some(tx) = in_flight.remove(&token_key) {
            let broadcast_result = match &result {
                Ok(user) => Ok(InFlightVerificationResult {
                    user_id: user.id.clone(),
                    email: user.email.clone(),
                }),
                Err(_) => Err(()),
            };
            // Ignore send errors - receivers may have been dropped
            let _ = tx.send(broadcast_result);
        }
    }

    // Cache successful results (with size limit to prevent memory exhaustion)
    if let Ok(ref user) = result {
        let mut cache = auth_state.token_cache.write().await;

        // SOC 2 CC6.1: Evict oldest entry if at capacity
        if cache.len() >= MAX_CACHE_ENTRIES {
            if let Some(oldest_key) = cache
                .iter()
                .min_by_key(|(_, v)| v.cached_at)
                .map(|(k, _)| k.clone())
            {
                cache.remove(&oldest_key);
                tracing::debug!("Evicted oldest cache entry to stay under limit");
            }
        }

        cache.insert(
            token.to_string(),
            CachedSupabaseAuth {
                user: user.clone(),
                cached_at: Instant::now(),
            },
        );
        tracing::debug!("Cached Supabase auth for user {}", user.id);
    }

    result
}

/// Actually make the API call to Supabase to verify a token
async fn verify_supabase_token_api_call(
    auth_state: &AuthState,
    token: &str,
) -> Result<SupabaseUserResponse, AuthError> {
    let url = format!("{}/auth/v1/user", auth_state.supabase_url);

    let response = auth_state
        .http_client
        .get(&url)
        .header("apikey", &auth_state.supabase_anon_key)
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
        .map_err(|e| {
            tracing::error!("Failed to verify Supabase token: {}", e);
            AuthError::InvalidToken
        })?;

    if !response.status().is_success() {
        tracing::warn!(
            "Supabase token verification failed with status: {}",
            response.status()
        );
        return Err(AuthError::InvalidToken);
    }

    response
        .json::<SupabaseUserResponse>()
        .await
        .map_err(|_| AuthError::InvalidToken)
}

/// Database row type for existing user lookup
#[derive(Debug, FromRow)]
struct ExistingUserRow {
    id: Uuid,
}

/// Database row type for existing user lookup with org_id
#[derive(Debug, FromRow)]
struct ExistingUserWithOrgRow {
    id: Uuid,
    org_id: Uuid,
}

/// Ensure OAuth user exists in our users table for foreign key constraints
/// Creates organization and user record if they don't exist
/// Returns (org_id, role, resolved_user_id) - resolved_user_id may be different from OAuth user_id
/// if there's already a user with the same org+email
async fn ensure_oauth_user_exists(
    pool: &PgPool,
    user_id: Uuid,
    email: Option<&str>,
) -> Result<(Uuid, String, Uuid), sqlx::Error> {
    // First check if user has an org membership - this tells us what org they belong to
    let existing_membership: Option<OrgMembershipRow> = sqlx::query_as(
        r#"
        SELECT om.org_id, om.role
        FROM organization_members om
        WHERE om.user_id = $1
        ORDER BY om.created_at ASC
        LIMIT 1
        "#,
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await?;

    if let Some(membership) = existing_membership {
        // User has org membership - ensure they exist in users table with correct org_id
        // This is needed for foreign key constraints (e.g., api_keys.created_by)
        let user_exists: Option<(bool,)> = sqlx::query_as("SELECT TRUE FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(pool)
            .await?;

        if user_exists.is_none() {
            // User doesn't exist in users table yet - try to create them
            let email_str = email.unwrap_or("oauth@placeholder.local");

            tracing::info!(
                user_id = %user_id,
                org_id = %membership.org_id,
                email = %email_str,
                "Creating user record for OAuth user with existing org membership"
            );

            // SOC 2 CC6.1: Generate cryptographically random hash for OAuth users
            let impossible_hash = password::generate_impossible_hash()
                .map_err(|e| sqlx::Error::Protocol(e.to_string()))?;

            // Try to insert the user
            let insert_result = sqlx::query(
                r#"
                INSERT INTO users (id, email, password_hash, org_id, role, created_at, updated_at)
                VALUES ($1, $2, $3, $4, 'member', NOW(), NOW())
                ON CONFLICT (id) DO NOTHING
                "#,
            )
            .bind(user_id)
            .bind(email_str)
            .bind(&impossible_hash)
            .bind(membership.org_id)
            .execute(pool)
            .await;

            match insert_result {
                Ok(_) => {
                    // User created successfully, return the OAuth user_id
                    return Ok((membership.org_id, membership.role, user_id));
                }
                Err(e) => {
                    // Check if this is an org+email conflict
                    if e.to_string().contains("users_org_id_email_key") {
                        // A user with the same org+email already exists with a different ID
                        // Look up that user's ID and use it instead
                        let existing_user: Option<ExistingUserRow> =
                            sqlx::query_as("SELECT id FROM users WHERE org_id = $1 AND email = $2")
                                .bind(membership.org_id)
                                .bind(email_str)
                                .fetch_optional(pool)
                                .await?;

                        if let Some(existing) = existing_user {
                            tracing::info!(
                                oauth_user_id = %user_id,
                                resolved_user_id = %existing.id,
                                org_id = %membership.org_id,
                                email = %email_str,
                                "OAuth user mapped to existing user with same org+email"
                            );
                            return Ok((membership.org_id, membership.role, existing.id));
                        } else {
                            // Shouldn't happen, but fall through to return OAuth user_id
                            tracing::warn!(
                                user_id = %user_id,
                                org_id = %membership.org_id,
                                email = %email_str,
                                "org+email conflict but couldn't find existing user"
                            );
                        }
                    } else {
                        return Err(e);
                    }
                }
            }
        }

        return Ok((membership.org_id, membership.role, user_id));
    }

    // No org membership - check if there's an existing user with this email
    // This handles the case where an OAuth user logs in but their record was created differently
    if let Some(email_str) = email {
        let existing_user: Option<ExistingUserWithOrgRow> =
            sqlx::query_as("SELECT id, org_id FROM users WHERE email = $1")
                .bind(email_str)
                .fetch_optional(pool)
                .await?;

        if let Some(existing) = existing_user {
            tracing::info!(
                oauth_user_id = %user_id,
                resolved_user_id = %existing.id,
                org_id = %existing.org_id,
                email = %email_str,
                "OAuth user mapped to existing user via email lookup"
            );

            // Get the user's role from their org membership, or default to "member"
            let role: String = sqlx::query_scalar(
                "SELECT role FROM organization_members WHERE user_id = $1 AND org_id = $2",
            )
            .bind(existing.id)
            .bind(existing.org_id)
            .fetch_optional(pool)
            .await?
            .unwrap_or_else(|| "member".to_string());

            return Ok((existing.org_id, role, existing.id));
        }
    }

    // No existing user - need to create everything from scratch
    tracing::info!(user_id = %user_id, email = ?email, "Creating org and user records for new OAuth user");

    // Create organization first
    let org_id = Uuid::new_v4();
    let org_name = email
        .map(|e| format!("{}'s Organization", e.split('@').next().unwrap_or("User")))
        .unwrap_or_else(|| "My Organization".to_string());

    sqlx::query(
        r#"
        INSERT INTO organizations (id, name, subscription_tier, created_at, updated_at)
        VALUES ($1, $2, 'free', NOW(), NOW())
        "#,
    )
    .bind(org_id)
    .bind(&org_name)
    .execute(pool)
    .await?;

    // SOC 2 CC6.1: Generate cryptographically random hash for OAuth users
    let impossible_hash =
        password::generate_impossible_hash().map_err(|e| sqlx::Error::Protocol(e.to_string()))?;

    // Create user record with org_id and required NOT NULL columns
    sqlx::query(
        r#"
        INSERT INTO users (id, email, password_hash, org_id, role, created_at, updated_at)
        VALUES ($1, $2, $3, $4, 'member', NOW(), NOW())
        "#,
    )
    .bind(user_id)
    .bind(email.unwrap_or("oauth@placeholder.local"))
    .bind(&impossible_hash)
    .bind(org_id)
    .execute(pool)
    .await?;

    // Create org membership with owner role
    sqlx::query(
        r#"
        INSERT INTO organization_members (id, org_id, user_id, role, created_at)
        VALUES ($1, $2, $3, 'owner', NOW())
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(org_id)
    .bind(user_id)
    .execute(pool)
    .await?;

    tracing::info!(
        user_id = %user_id,
        org_id = %org_id,
        "Successfully created org and user records for OAuth user"
    );

    // For new users, the OAuth user_id is the same as the resolved user_id
    Ok((org_id, "owner".to_string(), user_id))
}

async fn authenticate_api_key(
    auth_state: &AuthState,
    key: &str,
    ip_address: Option<String>,
    user_agent: Option<String>,
) -> Result<AuthUser, AuthError> {
    // Extract key prefix for safe logging (first 12 chars)
    let key_prefix = if key.len() >= 12 {
        key[..12].to_string()
    } else {
        key.to_string()
    };

    // Validate key format and signature
    if !auth_state
        .api_key_manager
        .validate_key(key)
        .map_err(|_| AuthError::InvalidApiKey)?
    {
        // Log failed validation (fire and forget)
        log_api_key_failure(
            auth_state.pool.clone(),
            key_prefix,
            "invalid_format_or_signature".to_string(),
            ip_address,
            user_agent,
        );
        return Err(AuthError::InvalidApiKey);
    }

    // Hash and look up in database
    let key_hash = auth_state.api_key_manager.hash_key(key);

    let api_key: ApiKeyRow = match sqlx::query_as(
        r#"
        SELECT ak.id, ak.org_id, ak.scopes, o.subscription_tier
        FROM api_keys ak
        JOIN organizations o ON o.id = ak.org_id
        WHERE ak.key_hash = $1
          AND (ak.expires_at IS NULL OR ak.expires_at > NOW())
        "#,
    )
    .bind(&key_hash)
    .fetch_optional(&auth_state.pool)
    .await
    {
        Ok(Some(key)) => key,
        Ok(None) => {
            // Log key not found or expired (fire and forget)
            log_api_key_failure(
                auth_state.pool.clone(),
                key_prefix,
                "key_not_found_or_expired".to_string(),
                ip_address,
                user_agent,
            );
            return Err(AuthError::InvalidApiKey);
        }
        Err(_) => return Err(AuthError::DatabaseError),
    };

    // Update last used timestamp (fire and forget)
    let pool = auth_state.pool.clone();
    let key_id = api_key.id;
    tokio::spawn(async move {
        let _ = sqlx::query(
            "UPDATE api_keys SET last_used_at = NOW(), request_count = request_count + 1 WHERE id = $1"
        )
        .bind(key_id)
        .execute(&pool)
        .await;
    });

    Ok(AuthUser {
        user_id: None,
        org_id: Some(api_key.org_id),
        role: "api_key".to_string(),
        email: None,
        auth_method: AuthMethod::ApiKey { key_id: api_key.id },
        session_id: None, // API keys don't have sessions
    })
}

/// Log API key authentication failure to auth_audit_log (fire and forget)
fn log_api_key_failure(
    pool: PgPool,
    key_prefix: String,
    reason: String,
    ip_address: Option<String>,
    user_agent: Option<String>,
) {
    tokio::spawn(async move {
        let metadata = json!({
            "key_prefix": key_prefix,
            "reason": reason,
        });

        let _ = sqlx::query(
            r#"
            INSERT INTO auth_audit_log (
                user_id, email, event_type, severity,
                ip_address, user_agent, metadata, auth_method
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            "#,
        )
        .bind(None::<Uuid>)
        .bind("unknown") // email required by schema
        .bind("login_failed") // matches CHECK constraint
        .bind("warning")
        .bind(ip_address)
        .bind(user_agent)
        .bind(metadata)
        .bind("api_key")
        .execute(&pool)
        .await;
    });
}

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("Missing authentication")]
    MissingAuth,
    #[error("Invalid authentication format")]
    InvalidAuthFormat,
    #[error("Invalid or expired token")]
    InvalidToken,
    #[error("Invalid API key")]
    InvalidApiKey,
    #[error("Insufficient permissions")]
    InsufficientPermissions,
    #[error("No organization found")]
    NoOrganization,
    #[error("Database error")]
    DatabaseError,
    #[error("Internal error: {0}")]
    Internal(String),
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            AuthError::MissingAuth => (StatusCode::UNAUTHORIZED, "Authentication required"),
            AuthError::InvalidAuthFormat => {
                (StatusCode::UNAUTHORIZED, "Invalid authentication format")
            }
            AuthError::InvalidToken => (StatusCode::UNAUTHORIZED, "Invalid or expired token"),
            AuthError::InvalidApiKey => (StatusCode::UNAUTHORIZED, "Invalid API key"),
            AuthError::InsufficientPermissions => {
                (StatusCode::FORBIDDEN, "Insufficient permissions")
            }
            AuthError::NoOrganization => (
                StatusCode::BAD_REQUEST,
                "No organization found. Please create an organization first.",
            ),
            AuthError::DatabaseError => {
                (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error")
            }
            AuthError::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error"),
        };

        let body = Json(json!({
            "error": message,
            "code": status.as_u16()
        }));

        (status, body).into_response()
    }
}

/// Database row for billing status check
#[derive(Debug, FromRow)]
struct BillingStatusRow {
    billing_blocked_at: Option<time::OffsetDateTime>,
}

/// Middleware that blocks API access if org has exceeded grace period
/// Allows billing endpoints so users can pay outstanding invoices
pub async fn require_billing_active(
    State(auth_state): State<AuthState>,
    request: Request,
    next: Next,
) -> Response {
    // Get authenticated user from request extensions
    let auth_user = request.extensions().get::<AuthUser>().cloned();

    if let Some(auth_user) = auth_user {
        // Only check billing status for orgs
        if let Some(org_id) = auth_user.org_id {
            // Allow billing endpoints for payment even if blocked
            let path = request.uri().path();
            if is_billing_endpoint(path) {
                return next.run(request).await;
            }

            // Check if org is blocked
            let billing_status: Option<BillingStatusRow> =
                sqlx::query_as("SELECT billing_blocked_at FROM organizations WHERE id = $1")
                    .bind(org_id)
                    .fetch_optional(&auth_state.pool)
                    .await
                    .ok()
                    .flatten();

            if let Some(status) = billing_status {
                if status.billing_blocked_at.is_some() {
                    return billing_blocked_response();
                }
            }
        }
    }

    next.run(request).await
}

/// Check if the path is a billing endpoint (allowed even when blocked)
fn is_billing_endpoint(path: &str) -> bool {
    // Allow all billing-related endpoints
    if path.starts_with("/api/v1/billing") {
        return true;
    }

    // Allow org/settings endpoints for payment method management
    if path.starts_with("/api/v1/org") && path.contains("settings") {
        return true;
    }

    // Allow user profile access
    if path == "/api/v1/auth/me" {
        return true;
    }

    false
}

/// Response for billing-blocked organizations
fn billing_blocked_response() -> Response {
    let body = Json(json!({
        "error": "payment_required",
        "message": "Service suspended due to unpaid invoices. Please pay outstanding balance to restore access.",
        "code": 402,
        "billing_url": "/billing"
    }));

    (StatusCode::PAYMENT_REQUIRED, body).into_response()
}

/// Combined middleware that requires authentication AND checks billing status
/// This is the recommended middleware for protected endpoints
pub async fn require_auth_with_billing(
    State(auth_state): State<AuthState>,
    mut request: Request,
    next: Next,
) -> Response {
    // First, authenticate the user
    let auth_header = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|h| h.to_str().ok());

    // Extract IP and User-Agent for audit logging
    let ip_address = extract_ip_address(&request);
    let user_agent = request
        .headers()
        .get("User-Agent")
        .and_then(|h| h.to_str().ok())
        .map(String::from);

    let auth_result = match auth_header {
        Some(header) if header.starts_with("Bearer ") => {
            let token = &header[7..];
            authenticate_jwt(&auth_state, token).await
        }
        Some(header) if header.starts_with("ApiKey ") => {
            let key = &header[7..];
            authenticate_api_key(&auth_state, key, ip_address, user_agent).await
        }
        _ => return AuthError::MissingAuth.into_response(),
    };

    let auth_user = match auth_result {
        Ok(user) => user,
        Err(err) => return err.into_response(),
    };

    // Check billing status if user has an org
    if let Some(org_id) = auth_user.org_id {
        let path = request.uri().path();

        // Skip billing check for billing endpoints (so users can pay their invoices)
        if !is_billing_endpoint(path) {
            let billing_status: Option<BillingStatusRow> =
                sqlx::query_as("SELECT billing_blocked_at FROM organizations WHERE id = $1")
                    .bind(org_id)
                    .fetch_optional(&auth_state.pool)
                    .await
                    .ok()
                    .flatten();

            if let Some(status) = billing_status {
                if status.billing_blocked_at.is_some() {
                    return billing_blocked_response();
                }
            }
        }
    }

    // Authentication and billing check passed, proceed
    request.extensions_mut().insert(auth_user);
    next.run(request).await
}

/// Database row for member status check
#[derive(Debug, FromRow)]
struct MemberStatusRow {
    status: String,
}

/// Middleware that enforces read-only access for suspended members
/// Suspended members can only perform GET requests (read-only access)
/// This should be used AFTER authentication middleware
pub async fn require_active_member(
    State(auth_state): State<AuthState>,
    request: Request,
    next: Next,
) -> Response {
    // Get authenticated user from request extensions
    let auth_user = request.extensions().get::<AuthUser>().cloned();

    if let Some(auth_user) = auth_user {
        // Only check for JWT-authenticated users (not API keys which are org-level)
        if let (Some(user_id), Some(org_id)) = (auth_user.user_id, auth_user.org_id) {
            // Skip check for billing endpoints (allow suspended members to view billing)
            let path = request.uri().path();
            if is_billing_endpoint(path) || is_readonly_endpoint(path) {
                return next.run(request).await;
            }

            // Check if member is suspended
            let member_status: Option<MemberStatusRow> = sqlx::query_as(
                r#"
                SELECT status
                FROM organization_members
                WHERE user_id = $1 AND org_id = $2
                "#,
            )
            .bind(user_id)
            .bind(org_id)
            .fetch_optional(&auth_state.pool)
            .await
            .ok()
            .flatten();

            if let Some(status) = member_status {
                if status.status == "suspended" {
                    // Suspended members can only perform read-only operations
                    if !is_read_only_method(request.method()) {
                        return suspended_member_response();
                    }
                }
            }
        }
    }

    next.run(request).await
}

/// Check if the HTTP method is read-only
fn is_read_only_method(method: &Method) -> bool {
    matches!(method, &Method::GET | &Method::HEAD | &Method::OPTIONS)
}

/// Check if the path is inherently read-only (allowed for suspended members)
fn is_readonly_endpoint(path: &str) -> bool {
    // Allow user profile access
    if path == "/api/v1/auth/me" {
        return true;
    }

    // Allow team page viewing
    if path.starts_with("/api/v1/team") && !path.contains("/invite") {
        return true;
    }

    false
}

/// Response for suspended members attempting write operations
fn suspended_member_response() -> Response {
    let body = Json(json!({
        "error": "suspended_member",
        "message": "Your account has read-only access due to plan limits. Contact your organization owner to restore full access.",
        "code": 403,
        "read_only": true
    }));

    (StatusCode::FORBIDDEN, body).into_response()
}

/// Combined middleware: auth + billing + member status (read-only enforcement)
/// This is the most complete middleware for protected endpoints
pub async fn require_full_access(
    State(auth_state): State<AuthState>,
    mut request: Request,
    next: Next,
) -> Response {
    // First, authenticate the user
    let auth_header = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|h| h.to_str().ok());

    // Extract IP and User-Agent for audit logging
    let ip_address = extract_ip_address(&request);
    let user_agent = request
        .headers()
        .get("User-Agent")
        .and_then(|h| h.to_str().ok())
        .map(String::from);

    let auth_result = match auth_header {
        Some(header) if header.starts_with("Bearer ") => {
            let token = &header[7..];
            authenticate_jwt(&auth_state, token).await
        }
        Some(header) if header.starts_with("ApiKey ") => {
            let key = &header[7..];
            authenticate_api_key(&auth_state, key, ip_address, user_agent).await
        }
        _ => return AuthError::MissingAuth.into_response(),
    };

    let auth_user = match auth_result {
        Ok(user) => user,
        Err(err) => return err.into_response(),
    };

    let path = request.uri().path().to_string();
    let method = request.method().clone();

    // Check billing status if user has an org
    if let Some(org_id) = auth_user.org_id {
        // Skip billing check for billing endpoints
        if !is_billing_endpoint(&path) {
            let billing_status: Option<BillingStatusRow> =
                sqlx::query_as("SELECT billing_blocked_at FROM organizations WHERE id = $1")
                    .bind(org_id)
                    .fetch_optional(&auth_state.pool)
                    .await
                    .ok()
                    .flatten();

            if let Some(status) = billing_status {
                if status.billing_blocked_at.is_some() {
                    return billing_blocked_response();
                }
            }
        }

        // Check member suspension status (only for JWT-authenticated users)
        if let Some(user_id) = auth_user.user_id {
            // Skip check for billing/readonly endpoints
            if !is_billing_endpoint(&path) && !is_readonly_endpoint(&path) {
                let member_status: Option<MemberStatusRow> = sqlx::query_as(
                    "SELECT status FROM organization_members WHERE user_id = $1 AND org_id = $2",
                )
                .bind(user_id)
                .bind(org_id)
                .fetch_optional(&auth_state.pool)
                .await
                .ok()
                .flatten();

                if let Some(status) = member_status {
                    if status.status == "suspended" && !is_read_only_method(&method) {
                        return suspended_member_response();
                    }
                }
            }
        }
    }

    // All checks passed, proceed
    request.extensions_mut().insert(auth_user);
    next.run(request).await
}
