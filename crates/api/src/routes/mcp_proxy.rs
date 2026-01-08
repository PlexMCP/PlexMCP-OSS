//! MCP Proxy Endpoint
//!
//! Main endpoint for proxying MCP requests to upstream MCPs.
//! Supports host-based routing for org-specific URLs:
//! - Auto subdomains: swift-cloud-742.plexmcp.com/mcp
//! - Custom subdomains: acme.plexmcp.com/mcp
//! - Custom domains: mcp.company.com/mcp
//! - Legacy: api.plexmcp.com/mcp (API key only)
//!
//! Authentication is always via X-API-Key header, and the API key must
//! belong to the organization resolved from the host.

use axum::{
    body::Bytes,
    extract::State,
    http::{header, HeaderMap, StatusCode},
    response::{sse::Event, IntoResponse, Response, Sse},
};
use futures::stream;
#[cfg(feature = "billing")]
use plexmcp_billing::UsageEvent;
use plexmcp_shared::SubscriptionTier;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::{Duration, Instant};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::mcp::{
    audit::{log_mcp_request, McpRequestLog},
    handlers::{McpProxyHandler, McpTrackedResponse},
    streaming::McpStreamEvent,
    types::{JsonRpcError, JsonRpcId, JsonRpcRequest, JsonRpcResponse},
};
use crate::routing::HostResolveError;
use crate::state::AppState;

/// Handle MCP proxy requests
///
/// Accepts JSON-RPC 2.0 requests and routes them to configured upstream MCPs.
/// Supports host-based routing (subdomain/custom domain) with API key authentication.
pub async fn handle_mcp_request(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    // 1. Extract Host header for routing
    let host = headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    // 2. Resolve org from host (returns None for api.plexmcp.com = legacy mode)
    let resolved_org = match state.host_resolver.resolve(host).await {
        Ok(resolved) => resolved,
        Err(HostResolveError::NotFound(host)) => {
            return error_response(
                None,
                JsonRpcError {
                    code: -32001,
                    message: format!("Unknown host: {}", host),
                    data: None,
                },
                StatusCode::NOT_FOUND,
            );
        }
        Err(HostResolveError::ReservedSubdomain(subdomain)) => {
            return error_response(
                None,
                JsonRpcError {
                    code: -32001,
                    message: format!("Reserved subdomain: {}", subdomain),
                    data: None,
                },
                StatusCode::FORBIDDEN,
            );
        }
        Err(HostResolveError::DatabaseError(e)) => {
            tracing::error!("Host resolution database error: {}", e);
            return error_response(
                None,
                JsonRpcError {
                    code: -32003,
                    message: "Internal server error".to_string(),
                    data: None,
                },
                StatusCode::INTERNAL_SERVER_ERROR,
            );
        }
    };

    // 3. Extract API key from header (always required)
    let api_key = match extract_api_key(&headers) {
        Some(key) => key,
        None => {
            return error_response(
                None,
                JsonRpcError {
                    code: -32001,
                    message: "Missing X-API-Key header".to_string(),
                    data: None,
                },
                StatusCode::UNAUTHORIZED,
            );
        }
    };

    // 4. Validate API key and get the org_id plus MCP access control settings
    let ip_address = extract_ip_from_headers(&headers);
    let user_agent = extract_user_agent(&headers);
    let api_key_validation = match validate_api_key(&state, &api_key, ip_address, user_agent).await
    {
        Ok(validation) => validation,
        Err(e) => {
            return error_response(
                None,
                JsonRpcError {
                    code: -32002,
                    message: e,
                    data: None,
                },
                StatusCode::UNAUTHORIZED,
            );
        }
    };

    // 4.5. Check rate limit for this API key
    match state
        .rate_limiter
        .check_api_key(
            api_key_validation.org_id,
            api_key_validation.api_key_id,
            api_key_validation.rate_limit_rpm,
        )
        .await
    {
        Ok(result) if !result.allowed => {
            tracing::warn!(
                api_key_id = %api_key_validation.api_key_id,
                org_id = %api_key_validation.org_id,
                "Rate limit exceeded"
            );

            return error_response(
                None,
                JsonRpcError {
                    code: -32029, // Custom rate limit error code
                    message: "Rate limit exceeded".to_string(),
                    data: Some(serde_json::json!({
                        "retry_after_seconds": result.retry_after_seconds,
                        "limit_rpm": api_key_validation.rate_limit_rpm,
                        "remaining_minute": result.remaining_minute,
                        "reset_at": result.reset_at.unix_timestamp(),
                    })),
                },
                StatusCode::TOO_MANY_REQUESTS,
            );
        }
        Ok(_) => {
            // Rate limit check passed
        }
        Err(e) => {
            // Fail-open: log error but allow request (availability > strict enforcement)
            tracing::error!(
                api_key_id = %api_key_validation.api_key_id,
                error = %e,
                "Rate limit check failed, allowing request"
            );
        }
    }

    // 5. Verify API key belongs to the resolved org (if host-based routing was used)
    let org_id = if let Some(ref resolved) = resolved_org {
        if api_key_validation.org_id != resolved.org_id {
            return error_response(
                None,
                JsonRpcError {
                    code: -32004,
                    message: "API key does not belong to this organization".to_string(),
                    data: None,
                },
                StatusCode::FORBIDDEN,
            );
        }
        resolved.org_id
    } else {
        // Legacy mode (api.plexmcp.com) - use org from API key
        api_key_validation.org_id
    };

    // 6. Check monthly usage limit (Free tier blocks when over limit) - only when billing feature is enabled
    #[cfg(feature = "billing")]
    {
        let limit_check = match check_monthly_limit(&state, org_id).await {
            Ok(check) => check,
            Err(e) => {
                tracing::error!("Monthly limit check failed: {}", e);
                // Fail-open: allow request if billing check fails (prioritize availability)
                MonthlyLimitCheck {
                    allowed: true,
                    current_usage: 0,
                    limit: u64::MAX,
                    resets_at: OffsetDateTime::now_utc(),
                    tier: SubscriptionTier::Free,
                    overages_disabled: false,
                }
            }
        };

        if !limit_check.allowed {
            // Format reset time as ISO 8601
            let resets_at = limit_check
                .resets_at
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_else(|_| "unknown".to_string());

            // Build upgrade message based on current tier
            let tier_name = limit_check.tier.to_string();
            let limit_formatted = format_number(limit_check.limit);
            let upgrade_message = match limit_check.tier {
                SubscriptionTier::Free => format!(
                    "You've reached your {} tier limit of {} requests/month. Upgrade to Pro for 50,000 requests/month or Team for 200,000 requests/month.",
                    tier_name, limit_formatted
                ),
                SubscriptionTier::Pro => format!(
                    "You've reached your {} tier limit of {} requests/month. Upgrade to Team for 200,000 requests/month or contact us for Enterprise.",
                    tier_name, limit_formatted
                ),
                SubscriptionTier::Team => format!(
                    "You've reached your {} tier limit of {} requests/month. Contact us for Enterprise with unlimited requests.",
                    tier_name, limit_formatted
                ),
                _ => format!(
                    "You've reached your {} tier limit of {} requests/month. Visit billing to explore upgrade options.",
                    tier_name, limit_formatted
                ),
            };

            // Build dashboard URL from config (supports self-hosted deployments)
            let dashboard_url = if state.config.base_domain == "localhost" {
                "http://localhost:3000/billing?upgrade=true".to_string()
            } else {
                format!(
                    "https://dashboard.{}/billing?upgrade=true",
                    state.config.base_domain
                )
            };
            return error_response(
                None,
                JsonRpcError {
                    code: -32029, // Custom rate limit exceeded code
                    message: "Monthly request limit exceeded. Upgrade your plan to continue."
                        .to_string(),
                    data: Some(serde_json::json!({
                        "upgrade_url": dashboard_url,
                        "upgrade_message": upgrade_message,
                        "current_usage": limit_check.current_usage,
                        "limit": limit_check.limit,
                        "resets_at": resets_at,
                        "tier": tier_name,
                    })),
                },
                StatusCode::TOO_MANY_REQUESTS,
            );
        }
    }

    // 7. Check if org is paused due to spend cap (only when billing feature is enabled)
    #[cfg(feature = "billing")]
    if let Some(ref billing) = state.billing {
        match billing.spend_cap.check_paused(org_id).await {
            Ok(true) => {
                // Build dashboard URL from config (supports self-hosted deployments)
                let billing_url = if state.config.base_domain == "localhost" {
                    "http://localhost:3000/billing".to_string()
                } else {
                    format!("https://dashboard.{}/billing", state.config.base_domain)
                };
                return error_response(
                    None,
                    JsonRpcError {
                        code: -32030, // Custom spend cap exceeded code
                        message: "API access paused due to spend cap".to_string(),
                        data: Some(serde_json::json!({
                            "billing_url": billing_url,
                            "reason": "spend_cap_exceeded",
                            "action_required": "Increase spend cap or pay outstanding overages",
                        })),
                    },
                    StatusCode::TOO_MANY_REQUESTS,
                );
            }
            Ok(false) => {} // Not paused, continue
            Err(e) => {
                // Log error but fail-open (allow request if spend cap check fails)
                tracing::error!("Spend cap check failed: {}", e);
            }
        }
    }

    // Extract MCP access control settings
    let mcp_filter = McpFilter {
        mode: api_key_validation.mcp_access_mode,
        allowed_ids: api_key_validation.allowed_mcp_ids,
    };

    // Convert bytes to string
    let body_str = match std::str::from_utf8(&body) {
        Ok(s) => s,
        Err(e) => {
            return error_response(
                None,
                JsonRpcError {
                    code: -32700, // Parse error
                    message: format!("Invalid UTF-8: {}", e),
                    data: None,
                },
                StatusCode::BAD_REQUEST,
            );
        }
    };

    // Parse JSON-RPC request
    let request: JsonRpcRequest = match serde_json::from_str(body_str) {
        Ok(req) => req,
        Err(e) => {
            return error_response(
                None,
                JsonRpcError::parse_error(format!("Invalid JSON: {}", e)),
                StatusCode::BAD_REQUEST,
            );
        }
    };

    // Validate JSON-RPC version
    if request.jsonrpc != "2.0" {
        return error_response(
            request.id,
            JsonRpcError::invalid_request("Invalid JSON-RPC version, expected 2.0"),
            StatusCode::BAD_REQUEST,
        );
    }

    // Check if this is a streaming request (for future SSE support)
    let wants_stream = headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.contains("text/event-stream"))
        .unwrap_or(false);

    // Start timing for latency tracking
    let start_time = Instant::now();

    // Create handler and process request with MCP filtering
    // Returns McpTrackedResponse which includes which MCPs were accessed
    // Uses shared MCP client for HTTP session caching across requests
    let handler = McpProxyHandler::new(
        state.pool.clone(),
        Arc::new(state.config.clone()),
        state.mcp_client.clone(),
    );
    let tracked_response = handler
        .handle_request_filtered(org_id, request.clone(), mcp_filter)
        .await;

    // Calculate latency
    let latency_ms = start_time.elapsed().as_millis() as i32;

    // Log the request for usage tracking (including billing)
    // Pass the tracked response to enable per-MCP usage recording
    log_request(
        &state,
        &api_key,
        org_id,
        &request,
        &tracked_response,
        latency_ms,
    )
    .await;

    if wants_stream {
        // Return SSE stream
        stream_response(tracked_response.response)
    } else {
        // Return JSON response
        json_response(tracked_response.response)
    }
}

/// Extract API key from headers
fn extract_api_key(headers: &HeaderMap) -> Option<String> {
    // Try X-API-Key header first
    if let Some(key) = headers.get("X-API-Key").and_then(|v| v.to_str().ok()) {
        return Some(key.to_string());
    }

    // Try Authorization header with Bearer token
    if let Some(auth) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    {
        if let Some(key) = auth.strip_prefix("Bearer ") {
            return Some(key.to_string());
        }
    }

    // Try MCP_API_KEY from query (for compatibility with some clients)
    None
}

/// API key validation result with MCP access control
struct ApiKeyValidation {
    api_key_id: Uuid,
    org_id: Uuid,
    mcp_access_mode: String,
    allowed_mcp_ids: Option<Vec<Uuid>>,
    rate_limit_rpm: u32,
}

/// MCP filter settings for access control
#[derive(Debug, Clone)]
pub struct McpFilter {
    /// Access mode: "all", "selected", or "none"
    pub mode: String,
    /// When mode is "selected", only these MCP IDs are accessible
    pub allowed_ids: Option<Vec<Uuid>>,
}

/// Monthly usage limit check result (only available with billing feature)
#[cfg(feature = "billing")]
#[allow(dead_code)]
struct MonthlyLimitCheck {
    /// Whether the request should be allowed
    allowed: bool,
    /// Current usage count
    current_usage: i64,
    /// Monthly limit for the tier
    limit: u64,
    /// When the billing period resets
    resets_at: OffsetDateTime,
    /// Current subscription tier
    tier: SubscriptionTier,
    /// Whether overages are disabled for this org (admin override or Free tier)
    overages_disabled: bool,
}

/// Check monthly usage limit for an organization (only available with billing feature)
///
/// Returns whether the request should proceed:
/// - Free tier: BLOCKED when over limit (overages not available)
/// - Pro/Team/Enterprise: ALLOWED (overage billing) unless admin disabled overages
#[cfg(feature = "billing")]
async fn check_monthly_limit(state: &AppState, org_id: Uuid) -> Result<MonthlyLimitCheck, String> {
    // If billing is not configured, allow all requests
    let billing = match &state.billing {
        Some(b) => b,
        None => {
            return Ok(MonthlyLimitCheck {
                allowed: true,
                current_usage: 0,
                limit: u64::MAX,
                resets_at: OffsetDateTime::now_utc(),
                tier: SubscriptionTier::Enterprise,
                overages_disabled: false,
            });
        }
    };

    // Get current billing period usage
    let usage = billing
        .usage
        .get_billing_period_usage(org_id)
        .await
        .map_err(|e| format!("Failed to check usage: {}", e))?;

    // Check if overages are disabled for this org (admin override)
    let overages_disabled_by_admin: bool = sqlx::query_scalar(
        "SELECT COALESCE(overages_disabled, false) FROM organizations WHERE id = $1",
    )
    .bind(org_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| format!("Failed to check overages_disabled: {}", e))?
    .unwrap_or(false);

    // Free tier always has overages disabled
    let overages_disabled = match usage.tier {
        SubscriptionTier::Free | SubscriptionTier::Starter => true,
        _ => overages_disabled_by_admin,
    };

    // Determine if request should be allowed
    let allowed = if overages_disabled {
        // Overages disabled: block when over limit
        !usage.is_over_limit
    } else {
        // Overages enabled: always allow (overage billing handled separately)
        true
    };

    // Calculate period end (first of next month)
    let now = OffsetDateTime::now_utc();
    let next_month = if now.month() == time::Month::December {
        now.replace_year(now.year() + 1)
            .map_err(|e| format!("Failed to calculate next year: {}", e))?
            .replace_month(time::Month::January)
            .map_err(|e| format!("Failed to set month to January: {}", e))?
    } else {
        now.replace_month(now.month().next())
            .map_err(|e| format!("Failed to calculate next month: {}", e))?
    };
    let period_end = next_month
        .replace_day(1)
        .map_err(|e| format!("Failed to set day to 1: {}", e))?
        .replace_time(time::Time::MIDNIGHT);

    Ok(MonthlyLimitCheck {
        allowed,
        current_usage: usage.requests_used,
        limit: usage.requests_limit,
        resets_at: period_end,
        tier: usage.tier,
        overages_disabled,
    })
}

/// Validate API key and return the org_id plus MCP access control settings
async fn validate_api_key(
    state: &AppState,
    api_key: &str,
    ip_address: Option<String>,
    user_agent: Option<String>,
) -> Result<ApiKeyValidation, String> {
    // Extract key prefix for safe logging (first 12 chars)
    let key_prefix = if api_key.len() >= 12 {
        api_key[..12].to_string()
    } else {
        api_key.to_string()
    };

    // First validate the key format and signature
    if !state
        .api_key_manager
        .validate_key(api_key)
        .map_err(|e| format!("Key validation error: {}", e))?
    {
        // Log the failure for SOC 2 CC7.1 compliance
        log_mcp_auth_failure(
            state.pool.clone(),
            key_prefix,
            "invalid_format_or_signature".to_string(),
            ip_address,
            user_agent,
        );
        return Err("Invalid API key format".to_string());
    }

    // Hash the key and look it up in the database
    let key_hash = state.api_key_manager.hash_key(api_key);

    #[derive(sqlx::FromRow)]
    struct ApiKeyRow {
        id: Uuid,
        org_id: Uuid,
        status: String,
        expires_at: Option<time::OffsetDateTime>,
        org_status: String,
        mcp_access_mode: String,
        allowed_mcp_ids: Option<Vec<Uuid>>,
        rate_limit_rpm: i32,
    }

    let result: Option<ApiKeyRow> = sqlx::query_as(
        r#"
        SELECT ak.id, ak.org_id, ak.status, ak.expires_at, o.status as org_status,
               ak.mcp_access_mode, ak.allowed_mcp_ids, ak.rate_limit_rpm
        FROM api_keys ak
        JOIN organizations o ON ak.org_id = o.id
        WHERE ak.key_hash = $1
        "#,
    )
    .bind(&key_hash)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| format!("Database error: {}", e))?;

    match result {
        Some(row) => {
            // Check key status
            if row.status != "active" {
                log_mcp_auth_failure(
                    state.pool.clone(),
                    key_prefix.clone(),
                    format!("key_status_{}", row.status),
                    ip_address.clone(),
                    user_agent.clone(),
                );
                return Err(format!("API key is {}", row.status));
            }

            // Check org status
            if row.org_status != "active" {
                log_mcp_auth_failure(
                    state.pool.clone(),
                    key_prefix.clone(),
                    format!("org_status_{}", row.org_status),
                    ip_address.clone(),
                    user_agent.clone(),
                );
                return Err("Organization is not active".to_string());
            }

            // Check expiration
            if let Some(expires_at) = row.expires_at {
                if expires_at < time::OffsetDateTime::now_utc() {
                    log_mcp_auth_failure(
                        state.pool.clone(),
                        key_prefix.clone(),
                        "key_expired".to_string(),
                        ip_address.clone(),
                        user_agent.clone(),
                    );
                    return Err("API key has expired".to_string());
                }
            }

            // Check if MCP access is disabled
            if row.mcp_access_mode == "none" {
                log_mcp_auth_failure(
                    state.pool.clone(),
                    key_prefix.clone(),
                    "mcp_access_disabled".to_string(),
                    ip_address.clone(),
                    user_agent.clone(),
                );
                return Err("This API key has MCP access disabled".to_string());
            }

            Ok(ApiKeyValidation {
                api_key_id: row.id,
                org_id: row.org_id,
                mcp_access_mode: row.mcp_access_mode,
                allowed_mcp_ids: row.allowed_mcp_ids,
                rate_limit_rpm: row.rate_limit_rpm.max(0) as u32,
            })
        }
        None => {
            log_mcp_auth_failure(
                state.pool.clone(),
                key_prefix,
                "key_not_found".to_string(),
                ip_address,
                user_agent,
            );
            Err("API key not found".to_string())
        }
    }
}

/// Log the MCP request for usage tracking and billing
///
/// Records the request in `mcp_proxy_logs` for debugging and creates usage
/// records for billing. Creates one usage record per MCP that was accessed,
/// enabling accurate per-MCP analytics.
async fn log_request(
    state: &AppState,
    api_key: &str,
    org_id: Uuid,
    request: &JsonRpcRequest,
    tracked_response: &McpTrackedResponse,
    latency_ms: i32,
) {
    let key_hash = state.api_key_manager.hash_key(api_key);

    // Get API key ID and created_by user ID
    #[derive(sqlx::FromRow)]
    struct KeyIdRow {
        id: Uuid,
        created_by: Option<Uuid>, // Nullable in api_keys table
    }

    let key_result: Result<Option<KeyIdRow>, _> =
        sqlx::query_as("SELECT id, created_by FROM api_keys WHERE key_hash = $1")
            .bind(&key_hash)
            .fetch_optional(&state.pool)
            .await;

    let (api_key_id, user_id) = match key_result {
        Ok(Some(row)) => (row.id, row.created_by),
        _ => return, // Skip logging if we can't find the key
    };

    // Determine status from tracked response
    let is_error = tracked_response.response.error.is_some();
    let status = if is_error { "error" } else { "success" };

    // Extract tool name if this is a tools/call
    let tool_name: Option<String> = if request.method == "tools/call" {
        request
            .params
            .as_ref()
            .and_then(|p| p.get("name"))
            .and_then(|n| n.as_str())
            .map(String::from)
    } else {
        None
    };

    // Extract resource URI if this is a resources/read
    let resource_uri: Option<String> = if request.method == "resources/read" {
        request
            .params
            .as_ref()
            .and_then(|p| p.get("uri"))
            .and_then(|u| u.as_str())
            .map(String::from)
    } else {
        None
    };

    // Log to mcp_proxy_logs (best effort, don't fail if logging fails)
    let _ = sqlx::query(
        r#"
        INSERT INTO mcp_proxy_logs (api_key_id, method, tool_name, resource_uri, status, latency_ms, error_message)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        "#,
    )
    .bind(api_key_id)
    .bind(&request.method)
    .bind(&tool_name)
    .bind(&resource_uri)
    .bind(status)
    .bind(latency_ms)
    .bind(if is_error { Some(format!("Error in {} request", request.method)) } else { None::<String> })
    .execute(&state.pool)
    .await;

    // Update API key request count
    let _ = sqlx::query(
        "UPDATE api_keys SET request_count = request_count + 1, last_used_at = NOW() WHERE id = $1",
    )
    .bind(api_key_id)
    .execute(&state.pool)
    .await;

    // Record usage event for billing (if billing is enabled)
    // IMPORTANT: Count each API request as exactly 1 request, regardless of how many MCPs are accessed
    // Per-MCP analytics are tracked separately via mcp_instance_id, but request_count is always 1 total
    #[cfg(feature = "billing")]
    if let Some(billing) = &state.billing {
        if tracked_response.accessed_mcp_ids.is_empty() {
            // No MCPs accessed (e.g., initialize, errors, non-tool methods)
            let _ = billing
                .usage
                .record_event(UsageEvent {
                    org_id,
                    api_key_id: Some(api_key_id),
                    mcp_instance_id: None,
                    request_count: 1,
                    token_count: 0,
                    error_count: if is_error { 1 } else { 0 },
                    latency_ms: Some(latency_ms),
                })
                .await;
        } else {
            // One or more MCPs accessed - create separate event per MCP
            // This ensures accurate per-MCP usage tracking for billing
            let events: Vec<UsageEvent> = tracked_response
                .accessed_mcp_ids
                .iter()
                .map(|&mcp_id| UsageEvent {
                    org_id,
                    api_key_id: Some(api_key_id),
                    mcp_instance_id: Some(mcp_id),
                    request_count: 1, // Always 1 per API request (not 1 per MCP)
                    token_count: 0,
                    error_count: if is_error { 1 } else { 0 },
                    latency_ms: Some(latency_ms),
                })
                .collect();

            let _ = billing.usage.record_events(events).await;
        }
    }

    // SOC 2 Audit Logging: Log MCP request to mcp_request_log table
    // This provides comprehensive audit trail for compliance
    let mcp_server_name = if tracked_response.accessed_mcp_ids.is_empty() {
        "none".to_string() // No MCP accessed (e.g., initialize, errors)
    } else if tracked_response.accessed_mcp_ids.len() == 1 {
        // Single MCP - try to resolve name from tool_name or resource_uri
        tool_name
            .as_ref()
            .and_then(|t| t.split(':').next())
            .or_else(|| {
                resource_uri.as_ref().and_then(|u| {
                    if u.starts_with("plexmcp://") {
                        u.strip_prefix("plexmcp://")
                            .and_then(|s| s.split('/').next())
                    } else {
                        None
                    }
                })
            })
            .unwrap_or("unknown")
            .to_string()
    } else {
        // Multiple MCPs (aggregation requests like tools/list)
        "all".to_string()
    };

    let audit_log = McpRequestLog {
        request_id: Uuid::new_v4(),
        user_id, // Already Option<Uuid> from created_by column
        organization_id: Some(org_id),
        tenant_id: None, // Not using multi-tenant isolation yet
        mcp_server_name,
        endpoint_path: request.method.clone(),
        http_method: "POST".to_string(), // MCP uses JSON-RPC over HTTP POST
        http_status_code: if is_error { 400 } else { 200 },
        request_size_bytes: None, // Could add body.len() as i32 if needed
        response_size_bytes: None,
        latency_ms: Some(latency_ms),
        tokens_used: None, // Not tracked yet - could be added in future
        api_key_id: Some(api_key_id),
        session_id: None, // API key auth doesn't have session
        source_ip: None, // Headers not available in log_request - could pass from handle_mcp_request
        user_agent: None, // Headers not available in log_request - could pass from handle_mcp_request
        error_message: tracked_response
            .response
            .error
            .as_ref()
            .map(|e| e.message.clone()),
        error_code: tracked_response
            .response
            .error
            .as_ref()
            .map(|e| format!("{}", e.code)),
        rate_limit_hit: false, // Rate limiting checked before this function
        quota_exceeded: false, // Quota checked before this function
        metadata: Some(serde_json::json!({
            "tool_name": tool_name,
            "resource_uri": resource_uri,
            "accessed_mcp_count": tracked_response.accessed_mcp_ids.len(),
        })),
    };

    log_mcp_request(state.pool.clone(), audit_log);
}

/// Create a JSON response
fn json_response(response: JsonRpcResponse) -> Response {
    // JSON-RPC protocol: All responses (including errors) use HTTP 200 OK
    let status = StatusCode::OK;

    (
        status,
        [(header::CONTENT_TYPE, "application/json")],
        serde_json::to_string(&response).unwrap_or_default(),
    )
        .into_response()
}

/// Create an error response
fn error_response(id: Option<JsonRpcId>, error: JsonRpcError, status: StatusCode) -> Response {
    let response = JsonRpcResponse::error(id, error);
    (
        status,
        [(header::CONTENT_TYPE, "application/json")],
        serde_json::to_string(&response).unwrap_or_default(),
    )
        .into_response()
}

/// Create an SSE streaming response
/// Stream response as Server-Sent Events
///
/// Sends the final result as an SSE stream. For aggregation requests with partial results,
/// this could be extended to stream results as they arrive.
fn stream_response(response: JsonRpcResponse) -> Response {
    // Wrap the response in a FinalResult event
    let event = McpStreamEvent::FinalResult { response };

    let stream = stream::once(async move {
        Ok::<_, Infallible>(
            Event::default()
                .event(event.event_type())
                .data(event.to_sse_data()),
        )
    });

    Sse::new(stream)
        .keep_alive(
            axum::response::sse::KeepAlive::new()
                .interval(Duration::from_secs(15))
                .event(
                    Event::default()
                        .event("heartbeat")
                        .data("{\"type\":\"heartbeat\"}"),
                ),
        )
        .into_response()
}

/// Create an SSE stream from a channel receiver
///
/// This can be used to stream partial results as they arrive from different MCPs.
/// Currently not used, but provides foundation for future streaming aggregation.
#[allow(dead_code)]
fn stream_from_channel(
    rx: tokio::sync::mpsc::Receiver<McpStreamEvent>,
) -> impl futures::Stream<Item = Result<Event, Infallible>> {
    use futures::StreamExt;
    use tokio_stream::wrappers::ReceiverStream;

    ReceiverStream::new(rx).map(|event| {
        Ok(Event::default()
            .event(event.event_type())
            .data(event.to_sse_data()))
    })
}

/// Format a number with thousands separators (e.g., 1000 -> "1,000")
fn format_number(n: u64) -> String {
    let s = n.to_string();
    let mut result = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    result.chars().rev().collect()
}

/// Log MCP API key authentication failure to auth_audit_log (fire and forget)
/// This ensures SOC 2 CC7.1 compliance by logging all auth failures
fn log_mcp_auth_failure(
    pool: sqlx::PgPool,
    key_prefix: String,
    reason: String,
    ip_address: Option<String>,
    user_agent: Option<String>,
) {
    tokio::spawn(async move {
        let metadata = serde_json::json!({
            "key_prefix": key_prefix,
            "reason": reason,
            "endpoint": "mcp_proxy",
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

/// Extract IP address from request headers (X-Forwarded-For, CF-Connecting-IP, X-Real-IP)
fn extract_ip_from_headers(headers: &HeaderMap) -> Option<String> {
    // Try X-Forwarded-For first (may contain multiple IPs, take first)
    if let Some(xff) = headers.get("X-Forwarded-For") {
        if let Ok(xff_str) = xff.to_str() {
            return xff_str.split(',').next().map(|s| s.trim().to_string());
        }
    }
    // Try Cloudflare header
    if let Some(cf_ip) = headers.get("CF-Connecting-IP") {
        if let Ok(ip) = cf_ip.to_str() {
            return Some(ip.to_string());
        }
    }
    // Try X-Real-IP
    if let Some(real_ip) = headers.get("X-Real-IP") {
        if let Ok(ip) = real_ip.to_str() {
            return Some(ip.to_string());
        }
    }
    None
}

/// Extract user agent from request headers
fn extract_user_agent(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::USER_AGENT)
        .and_then(|h| h.to_str().ok())
        .map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_api_key_from_header() {
        let mut headers = HeaderMap::new();
        headers.insert("X-API-Key", "pmcp_test123".parse().unwrap());
        assert_eq!(extract_api_key(&headers), Some("pmcp_test123".to_string()));
    }

    #[test]
    fn test_extract_api_key_from_bearer() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            "Bearer pmcp_test123".parse().unwrap(),
        );
        assert_eq!(extract_api_key(&headers), Some("pmcp_test123".to_string()));
    }

    #[test]
    fn test_extract_api_key_missing() {
        let headers = HeaderMap::new();
        assert_eq!(extract_api_key(&headers), None);
    }
}
