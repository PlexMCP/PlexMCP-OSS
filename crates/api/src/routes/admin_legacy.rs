//! Platform Admin routes
//!
//! These routes are protected by platform_role check (admin, superadmin).
//! Staff can read but not write.

use axum::{
    extract::{Extension, Path, Query, State},
    http::HeaderMap,
    Json,
};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use std::collections::HashMap;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    audit_constants::{admin_action, event_type, severity, target_type},
    auth::AuthUser,
    error::{ApiError, ApiResult},
    routes::extract_client_ip,
    state::AppState,
};

// =============================================================================
// Helper Types (for non-billing mode)
// =============================================================================

/// Minimal tier change result for when billing is disabled
#[cfg(not(feature = "billing"))]
struct AdminTierChangeResult {
    message: String,
    trial_end: Option<time::OffsetDateTime>,
    stripe_subscription_id: Option<String>,
    stripe_customer_id: Option<String>,
    scheduled: bool,
    scheduled_effective_date: Option<String>,
}

// =============================================================================
// Helper Functions
// =============================================================================

/// Log comprehensive database error details for debugging
fn log_db_err(req_id: Uuid, step: &'static str, e: &sqlx::Error) {
    if let Some(db) = e.as_database_error() {
        tracing::error!(
            %req_id,
            step,
            code = ?db.code(),
            message = db.message(),
            table = ?db.table(),
            constraint = ?db.constraint(),
            full_error = ?e,
            "Database query failed"
        );
    } else {
        tracing::error!(%req_id, step, error = ?e, "Non-database SQLx error");
    }
}

// =============================================================================
// Request/Response Types
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct ListUsersQuery {
    pub page: Option<i64>,
    pub limit: Option<i64>,
    pub search: Option<String>,
    pub tier: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AdminUserListResponse {
    pub users: Vec<AdminUserSummary>,
    pub total: i64,
    pub page: i64,
    pub limit: i64,
}

#[derive(Debug, Serialize)]
pub struct AdminUserSummary {
    pub id: Uuid,
    pub email: String,
    pub org_id: Uuid,
    pub org_name: String,
    pub subscription_tier: String,
    pub platform_role: String,
    pub requests_used: i64,
    pub requests_limit: i64,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
pub struct AdminUserDetailResponse {
    pub id: Uuid,
    pub email: String,
    pub org_id: Uuid,
    pub org_name: String,
    pub subscription_tier: String,
    pub platform_role: String,
    pub has_payment_method: bool,
    // Trial information
    #[serde(with = "time::serde::rfc3339::option")]
    pub trial_start: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub trial_end: Option<OffsetDateTime>,
    pub subscription_status: Option<String>,
    pub admin_trial_granted: Option<bool>,
    pub admin_trial_granted_by: Option<Uuid>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub admin_trial_granted_at: Option<OffsetDateTime>,
    pub admin_trial_reason: Option<String>,
    pub role: String,
    pub email_verified: bool,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_login_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
    pub usage: UsageSummary,
    // Enhanced details
    pub is_suspended: bool,
    #[serde(with = "time::serde::rfc3339::option")]
    pub suspended_at: Option<OffsetDateTime>,
    pub suspended_reason: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub password_changed_at: Option<OffsetDateTime>,
    pub security: UserSecurityInfo,
    pub sessions: Vec<UserSessionInfo>,
    pub login_history: Vec<LoginHistoryEntry>,
    pub oauth_providers: Vec<OAuthProviderInfo>,
    pub trusted_devices: Vec<TrustedDeviceInfo>,
    pub api_keys: UserApiKeysInfo,
}

#[derive(Debug, Serialize)]
pub struct UserSecurityInfo {
    pub two_factor_enabled: bool,
    #[serde(with = "time::serde::rfc3339::option")]
    pub two_factor_enabled_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub two_factor_last_used: Option<OffsetDateTime>,
    pub has_backup_codes: bool,
    pub backup_codes_remaining: i32,
}

#[derive(Debug, Serialize)]
pub struct UserSessionInfo {
    pub id: Uuid,
    pub ip_address: Option<String>,
    pub user_agent: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
    pub is_current: bool,
}

#[derive(Debug, Serialize)]
pub struct LoginHistoryEntry {
    #[serde(with = "time::serde::rfc3339")]
    pub timestamp: OffsetDateTime,
    pub ip_address: Option<String>,
    pub user_agent: Option<String>,
    pub status: String,
    pub failure_reason: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct OAuthProviderInfo {
    pub provider: String,
    pub email: Option<String>,
    pub display_name: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub linked_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_used_at: Option<OffsetDateTime>,
}

#[derive(Debug, Serialize)]
pub struct TrustedDeviceInfo {
    pub id: Uuid,
    pub device_name: Option<String>,
    pub ip_address: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_used_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub expires_at: Option<OffsetDateTime>,
}

#[derive(Debug, Serialize)]
pub struct UserApiKeysInfo {
    pub total_count: i32,
    pub active_count: i32,
    pub total_requests: i64,
    pub keys: Vec<ApiKeyInfo>,
}

#[derive(Debug, Serialize)]
pub struct ApiKeyInfo {
    pub id: Uuid,
    pub name: String,
    pub key_prefix: String,
    pub status: String,
    pub request_count: i64,
    pub created_at: OffsetDateTime,
    pub last_used_at: Option<OffsetDateTime>,
    pub expires_at: Option<OffsetDateTime>,
}

#[derive(Debug, Serialize)]
pub struct UsageSummary {
    pub requests_used: i64,
    pub requests_limit: i64,
    pub percentage_used: f64,
    pub is_over_limit: bool,
    pub billing_period_start: Option<OffsetDateTime>,
    pub billing_period_end: Option<OffsetDateTime>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateUserRequest {
    pub subscription_tier: Option<String>,
    pub platform_role: Option<String>,
    /// Optional trial period in days (0-730) for admin-granted trials
    pub trial_days: Option<u32>,
    /// Reason for tier change (for audit logging)
    pub reason: Option<String>,
    /// Billing interval: "monthly" or "annual" (default: monthly)
    pub billing_interval: Option<String>,
    /// Custom price in cents for Enterprise tier
    pub custom_price_cents: Option<i64>,
    /// Scheduled start date for subscription (ISO 8601 format)
    pub subscription_start_date: Option<String>,
    /// Payment method: "immediate", "invoice", or "trial"
    pub payment_method: Option<String>,
    /// For downgrades: "scheduled" (at period end, default) or "immediate" (now with prorated credit/refund)
    pub downgrade_timing: Option<String>,
    /// For immediate downgrades: "refund" (money back to payment method) or "credit" (Stripe account credit)
    /// Defaults to "credit" for backwards compatibility
    pub refund_type: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PlatformStatsResponse {
    pub total_users: i64,
    pub total_organizations: i64,
    pub total_api_requests_today: i64,
    pub total_api_requests_month: i64,
    pub active_users_7d: i64,
    pub users_by_tier: HashMap<String, i64>,
    pub users_by_platform_role: HashMap<String, i64>,
}

#[derive(Debug, Serialize)]
pub struct AdminOrgListResponse {
    pub organizations: Vec<AdminOrgSummary>,
    pub total: i64,
}

#[derive(Debug, Serialize)]
pub struct AdminOrgSummary {
    pub id: Uuid,
    pub name: String,
    pub subscription_tier: String,
    pub member_count: i64,
    pub requests_used: i64,
    pub requests_limit: i64,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

// =============================================================================
// Database Row Types
// =============================================================================

#[derive(Debug, FromRow)]
struct UserWithOrgRow {
    id: Uuid,
    email: String,
    org_id: Uuid,
    org_name: String,
    subscription_tier: String,
    platform_role: String,
    role: String,
    email_verified: bool,
    last_login_at: Option<OffsetDateTime>,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
    // Suspension columns
    is_suspended: bool,
    suspended_at: Option<OffsetDateTime>,
    suspended_reason: Option<String>,
    password_changed_at: Option<OffsetDateTime>,
    // Trial information from subscriptions table
    trial_start: Option<OffsetDateTime>,
    trial_end: Option<OffsetDateTime>,
    subscription_status: Option<String>,
    admin_trial_granted: Option<bool>,
    admin_trial_granted_by: Option<Uuid>,
    admin_trial_granted_at: Option<OffsetDateTime>,
    admin_trial_reason: Option<String>,
}

#[derive(Debug, FromRow)]
struct PlatformRoleRow {
    platform_role: String,
}

#[derive(Debug, FromRow)]
struct TierCountRow {
    tier: String,
    count: i64,
}

#[derive(Debug, FromRow)]
struct RoleCountRow {
    role: String,
    count: i64,
}

#[derive(Debug, FromRow)]
struct OrgWithStatsRow {
    id: Uuid,
    name: String,
    subscription_tier: String,
    member_count: i64,
    created_at: OffsetDateTime,
}

// Additional row types for enhanced user details
#[derive(Debug, FromRow)]
struct TwoFactorRow {
    is_enabled: bool,
    enabled_at: Option<OffsetDateTime>,
    last_used_at: Option<OffsetDateTime>,
}

#[derive(Debug, FromRow)]
struct BackupCodesCountRow {
    count: i64,
}

#[derive(Debug, FromRow)]
struct SessionRow {
    id: Uuid,
    ip_address: Option<String>,
    user_agent: Option<String>,
    created_at: OffsetDateTime,
    expires_at: OffsetDateTime,
}

#[derive(Debug, FromRow)]
struct LoginHistoryRow {
    created_at: OffsetDateTime,
    ip_address: Option<String>,
    user_agent: Option<String>,
    status: Option<String>,
    failure_reason: Option<String>,
}

#[derive(Debug, FromRow)]
struct OAuthProviderRow {
    provider: String,
    email: Option<String>,
    display_name: Option<String>,
    linked_at: OffsetDateTime,
    last_used_at: Option<OffsetDateTime>,
}

#[derive(Debug, FromRow)]
struct TrustedDeviceRow {
    id: Uuid,
    device_name: Option<String>,
    ip_address: Option<String>,
    last_used_at: Option<OffsetDateTime>,
    expires_at: Option<OffsetDateTime>,
}

#[derive(Debug, FromRow)]
struct ApiKeyRow {
    id: Uuid,
    name: String,
    key_prefix: String,
    status: String,
    request_count: i64,
    created_at: OffsetDateTime,
    last_used_at: Option<OffsetDateTime>,
    expires_at: Option<OffsetDateTime>,
}

// =============================================================================
// Helper Functions
// =============================================================================

/// Check if the authenticated user has platform admin privileges
async fn require_platform_admin(
    state: &AppState,
    auth_user: &AuthUser,
    require_write: bool,
) -> ApiResult<Uuid> {
    let user_id = auth_user.user_id.ok_or(ApiError::Unauthorized)?;

    let row: Option<PlatformRoleRow> = sqlx::query_as(
        "SELECT platform_role::TEXT as platform_role FROM public.users WHERE id = $1",
    )
    .bind(user_id)
    .fetch_optional(&state.pool)
    .await?;

    let platform_role = row
        .map(|r| r.platform_role)
        .unwrap_or_else(|| "user".to_string());

    match platform_role.as_str() {
        "superadmin" | "admin" => Ok(user_id),
        "staff" if !require_write => Ok(user_id), // Staff can read but not write
        _ => {
            tracing::warn!(
                user_id = %user_id,
                platform_role = %platform_role,
                "Unauthorized admin access attempt"
            );
            Err(ApiError::Forbidden)
        }
    }
}

/// Get tier limits
fn get_tier_limit(tier: &str) -> i64 {
    match tier {
        "free" => 1_000,
        "pro" => 50_000,
        "team" => 500_000,
        "enterprise" => i64::MAX,
        _ => 1_000,
    }
}

/// Enhanced audit logging with SOC 2 compliance and error propagation
#[allow(clippy::too_many_arguments)]
async fn log_admin_action(
    pool: &sqlx::PgPool,
    admin_user_id: Uuid,
    action: &str,
    target_type: &str,
    target_id: Option<Uuid>,
    details: Option<serde_json::Value>,
    event_type: &str,
    severity: &str,
    ip_address: Option<String>,
    user_agent: Option<String>,
    session_id: Option<Uuid>,
) -> ApiResult<()> {
    // Sanitize PII from details (HIGH issue fix)
    let sanitized_details = details.map(sanitize_pii);

    sqlx::query(
        r#"
        INSERT INTO admin_audit_log (
            admin_user_id, action, target_type, target_id, details,
            event_type, severity, ip_address, user_agent, session_id
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
        "#, // Removed ::inet cast - now using TEXT type
    )
    .bind(admin_user_id)
    .bind(action)
    .bind(target_type)
    .bind(target_id)
    .bind(sanitized_details)
    .bind(event_type)
    .bind(severity)
    .bind(ip_address)
    .bind(user_agent)
    .bind(session_id)
    .execute(pool)
    .await
    .map_err(|e| {
        tracing::error!(
            error = %e,
            action = %action,
            event_type = %event_type,
            "Failed to write audit log - CRITICAL COMPLIANCE VIOLATION"
        );
        ApiError::Internal
    })?;

    Ok(())
}

/// Sanitize PII from audit log details (HIGH issue fix)
fn sanitize_pii(mut details: serde_json::Value) -> serde_json::Value {
    if let Some(obj) = details.as_object_mut() {
        let sensitive_keys = [
            "password",
            "password_hash",
            "token",
            "api_key",
            "secret",
            "private_key",
            "credit_card",
            "ssn",
            "bearer_token",
        ];

        for key in &sensitive_keys {
            if obj.contains_key(*key) {
                obj.insert(key.to_string(), serde_json::json!("[REDACTED]"));
            }
        }
    }
    details
}

/// Extract IP, user agent, and session ID from request (CRITICAL issue fix)
fn extract_audit_context(
    headers: &HeaderMap,
    auth_user: &AuthUser,
) -> (Option<String>, Option<String>, Option<Uuid>) {
    let ip_address = extract_client_ip(headers);
    let user_agent = headers
        .get("user-agent")
        .and_then(|h| h.to_str().ok())
        .map(|s| s.to_string());

    // Extract session_id from AuthUser (populated by middleware for JWT auth)
    let session_id = auth_user.session_id;

    (ip_address, user_agent, session_id)
}

// =============================================================================
// Handlers
// =============================================================================

/// List all users with their organization and usage info
pub async fn list_users(
    State(state): State<AppState>,
    Extension(auth_user): Extension<AuthUser>,
    Query(query): Query<ListUsersQuery>,
) -> ApiResult<Json<AdminUserListResponse>> {
    require_platform_admin(&state, &auth_user, false).await?;

    let page = query.page.unwrap_or(1).max(1);
    let limit = query.limit.unwrap_or(50).min(100);
    let offset = (page - 1) * limit;

    // Build dynamic query based on filters
    let mut sql = String::from(
        r#"
        SELECT
            u.id, u.email, u.org_id, o.name as org_name,
            o.subscription_tier, u.platform_role::TEXT as platform_role, u.role,
            u.email_verified, u.last_login_at, u.created_at, u.updated_at,
            u.is_suspended, u.suspended_at, u.suspended_reason, u.password_changed_at,
            s.trial_start,
            s.trial_end,
            s.status as subscription_status,
            s.admin_trial_granted,
            s.admin_trial_granted_by,
            s.admin_trial_granted_at,
            s.admin_trial_reason
        FROM users u
        JOIN organizations o ON o.id = u.org_id
        LEFT JOIN subscriptions s ON s.org_id = u.org_id
        WHERE 1=1
        "#,
    );

    if query.search.is_some() {
        sql.push_str(" AND u.email ILIKE '%' || $3 || '%'");
    }
    if query.tier.is_some() {
        sql.push_str(" AND o.subscription_tier = $4");
    }

    sql.push_str(" ORDER BY u.created_at DESC LIMIT $1 OFFSET $2");

    let users: Vec<UserWithOrgRow> = if let Some(ref search) = query.search {
        if let Some(ref tier) = query.tier {
            sqlx::query_as(&sql)
                .bind(limit)
                .bind(offset)
                .bind(search)
                .bind(tier)
                .fetch_all(&state.pool)
                .await?
        } else {
            let sql_no_tier = sql.replace(" AND o.subscription_tier = $4", "");
            sqlx::query_as(&sql_no_tier)
                .bind(limit)
                .bind(offset)
                .bind(search)
                .fetch_all(&state.pool)
                .await?
        }
    } else if let Some(ref tier) = query.tier {
        let sql_no_search = sql
            .replace(" AND u.email ILIKE '%' || $3 || '%'", "")
            .replace("$4", "$3");
        sqlx::query_as(&sql_no_search)
            .bind(limit)
            .bind(offset)
            .bind(tier)
            .fetch_all(&state.pool)
            .await?
    } else {
        let sql_base = r#"
            SELECT
                u.id, u.email, u.org_id, o.name as org_name,
                o.subscription_tier, u.platform_role::TEXT as platform_role, u.role,
                u.email_verified, u.last_login_at, u.created_at, u.updated_at,
                u.is_suspended, u.suspended_at, u.suspended_reason, u.password_changed_at,
                s.trial_start,
                s.trial_end,
                s.status as subscription_status,
                s.admin_trial_granted,
                s.admin_trial_granted_by,
                s.admin_trial_granted_at,
                s.admin_trial_reason
            FROM users u
            JOIN organizations o ON o.id = u.org_id
            LEFT JOIN subscriptions s ON s.org_id = u.org_id
            ORDER BY u.created_at DESC
            LIMIT $1 OFFSET $2
        "#;
        sqlx::query_as(sql_base)
            .bind(limit)
            .bind(offset)
            .fetch_all(&state.pool)
            .await?
    };

    // Get total count
    let total: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users")
        .fetch_one(&state.pool)
        .await?;

    // Build response with usage info
    let mut user_summaries = Vec::new();
    for user in users {
        // Get usage for this user's org
        let requests_used = get_org_usage(&state.pool, user.org_id).await.unwrap_or(0);
        let requests_limit = get_tier_limit(&user.subscription_tier);

        user_summaries.push(AdminUserSummary {
            id: user.id,
            email: user.email,
            org_id: user.org_id,
            org_name: user.org_name,
            subscription_tier: user.subscription_tier,
            platform_role: user.platform_role,
            requests_used,
            requests_limit,
            created_at: user.created_at,
        });
    }

    Ok(Json(AdminUserListResponse {
        users: user_summaries,
        total: total.0,
        page,
        limit,
    }))
}

/// Get a specific user's details with enhanced security and activity info
pub async fn get_user(
    State(state): State<AppState>,
    Extension(auth_user): Extension<AuthUser>,
    Path(user_id): Path<Uuid>,
) -> ApiResult<Json<AdminUserDetailResponse>> {
    require_platform_admin(&state, &auth_user, false).await?;

    // Get basic user info with suspension columns and trial information
    let user: UserWithOrgRow = sqlx::query_as(
        r#"
        SELECT
            u.id, u.email, u.org_id, o.name as org_name,
            o.subscription_tier, u.platform_role::TEXT as platform_role, u.role,
            u.email_verified, u.last_login_at, u.created_at, u.updated_at,
            u.is_suspended, u.suspended_at, u.suspended_reason, u.password_changed_at,
            s.trial_start,
            s.trial_end,
            s.status as subscription_status,
            s.admin_trial_granted,
            s.admin_trial_granted_by,
            s.admin_trial_granted_at,
            s.admin_trial_reason
        FROM users u
        JOIN organizations o ON o.id = u.org_id
        LEFT JOIN subscriptions s ON s.org_id = u.org_id
        WHERE u.id = $1
        "#,
    )
    .bind(user_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(ApiError::NotFound)?;

    // Get usage details
    let requests_used = get_org_usage(&state.pool, user.org_id).await.unwrap_or(0);
    let requests_limit = get_tier_limit(&user.subscription_tier);

    // Get 2FA status
    let two_factor: Option<TwoFactorRow> = sqlx::query_as(
        r#"
        SELECT is_enabled, enabled_at, last_used_at
        FROM user_2fa
        WHERE user_id = $1
        "#,
    )
    .bind(user_id)
    .fetch_optional(&state.pool)
    .await?;

    // Get backup codes count
    let backup_codes_count: BackupCodesCountRow = sqlx::query_as(
        r#"
        SELECT COALESCE(COUNT(*), 0) as count
        FROM user_2fa_backup_codes
        WHERE user_id = $1 AND used_at IS NULL
        "#,
    )
    .bind(user_id)
    .fetch_one(&state.pool)
    .await
    .unwrap_or(BackupCodesCountRow { count: 0 });

    // Get active sessions
    let sessions: Vec<SessionRow> = sqlx::query_as(
        r#"
        SELECT id, ip_address::TEXT as ip_address, user_agent, created_at, expires_at
        FROM sessions
        WHERE user_id = $1 AND revoked_at IS NULL AND expires_at > NOW()
        ORDER BY created_at DESC
        LIMIT 20
        "#,
    )
    .bind(user_id)
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    // Get login history from audit_logs
    let login_history: Vec<LoginHistoryRow> = sqlx::query_as(
        r#"
        SELECT
            created_at,
            ip_address::TEXT as ip_address,
            user_agent,
            details->>'status' as status,
            details->>'failure_reason' as failure_reason
        FROM audit_logs
        WHERE user_id = $1 AND action LIKE 'login%'
        ORDER BY created_at DESC
        LIMIT 50
        "#,
    )
    .bind(user_id)
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    // Get OAuth providers
    let oauth_providers: Vec<OAuthProviderRow> = sqlx::query_as(
        r#"
        SELECT provider, email, display_name, linked_at, last_used_at
        FROM user_identities
        WHERE user_id = $1
        ORDER BY linked_at DESC
        "#,
    )
    .bind(user_id)
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    // Get trusted devices
    let trusted_devices: Vec<TrustedDeviceRow> = sqlx::query_as(
        r#"
        SELECT id, device_name, ip_address, last_used_at, expires_at
        FROM user_trusted_devices
        WHERE user_id = $1 AND (expires_at IS NULL OR expires_at > NOW())
        ORDER BY last_used_at DESC NULLS LAST
        "#,
    )
    .bind(user_id)
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    // Get API keys for user's org
    let api_keys: Vec<ApiKeyRow> = sqlx::query_as(
        r#"
        SELECT id, name, key_prefix, status::TEXT as status, request_count, created_at, last_used_at, expires_at
        FROM api_keys
        WHERE org_id = $1
        ORDER BY created_at DESC
        "#
    )
    .bind(user.org_id)
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    // Build security info
    let security = UserSecurityInfo {
        two_factor_enabled: two_factor.as_ref().map(|t| t.is_enabled).unwrap_or(false),
        two_factor_enabled_at: two_factor.as_ref().and_then(|t| t.enabled_at),
        two_factor_last_used: two_factor.as_ref().and_then(|t| t.last_used_at),
        has_backup_codes: backup_codes_count.count > 0,
        backup_codes_remaining: backup_codes_count.count as i32,
    };

    // Build sessions list
    let session_infos: Vec<UserSessionInfo> = sessions
        .into_iter()
        .map(|s| UserSessionInfo {
            id: s.id,
            ip_address: s.ip_address,
            user_agent: s.user_agent,
            created_at: s.created_at,
            expires_at: s.expires_at,
            is_current: false, // Admin view doesn't track current session
        })
        .collect();

    // Build login history
    let login_entries: Vec<LoginHistoryEntry> = login_history
        .into_iter()
        .map(|l| LoginHistoryEntry {
            timestamp: l.created_at,
            ip_address: l.ip_address,
            user_agent: l.user_agent,
            status: l.status.unwrap_or_else(|| "unknown".to_string()),
            failure_reason: l.failure_reason,
        })
        .collect();

    // Build OAuth providers list
    let oauth_infos: Vec<OAuthProviderInfo> = oauth_providers
        .into_iter()
        .map(|o| OAuthProviderInfo {
            provider: o.provider,
            email: o.email,
            display_name: o.display_name,
            linked_at: o.linked_at,
            last_used_at: o.last_used_at,
        })
        .collect();

    // Build trusted devices list
    let device_infos: Vec<TrustedDeviceInfo> = trusted_devices
        .into_iter()
        .map(|d| TrustedDeviceInfo {
            id: d.id,
            device_name: d.device_name,
            ip_address: d.ip_address,
            last_used_at: d.last_used_at,
            expires_at: d.expires_at,
        })
        .collect();

    // Build API keys info
    let active_keys = api_keys.iter().filter(|k| k.status == "active").count() as i32;
    let total_requests: i64 = api_keys.iter().map(|k| k.request_count).sum();
    let api_key_infos: Vec<ApiKeyInfo> = api_keys
        .into_iter()
        .map(|k| ApiKeyInfo {
            id: k.id,
            name: k.name,
            key_prefix: k.key_prefix,
            status: k.status,
            request_count: k.request_count,
            created_at: k.created_at,
            last_used_at: k.last_used_at,
            expires_at: k.expires_at,
        })
        .collect();

    // Check if organization has a payment method on file in Stripe (only when billing is enabled)
    #[cfg(feature = "billing")]
    let has_payment_method: bool = if let Some(billing) = &state.billing {
        billing
            .customer
            .has_payment_method(user.org_id)
            .await
            .unwrap_or(false)
    } else {
        false
    };
    #[cfg(not(feature = "billing"))]
    let has_payment_method: bool = false;

    Ok(Json(AdminUserDetailResponse {
        id: user.id,
        email: user.email,
        org_id: user.org_id,
        org_name: user.org_name,
        subscription_tier: user.subscription_tier,
        platform_role: user.platform_role,
        has_payment_method,
        trial_start: user.trial_start,
        trial_end: user.trial_end,
        subscription_status: user.subscription_status,
        admin_trial_granted: user.admin_trial_granted,
        admin_trial_granted_by: user.admin_trial_granted_by,
        admin_trial_granted_at: user.admin_trial_granted_at,
        admin_trial_reason: user.admin_trial_reason,
        role: user.role,
        email_verified: user.email_verified,
        last_login_at: user.last_login_at,
        created_at: user.created_at,
        updated_at: user.updated_at,
        usage: UsageSummary {
            requests_used,
            requests_limit,
            percentage_used: if requests_limit > 0 {
                (requests_used as f64 / requests_limit as f64) * 100.0
            } else {
                0.0
            },
            is_over_limit: requests_used >= requests_limit,
            billing_period_start: None,
            billing_period_end: None,
        },
        is_suspended: user.is_suspended,
        suspended_at: user.suspended_at,
        suspended_reason: user.suspended_reason,
        password_changed_at: user.password_changed_at,
        security,
        sessions: session_infos,
        login_history: login_entries,
        oauth_providers: oauth_infos,
        trusted_devices: device_infos,
        api_keys: UserApiKeysInfo {
            total_count: api_key_infos.len() as i32,
            active_count: active_keys,
            total_requests,
            keys: api_key_infos,
        },
    }))
}

/// Update a user's subscription tier or platform role
pub async fn update_user(
    State(state): State<AppState>,
    Extension(auth_user): Extension<AuthUser>,
    headers: HeaderMap,
    Path(user_id): Path<Uuid>,
    Json(req): Json<UpdateUserRequest>,
) -> ApiResult<Json<AdminUserDetailResponse>> {
    require_platform_admin(&state, &auth_user, true).await?;

    let admin_user_id = auth_user.user_id.ok_or(ApiError::Unauthorized)?;
    let (ip_address, user_agent, session_id) = extract_audit_context(&headers, &auth_user);

    // Get current user data
    let current_user: UserWithOrgRow = sqlx::query_as(
        r#"
        SELECT
            u.id, u.email, u.org_id, o.name as org_name,
            o.subscription_tier, u.platform_role::TEXT as platform_role, u.role,
            u.email_verified, u.last_login_at, u.created_at, u.updated_at,
            u.is_suspended, u.suspended_at, u.suspended_reason, u.password_changed_at,
            s.trial_start,
            s.trial_end,
            s.status as subscription_status,
            s.admin_trial_granted,
            s.admin_trial_granted_by,
            s.admin_trial_granted_at,
            s.admin_trial_reason
        FROM users u
        JOIN organizations o ON o.id = u.org_id
        LEFT JOIN subscriptions s ON s.org_id = u.org_id
        WHERE u.id = $1
        "#,
    )
    .bind(user_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(ApiError::NotFound)?;

    // Prevent modifying superadmins unless you're a superadmin
    if current_user.platform_role == "superadmin" {
        let admin_role: Option<PlatformRoleRow> = sqlx::query_as(
            "SELECT platform_role::TEXT as platform_role FROM public.users WHERE id = $1",
        )
        .bind(admin_user_id)
        .fetch_optional(&state.pool)
        .await?;

        if admin_role.map(|r| r.platform_role).unwrap_or_default() != "superadmin" {
            return Err(ApiError::Forbidden);
        }
    }

    // Update subscription tier if provided
    // This now properly syncs with Stripe to prevent database/Stripe drift
    if let Some(ref tier) = req.subscription_tier {
        let valid_tiers = ["free", "pro", "team", "enterprise"];
        if !valid_tiers.contains(&tier.as_str()) {
            return Err(ApiError::Validation(format!(
                "Invalid tier. Must be one of: {}",
                valid_tiers.join(", ")
            )));
        }

        // Validate billing_interval
        if let Some(ref interval) = req.billing_interval {
            if !["monthly", "annual"].contains(&interval.as_str()) {
                return Err(ApiError::Validation(
                    "billing_interval must be 'monthly' or 'annual'".into(),
                ));
            }
        }

        // Validate payment_method
        if let Some(ref method) = req.payment_method {
            if !["immediate", "invoice", "trial"].contains(&method.as_str()) {
                return Err(ApiError::Validation(
                    "payment_method must be 'immediate', 'invoice', or 'trial'".into(),
                ));
            }
            if method == "trial" && req.trial_days.is_none() {
                return Err(ApiError::Validation(
                    "trial_days required when payment_method is 'trial'".into(),
                ));
            }
        }

        // Validate Enterprise pricing
        if tier == "enterprise" && req.custom_price_cents.is_none() {
            return Err(ApiError::Validation(
                "custom_price_cents required for Enterprise tier".into(),
            ));
        }

        if let Some(price) = req.custom_price_cents {
            if price <= 0 {
                return Err(ApiError::Validation(
                    "custom_price_cents must be positive".into(),
                ));
            }
            if tier != "enterprise" {
                return Err(ApiError::Validation(
                    "custom_price_cents only allowed for Enterprise tier".into(),
                ));
            }
        }

        // Parse and validate subscription_start_date
        let start_date = if let Some(ref date_str) = req.subscription_start_date {
            let parsed = time::OffsetDateTime::parse(
                date_str,
                &time::format_description::well_known::Rfc3339,
            )
            .map_err(|_| {
                ApiError::Validation("Invalid date format (use ISO 8601/RFC 3339)".into())
            })?;

            if parsed <= time::OffsetDateTime::now_utc() {
                return Err(ApiError::Validation(
                    "subscription_start_date must be in the future".into(),
                ));
            }
            Some(parsed)
        } else {
            None
        };

        // Execute tier change with Stripe sync (when billing is enabled)
        #[cfg(feature = "billing")]
        let result = {
            let billing = state
                .billing
                .as_ref()
                .ok_or_else(|| ApiError::Database("Billing not configured".into()))?;

            billing.subscriptions.admin_change_tier(
                current_user.org_id,
                plexmcp_billing::AdminTierChangeParams {
                    new_tier: tier.clone(),
                    trial_days: req.trial_days,
                    reason: req.reason.clone()
                        .unwrap_or_else(|| "Admin manual tier change".to_string()),
                    skip_payment_validation: req.trial_days.is_some(),
                    billing_interval: req.billing_interval.clone(),
                    custom_price_cents: req.custom_price_cents,
                    subscription_start_date: start_date,
                    payment_method: req.payment_method.clone(),
                    admin_user_id: Some(admin_user_id),
                    downgrade_timing: req.downgrade_timing.clone(),
                    refund_type: req.refund_type.clone(),
                }
            ).await.map_err(|e| {
                match e {
                    plexmcp_billing::BillingError::PaymentMethodRequired => ApiError::Validation(
                        "Cannot upgrade: Organization has no payment method. User must add a payment method before upgrading to a paid tier. Alternatively, specify trial_days to grant a trial period.".to_string()
                    ),
                    plexmcp_billing::BillingError::InvalidTier(msg) => ApiError::Validation(msg),
                    _ => ApiError::Database(format!("Billing error: {}", e)),
                }
            })?
        };

        // Fallback when billing is not enabled: just update the database
        #[cfg(not(feature = "billing"))]
        let result = {
            // Update tier directly in database (using non-macro query to avoid sqlx offline cache requirement)
            sqlx::query("UPDATE organizations SET subscription_tier = $1 WHERE id = $2")
                .bind(&tier)
                .bind(current_user.org_id)
                .execute(&state.pool)
                .await
                .map_err(|e| ApiError::Database(e.to_string()))?;

            // Return a minimal result struct for audit logging
            AdminTierChangeResult {
                message: format!("Tier changed to {} (billing disabled)", tier),
                trial_end: None,
                stripe_subscription_id: None,
                stripe_customer_id: None,
                scheduled: false,
                scheduled_effective_date: None,
            }
        };

        // Enhanced audit logging with Stripe IDs
        log_admin_action(
            &state.pool,
            admin_user_id,
            admin_action::SUBSCRIPTION_CHANGED,
            target_type::ORGANIZATION,
            Some(current_user.org_id),
            Some(serde_json::json!({
                "old_tier": current_user.subscription_tier,
                "new_tier": tier,
                "trial_days": req.trial_days,
                "trial_end": result.trial_end,
                "reason": req.reason,
                "stripe_subscription_id": result.stripe_subscription_id,
                "stripe_customer_id": result.stripe_customer_id,
                "scheduled": result.scheduled,
                "scheduled_effective_date": result.scheduled_effective_date,
                "message": result.message,
                "user_id": user_id
            })),
            event_type::ADMIN_ACTION,
            severity::WARNING,
            ip_address.clone(),
            user_agent.clone(),
            session_id,
        )
        .await?;
    }

    // Update platform role if provided
    if let Some(ref role) = req.platform_role {
        let valid_roles = ["user", "staff", "admin", "superadmin"];
        if !valid_roles.contains(&role.as_str()) {
            return Err(ApiError::Validation(format!(
                "Invalid platform role. Must be one of: {}",
                valid_roles.join(", ")
            )));
        }

        // Only superadmins can grant superadmin
        if role == "superadmin" {
            let admin_role: Option<PlatformRoleRow> = sqlx::query_as(
                "SELECT platform_role::TEXT as platform_role FROM public.users WHERE id = $1",
            )
            .bind(admin_user_id)
            .fetch_optional(&state.pool)
            .await?;

            if admin_role.map(|r| r.platform_role).unwrap_or_default() != "superadmin" {
                return Err(ApiError::Forbidden);
            }
        }

        sqlx::query(
            "UPDATE users SET platform_role = $1::platform_role, updated_at = NOW() WHERE id = $2",
        )
        .bind(role)
        .bind(user_id)
        .execute(&state.pool)
        .await?;

        log_admin_action(
            &state.pool,
            admin_user_id,
            admin_action::ROLE_CHANGED,
            target_type::USER,
            Some(user_id),
            Some(serde_json::json!({
                "old_role": current_user.platform_role,
                "new_role": role
            })),
            event_type::USER_MANAGEMENT,
            severity::CRITICAL,
            ip_address.clone(),
            user_agent.clone(),
            session_id,
        )
        .await?;
    }

    // Fetch and return updated user
    get_user(State(state), Extension(auth_user), Path(user_id)).await
}

/// Get platform-wide statistics
pub async fn get_stats(
    State(state): State<AppState>,
    Extension(auth_user): Extension<AuthUser>,
) -> ApiResult<Json<PlatformStatsResponse>> {
    require_platform_admin(&state, &auth_user, false).await?;

    // Total users
    let total_users: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users")
        .fetch_one(&state.pool)
        .await?;

    // Total organizations
    let total_orgs: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM organizations")
        .fetch_one(&state.pool)
        .await?;

    // API requests today
    let requests_today: (i64,) = sqlx::query_as(
        r#"
        SELECT COALESCE(SUM(request_count), 0)
        FROM usage_records
        WHERE period_start >= date_trunc('day', NOW())
        "#,
    )
    .fetch_one(&state.pool)
    .await
    .unwrap_or((0,));

    // API requests this month
    let requests_month: (i64,) = sqlx::query_as(
        r#"
        SELECT COALESCE(SUM(request_count), 0)
        FROM usage_records
        WHERE period_start >= date_trunc('month', NOW())
        "#,
    )
    .fetch_one(&state.pool)
    .await
    .unwrap_or((0,));

    // Active users in last 7 days
    let active_7d: (i64,) = sqlx::query_as(
        r#"
        SELECT COUNT(DISTINCT user_id)
        FROM (
            SELECT created_by as user_id FROM api_keys WHERE last_used_at > NOW() - interval '7 days'
            UNION
            SELECT id FROM users WHERE last_login_at > NOW() - interval '7 days'
        ) active
        "#
    )
    .fetch_one(&state.pool)
    .await
    .unwrap_or((0,));

    // Users by tier
    let tier_counts: Vec<TierCountRow> = sqlx::query_as(
        r#"
        SELECT o.subscription_tier as tier, COUNT(u.id) as count
        FROM users u
        JOIN organizations o ON o.id = u.org_id
        GROUP BY o.subscription_tier
        "#,
    )
    .fetch_all(&state.pool)
    .await?;

    let mut users_by_tier = HashMap::new();
    for row in tier_counts {
        users_by_tier.insert(row.tier, row.count);
    }

    // Users by platform role
    let role_counts: Vec<RoleCountRow> = sqlx::query_as(
        r#"
        SELECT platform_role::text as role, COUNT(*) as count
        FROM users
        GROUP BY platform_role
        "#,
    )
    .fetch_all(&state.pool)
    .await?;

    let mut users_by_platform_role = HashMap::new();
    for row in role_counts {
        users_by_platform_role.insert(row.role, row.count);
    }

    Ok(Json(PlatformStatsResponse {
        total_users: total_users.0,
        total_organizations: total_orgs.0,
        total_api_requests_today: requests_today.0,
        total_api_requests_month: requests_month.0,
        active_users_7d: active_7d.0,
        users_by_tier,
        users_by_platform_role,
    }))
}

/// List all organizations
pub async fn list_organizations(
    State(state): State<AppState>,
    Extension(auth_user): Extension<AuthUser>,
) -> ApiResult<Json<AdminOrgListResponse>> {
    require_platform_admin(&state, &auth_user, false).await?;

    let orgs: Vec<OrgWithStatsRow> = sqlx::query_as(
        r#"
        SELECT
            o.id, o.name, o.subscription_tier,
            (SELECT COUNT(*) FROM users WHERE org_id = o.id) as member_count,
            o.created_at
        FROM organizations o
        ORDER BY o.created_at DESC
        "#,
    )
    .fetch_all(&state.pool)
    .await?;

    let total = orgs.len() as i64;

    let mut org_summaries = Vec::new();
    for org in orgs {
        let requests_used = get_org_usage(&state.pool, org.id).await.unwrap_or(0);
        let requests_limit = get_tier_limit(&org.subscription_tier);

        org_summaries.push(AdminOrgSummary {
            id: org.id,
            name: org.name,
            subscription_tier: org.subscription_tier,
            member_count: org.member_count,
            requests_used,
            requests_limit,
            created_at: org.created_at,
        });
    }

    Ok(Json(AdminOrgListResponse {
        organizations: org_summaries,
        total,
    }))
}

/// Get usage for an organization
async fn get_org_usage(pool: &sqlx::PgPool, org_id: Uuid) -> Result<i64, sqlx::Error> {
    let result: (i64,) = sqlx::query_as(
        r#"
        SELECT COALESCE(SUM(request_count), 0)
        FROM usage_records
        WHERE org_id = $1
        AND period_start >= date_trunc('month', NOW())
        AND period_end <= NOW() + interval '1 day'
        "#,
    )
    .bind(org_id)
    .fetch_one(pool)
    .await?;

    Ok(result.0)
}

// =============================================================================
// Admin User Action Endpoints
// =============================================================================

/// Response for session revocation
#[derive(Debug, Serialize)]
pub struct RevokeSessionsResponse {
    pub user_id: Uuid,
    pub sessions_revoked: i64,
    pub message: String,
}

/// Revoke all sessions for a user
pub async fn revoke_user_sessions(
    State(state): State<AppState>,
    Extension(auth_user): Extension<AuthUser>,
    headers: HeaderMap,
    Path(user_id): Path<Uuid>,
) -> ApiResult<Json<RevokeSessionsResponse>> {
    require_platform_admin(&state, &auth_user, true).await?;

    let admin_user_id = auth_user.user_id.ok_or(ApiError::Unauthorized)?;
    let (ip_address, user_agent, session_id) = extract_audit_context(&headers, &auth_user);

    // Verify user exists
    let user_exists: Option<(bool,)> =
        sqlx::query_as("SELECT TRUE FROM public.users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(&state.pool)
            .await?;

    if user_exists.is_none() {
        return Err(ApiError::NotFound);
    }

    // Revoke all sessions
    let result = sqlx::query(
        r#"
        UPDATE sessions
        SET revoked_at = NOW()
        WHERE user_id = $1 AND revoked_at IS NULL
        "#,
    )
    .bind(user_id)
    .execute(&state.pool)
    .await?;

    let sessions_revoked = result.rows_affected() as i64;

    log_admin_action(
        &state.pool,
        admin_user_id,
        admin_action::USER_SESSIONS_REVOKED,
        target_type::USER,
        Some(user_id),
        Some(serde_json::json!({
            "sessions_revoked": sessions_revoked
        })),
        event_type::USER_MANAGEMENT,
        severity::CRITICAL,
        ip_address,
        user_agent,
        session_id,
    )
    .await?;

    Ok(Json(RevokeSessionsResponse {
        user_id,
        sessions_revoked,
        message: format!("Revoked {} sessions", sessions_revoked),
    }))
}

/// Response for password reset
#[derive(Debug, Serialize)]
pub struct ForcePasswordResetResponse {
    pub user_id: Uuid,
    pub sessions_revoked: i64,
    pub message: String,
}

/// Force password reset for a user (revokes sessions, sets flag for next login)
pub async fn force_password_reset(
    State(state): State<AppState>,
    Extension(auth_user): Extension<AuthUser>,
    headers: HeaderMap,
    Path(user_id): Path<Uuid>,
) -> ApiResult<Json<ForcePasswordResetResponse>> {
    require_platform_admin(&state, &auth_user, true).await?;

    let admin_user_id = auth_user.user_id.ok_or(ApiError::Unauthorized)?;
    let (ip_address, user_agent, session_id) = extract_audit_context(&headers, &auth_user);

    // Verify user exists
    let user_exists: Option<(bool,)> =
        sqlx::query_as("SELECT TRUE FROM public.users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(&state.pool)
            .await?;

    if user_exists.is_none() {
        return Err(ApiError::NotFound);
    }

    // Clear password hash to force reset on next login
    sqlx::query(
        r#"
        UPDATE users
        SET password_hash = NULL, password_changed_at = NULL, updated_at = NOW()
        WHERE id = $1
        "#,
    )
    .bind(user_id)
    .execute(&state.pool)
    .await?;

    // Revoke all sessions
    let result = sqlx::query(
        r#"
        UPDATE sessions
        SET revoked_at = NOW()
        WHERE user_id = $1 AND revoked_at IS NULL
        "#,
    )
    .bind(user_id)
    .execute(&state.pool)
    .await?;

    let sessions_revoked = result.rows_affected() as i64;

    log_admin_action(
        &state.pool,
        admin_user_id,
        admin_action::USER_FORCE_PASSWORD_RESET,
        target_type::USER,
        Some(user_id),
        Some(serde_json::json!({
            "sessions_revoked": sessions_revoked
        })),
        event_type::USER_MANAGEMENT,
        severity::CRITICAL,
        ip_address,
        user_agent,
        session_id,
    )
    .await?;

    Ok(Json(ForcePasswordResetResponse {
        user_id,
        sessions_revoked,
        message: "Password reset required. User must set new password on next login.".to_string(),
    }))
}

/// Response for 2FA disable
#[derive(Debug, Serialize)]
pub struct Disable2FAResponse {
    pub user_id: Uuid,
    pub backup_codes_deleted: i64,
    pub trusted_devices_deleted: i64,
    pub message: String,
}

/// Disable 2FA for a user (admin override)
pub async fn disable_user_2fa(
    State(state): State<AppState>,
    Extension(auth_user): Extension<AuthUser>,
    headers: HeaderMap,
    Path(user_id): Path<Uuid>,
) -> ApiResult<Json<Disable2FAResponse>> {
    require_platform_admin(&state, &auth_user, true).await?;

    let admin_user_id = auth_user.user_id.ok_or(ApiError::Unauthorized)?;
    let (ip_address, user_agent, session_id) = extract_audit_context(&headers, &auth_user);

    // Verify user exists
    let user_exists: Option<(bool,)> =
        sqlx::query_as("SELECT TRUE FROM public.users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(&state.pool)
            .await?;

    if user_exists.is_none() {
        return Err(ApiError::NotFound);
    }

    // Disable 2FA
    sqlx::query(
        r#"
        UPDATE user_2fa
        SET is_enabled = FALSE, secret_key = NULL, updated_at = NOW()
        WHERE user_id = $1
        "#,
    )
    .bind(user_id)
    .execute(&state.pool)
    .await?;

    // Delete backup codes
    let backup_result = sqlx::query("DELETE FROM user_2fa_backup_codes WHERE user_id = $1")
        .bind(user_id)
        .execute(&state.pool)
        .await?;

    let backup_codes_deleted = backup_result.rows_affected() as i64;

    // Delete trusted devices
    let devices_result = sqlx::query("DELETE FROM user_trusted_devices WHERE user_id = $1")
        .bind(user_id)
        .execute(&state.pool)
        .await?;

    let trusted_devices_deleted = devices_result.rows_affected() as i64;

    log_admin_action(
        &state.pool,
        admin_user_id,
        admin_action::USER_2FA_DISABLED,
        target_type::USER,
        Some(user_id),
        Some(serde_json::json!({
            "backup_codes_deleted": backup_codes_deleted,
            "trusted_devices_deleted": trusted_devices_deleted
        })),
        event_type::USER_MANAGEMENT,
        severity::CRITICAL,
        ip_address,
        user_agent,
        session_id,
    )
    .await?;

    Ok(Json(Disable2FAResponse {
        user_id,
        backup_codes_deleted,
        trusted_devices_deleted,
        message: "Two-factor authentication disabled".to_string(),
    }))
}

/// Request for suspending a user
#[derive(Debug, Deserialize)]
pub struct SuspendUserRequest {
    pub reason: Option<String>,
}

/// Response for suspension actions
#[derive(Debug, Serialize)]
pub struct SuspendUserResponse {
    pub user_id: Uuid,
    pub is_suspended: bool,
    pub sessions_revoked: i64,
    pub message: String,
}

/// Suspend a user account
pub async fn suspend_user(
    State(state): State<AppState>,
    Extension(auth_user): Extension<AuthUser>,
    headers: HeaderMap,
    Path(user_id): Path<Uuid>,
    Json(req): Json<SuspendUserRequest>,
) -> ApiResult<Json<SuspendUserResponse>> {
    require_platform_admin(&state, &auth_user, true).await?;

    let admin_user_id = auth_user.user_id.ok_or(ApiError::Unauthorized)?;
    let (ip_address, user_agent, session_id) = extract_audit_context(&headers, &auth_user);

    // Verify user exists and get their role
    let target_role: Option<PlatformRoleRow> = sqlx::query_as(
        "SELECT platform_role::TEXT as platform_role FROM public.users WHERE id = $1",
    )
    .bind(user_id)
    .fetch_optional(&state.pool)
    .await?;

    if target_role.is_none() {
        return Err(ApiError::NotFound);
    }

    // Prevent suspending superadmins unless you're a superadmin
    if target_role.map(|r| r.platform_role).unwrap_or_default() == "superadmin" {
        let admin_role: Option<PlatformRoleRow> = sqlx::query_as(
            "SELECT platform_role::TEXT as platform_role FROM public.users WHERE id = $1",
        )
        .bind(admin_user_id)
        .fetch_optional(&state.pool)
        .await?;

        if admin_role.map(|r| r.platform_role).unwrap_or_default() != "superadmin" {
            return Err(ApiError::Forbidden);
        }
    }

    // Prevent self-suspension
    if user_id == admin_user_id {
        return Err(ApiError::Validation(
            "Cannot suspend your own account".to_string(),
        ));
    }

    // Suspend the user
    sqlx::query(
        r#"
        UPDATE users
        SET is_suspended = TRUE, suspended_at = NOW(), suspended_reason = $2, updated_at = NOW()
        WHERE id = $1
        "#,
    )
    .bind(user_id)
    .bind(&req.reason)
    .execute(&state.pool)
    .await?;

    // Revoke all sessions
    let result = sqlx::query(
        r#"
        UPDATE sessions
        SET revoked_at = NOW()
        WHERE user_id = $1 AND revoked_at IS NULL
        "#,
    )
    .bind(user_id)
    .execute(&state.pool)
    .await?;

    let sessions_revoked = result.rows_affected() as i64;

    log_admin_action(
        &state.pool,
        admin_user_id,
        admin_action::USER_SUSPENDED,
        target_type::USER,
        Some(user_id),
        Some(serde_json::json!({
            "reason": req.reason,
            "sessions_revoked": sessions_revoked
        })),
        event_type::USER_MANAGEMENT,
        severity::CRITICAL,
        ip_address,
        user_agent,
        session_id,
    )
    .await?;

    Ok(Json(SuspendUserResponse {
        user_id,
        is_suspended: true,
        sessions_revoked,
        message: "User account suspended".to_string(),
    }))
}

/// Unsuspend a user account
pub async fn unsuspend_user(
    State(state): State<AppState>,
    Extension(auth_user): Extension<AuthUser>,
    headers: HeaderMap,
    Path(user_id): Path<Uuid>,
) -> ApiResult<Json<SuspendUserResponse>> {
    require_platform_admin(&state, &auth_user, true).await?;

    let admin_user_id = auth_user.user_id.ok_or(ApiError::Unauthorized)?;
    let (ip_address, user_agent, session_id) = extract_audit_context(&headers, &auth_user);

    // Verify user exists
    let user_exists: Option<(bool,)> =
        sqlx::query_as("SELECT TRUE FROM public.users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(&state.pool)
            .await?;

    if user_exists.is_none() {
        return Err(ApiError::NotFound);
    }

    // Unsuspend the user
    sqlx::query(
        r#"
        UPDATE users
        SET is_suspended = FALSE, suspended_at = NULL, suspended_reason = NULL, updated_at = NOW()
        WHERE id = $1
        "#,
    )
    .bind(user_id)
    .execute(&state.pool)
    .await?;

    log_admin_action(
        &state.pool,
        admin_user_id,
        admin_action::USER_UNSUSPENDED,
        target_type::USER,
        Some(user_id),
        None,
        event_type::USER_MANAGEMENT,
        severity::CRITICAL,
        ip_address,
        user_agent,
        session_id,
    )
    .await?;

    Ok(Json(SuspendUserResponse {
        user_id,
        is_suspended: false,
        sessions_revoked: 0,
        message: "User account unsuspended".to_string(),
    }))
}

/// Response for user deletion
#[derive(Debug, Serialize)]
pub struct DeleteUserResponse {
    pub user_id: Uuid,
    pub message: String,
}

/// Delete a user account
pub async fn delete_user(
    State(state): State<AppState>,
    Extension(auth_user): Extension<AuthUser>,
    headers: HeaderMap,
    Path(user_id): Path<Uuid>,
) -> ApiResult<Json<DeleteUserResponse>> {
    require_platform_admin(&state, &auth_user, true).await?;

    let admin_user_id = auth_user.user_id.ok_or(ApiError::Unauthorized)?;
    let (ip_address, user_agent, session_id) = extract_audit_context(&headers, &auth_user);

    // Verify user exists and get their role
    let target_role: Option<PlatformRoleRow> = sqlx::query_as(
        "SELECT platform_role::TEXT as platform_role FROM public.users WHERE id = $1",
    )
    .bind(user_id)
    .fetch_optional(&state.pool)
    .await?;

    if target_role.is_none() {
        return Err(ApiError::NotFound);
    }

    // Prevent deleting superadmins unless you're a superadmin
    if target_role.map(|r| r.platform_role).unwrap_or_default() == "superadmin" {
        let admin_role: Option<PlatformRoleRow> = sqlx::query_as(
            "SELECT platform_role::TEXT as platform_role FROM public.users WHERE id = $1",
        )
        .bind(admin_user_id)
        .fetch_optional(&state.pool)
        .await?;

        if admin_role.map(|r| r.platform_role).unwrap_or_default() != "superadmin" {
            return Err(ApiError::Forbidden);
        }
    }

    // Prevent self-deletion
    if user_id == admin_user_id {
        return Err(ApiError::Validation(
            "Cannot delete your own account".to_string(),
        ));
    }

    // Log before deletion
    log_admin_action(
        &state.pool,
        admin_user_id,
        admin_action::USER_DELETED,
        target_type::USER,
        Some(user_id),
        None,
        event_type::USER_MANAGEMENT,
        severity::CRITICAL,
        ip_address,
        user_agent,
        session_id,
    )
    .await?;

    // Delete user (cascades to related tables via ON DELETE CASCADE)
    sqlx::query("DELETE FROM public.users WHERE id = $1")
        .bind(user_id)
        .execute(&state.pool)
        .await?;

    Ok(Json(DeleteUserResponse {
        user_id,
        message: "User account deleted permanently".to_string(),
    }))
}

/// Response for API key revocation
#[derive(Debug, Serialize)]
pub struct RevokeApiKeyResponse {
    pub key_id: Uuid,
    pub message: String,
}

/// Revoke a specific API key for a user
pub async fn revoke_user_api_key(
    State(state): State<AppState>,
    Extension(auth_user): Extension<AuthUser>,
    headers: HeaderMap,
    Path((user_id, key_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<RevokeApiKeyResponse>> {
    require_platform_admin(&state, &auth_user, true).await?;

    let admin_user_id = auth_user.user_id.ok_or(ApiError::Unauthorized)?;
    let (ip_address, user_agent, session_id) = extract_audit_context(&headers, &auth_user);

    // Verify user exists and get their org_id
    let user_org: Option<(Uuid,)> = sqlx::query_as("SELECT org_id FROM public.users WHERE id = $1")
        .bind(user_id)
        .fetch_optional(&state.pool)
        .await?;

    let org_id = user_org.map(|(id,)| id).ok_or(ApiError::NotFound)?;

    // Verify API key belongs to user's org
    let key_exists: Option<(bool,)> =
        sqlx::query_as("SELECT TRUE FROM api_keys WHERE id = $1 AND org_id = $2")
            .bind(key_id)
            .bind(org_id)
            .fetch_optional(&state.pool)
            .await?;

    if key_exists.is_none() {
        return Err(ApiError::NotFound);
    }

    // Revoke the API key
    sqlx::query(
        r#"
        UPDATE api_keys
        SET status = 'revoked', updated_at = NOW()
        WHERE id = $1
        "#,
    )
    .bind(key_id)
    .execute(&state.pool)
    .await?;

    log_admin_action(
        &state.pool,
        admin_user_id,
        admin_action::API_KEY_REVOKED,
        target_type::API_KEY,
        Some(key_id),
        Some(serde_json::json!({
            "user_id": user_id,
            "org_id": org_id
        })),
        event_type::ADMIN_ACTION,
        severity::CRITICAL,
        ip_address,
        user_agent,
        session_id,
    )
    .await?;

    Ok(Json(RevokeApiKeyResponse {
        key_id,
        message: "API key revoked".to_string(),
    }))
}

// =============================================================================
// MCP Proxy Logs Endpoints
// =============================================================================

/// Query parameters for listing MCP proxy logs
#[derive(Debug, Deserialize)]
pub struct McpLogsQuery {
    /// Filter by MCP instance ID
    pub mcp_id: Option<Uuid>,
    /// Filter by organization ID
    pub org_id: Option<Uuid>,
    /// Filter by status: "success", "error", "timeout"
    pub status: Option<String>,
    /// Filter by JSON-RPC method
    pub method: Option<String>,
    /// Search query (searches tool_name, resource_uri, error_message)
    pub search: Option<String>,
    /// Start of date range (inclusive)
    pub from: Option<String>,
    /// End of date range (inclusive)
    pub to: Option<String>,
    /// Page number (1-based, defaults to 1)
    pub page: Option<i64>,
    /// Results per page (defaults to 50, max 100)
    pub per_page: Option<i64>,
}

/// Statistics for MCP proxy logs
#[derive(Debug, Serialize)]
pub struct McpLogStats {
    pub total_requests: i64,
    pub success_count: i64,
    pub error_count: i64,
    pub timeout_count: i64,
    pub success_rate: f64,
    pub avg_latency_ms: f64,
    pub p50_latency_ms: Option<i32>,
    pub p95_latency_ms: Option<i32>,
    pub p99_latency_ms: Option<i32>,
}

/// Response for MCP proxy logs listing
#[derive(Debug, Serialize)]
pub struct McpLogsResponse {
    pub logs: Vec<McpLogEntry>,
    pub stats: McpLogStats,
    pub total: i64,
    pub page: i64,
    pub per_page: i64,
    pub total_pages: i64,
}

/// Single MCP proxy log entry
#[derive(Debug, Serialize)]
pub struct McpLogEntry {
    pub id: Uuid,
    pub mcp_id: Option<Uuid>,
    pub mcp_name: Option<String>,
    pub org_id: Option<Uuid>,
    pub org_name: Option<String>,
    pub api_key_name: Option<String>,
    pub method: String,
    pub tool_name: Option<String>,
    pub resource_uri: Option<String>,
    pub status: String,
    pub latency_ms: Option<i32>,
    pub error_message: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

/// Database row for MCP proxy log with joined data
#[derive(Debug, FromRow)]
struct McpLogRow {
    id: Uuid,
    mcp_id: Option<Uuid>,
    mcp_name: Option<String>,
    org_id: Option<Uuid>,
    org_name: Option<String>,
    api_key_name: Option<String>,
    method: String,
    tool_name: Option<String>,
    resource_uri: Option<String>,
    status: String,
    latency_ms: Option<i32>,
    error_message: Option<String>,
    created_at: OffsetDateTime,
}

/// Get MCP proxy logs with filtering and pagination
///
/// This endpoint requires platform admin privileges (admin, superadmin, or staff for read).
/// It returns logs from the mcp_proxy_logs table with joined data from api_keys,
/// mcp_instances, and organizations tables.
pub async fn get_mcp_logs(
    State(state): State<AppState>,
    Extension(auth_user): Extension<AuthUser>,
    Query(query): Query<McpLogsQuery>,
) -> ApiResult<Json<McpLogsResponse>> {
    require_platform_admin(&state, &auth_user, false).await?;

    let page = query.page.unwrap_or(1).max(1);
    let per_page = query.per_page.unwrap_or(50).clamp(1, 100);
    let offset = (page - 1) * per_page;

    // Validate status filter if provided
    if let Some(ref status) = query.status {
        let valid_statuses = ["success", "error", "timeout"];
        if !valid_statuses.contains(&status.as_str()) {
            return Err(ApiError::Validation(format!(
                "Invalid status. Must be one of: {}",
                valid_statuses.join(", ")
            )));
        }
    }

    // Parse date filters if provided
    let from_date = if let Some(ref from_str) = query.from {
        Some(
            OffsetDateTime::parse(from_str, &time::format_description::well_known::Rfc3339)
                .map_err(|_| {
                    ApiError::Validation(
                        "Invalid 'from' date format. Use RFC3339 (e.g., 2024-01-01T00:00:00Z)"
                            .to_string(),
                    )
                })?,
        )
    } else {
        None
    };

    let to_date = if let Some(ref to_str) = query.to {
        Some(
            OffsetDateTime::parse(to_str, &time::format_description::well_known::Rfc3339).map_err(
                |_| {
                    ApiError::Validation(
                        "Invalid 'to' date format. Use RFC3339 (e.g., 2024-01-31T23:59:59Z)"
                            .to_string(),
                    )
                },
            )?,
        )
    } else {
        None
    };

    // Build the query dynamically based on filters
    // We use a single optimized query with LEFT JOINs to avoid N+1
    let base_query = r#"
        SELECT
            l.id,
            l.mcp_id,
            m.name as mcp_name,
            o.id as org_id,
            o.name as org_name,
            k.name as api_key_name,
            l.method,
            l.tool_name,
            l.resource_uri,
            l.status,
            l.latency_ms,
            l.error_message,
            l.created_at
        FROM mcp_proxy_logs l
        LEFT JOIN api_keys k ON k.id = l.api_key_id
        LEFT JOIN organizations o ON o.id = k.org_id
        LEFT JOIN mcp_instances m ON m.id = l.mcp_id
        WHERE 1=1
    "#;

    let count_query = r#"
        SELECT COUNT(*)
        FROM mcp_proxy_logs l
        LEFT JOIN api_keys k ON k.id = l.api_key_id
        LEFT JOIN organizations o ON o.id = k.org_id
        LEFT JOIN mcp_instances m ON m.id = l.mcp_id
        WHERE 1=1
    "#;

    // Build filter conditions
    let mut conditions = String::new();
    let mut param_index = 1;

    if query.mcp_id.is_some() {
        conditions.push_str(&format!(" AND l.mcp_id = ${}", param_index));
        param_index += 1;
    }

    if query.org_id.is_some() {
        conditions.push_str(&format!(" AND o.id = ${}", param_index));
        param_index += 1;
    }

    if query.status.is_some() {
        conditions.push_str(&format!(" AND l.status = ${}", param_index));
        param_index += 1;
    }

    if query.method.is_some() {
        conditions.push_str(&format!(" AND l.method = ${}", param_index));
        param_index += 1;
    }

    if query.search.is_some() {
        conditions.push_str(&format!(
            " AND (l.tool_name ILIKE '%' || ${} || '%' OR l.resource_uri ILIKE '%' || ${} || '%' OR l.error_message ILIKE '%' || ${} || '%')",
            param_index, param_index, param_index
        ));
        param_index += 1;
    }

    if from_date.is_some() {
        conditions.push_str(&format!(" AND l.created_at >= ${}", param_index));
        param_index += 1;
    }

    if to_date.is_some() {
        conditions.push_str(&format!(" AND l.created_at <= ${}", param_index));
        param_index += 1;
    }

    // Build final queries
    let data_sql = format!(
        "{}{} ORDER BY l.created_at DESC LIMIT ${} OFFSET ${}",
        base_query,
        conditions,
        param_index,
        param_index + 1
    );

    let count_sql = format!("{}{}", count_query, conditions);

    // Execute queries with dynamic binding
    // We need to build the query dynamically based on which filters are present
    let logs: Vec<McpLogRow> = {
        let mut qb = sqlx::query_as::<_, McpLogRow>(&data_sql);

        if let Some(ref mcp_id) = query.mcp_id {
            qb = qb.bind(mcp_id);
        }
        if let Some(ref org_id) = query.org_id {
            qb = qb.bind(org_id);
        }
        if let Some(ref status) = query.status {
            qb = qb.bind(status);
        }
        if let Some(ref method) = query.method {
            qb = qb.bind(method);
        }
        if let Some(ref search) = query.search {
            qb = qb.bind(search);
        }
        if let Some(ref from) = from_date {
            qb = qb.bind(from);
        }
        if let Some(ref to) = to_date {
            qb = qb.bind(to);
        }

        qb = qb.bind(per_page);
        qb = qb.bind(offset);

        qb.fetch_all(&state.pool).await?
    };

    // Get total count with same filters
    let total: (i64,) = {
        let mut qb = sqlx::query_as::<_, (i64,)>(&count_sql);

        if let Some(ref mcp_id) = query.mcp_id {
            qb = qb.bind(mcp_id);
        }
        if let Some(ref org_id) = query.org_id {
            qb = qb.bind(org_id);
        }
        if let Some(ref status) = query.status {
            qb = qb.bind(status);
        }
        if let Some(ref method) = query.method {
            qb = qb.bind(method);
        }
        if let Some(ref search) = query.search {
            qb = qb.bind(search);
        }
        if let Some(ref from) = from_date {
            qb = qb.bind(from);
        }
        if let Some(ref to) = to_date {
            qb = qb.bind(to);
        }

        qb.fetch_one(&state.pool).await?
    };

    // Convert rows to response entries
    let log_entries: Vec<McpLogEntry> = logs
        .into_iter()
        .map(|row| McpLogEntry {
            id: row.id,
            mcp_id: row.mcp_id,
            mcp_name: row.mcp_name,
            org_id: row.org_id,
            org_name: row.org_name,
            api_key_name: row.api_key_name,
            method: row.method,
            tool_name: row.tool_name,
            resource_uri: row.resource_uri,
            status: row.status,
            latency_ms: row.latency_ms,
            error_message: row.error_message,
            created_at: row.created_at,
        })
        .collect();

    // Build stats query with same filters
    let stats_sql = format!(
        r#"
        SELECT
            COUNT(*) as total_requests,
            COALESCE(SUM(CASE WHEN l.status = 'success' THEN 1 ELSE 0 END), 0) as success_count,
            COALESCE(SUM(CASE WHEN l.status = 'error' THEN 1 ELSE 0 END), 0) as error_count,
            COALESCE(SUM(CASE WHEN l.status = 'timeout' THEN 1 ELSE 0 END), 0) as timeout_count,
            COALESCE(AVG(l.latency_ms::float8), 0.0) as avg_latency_ms,
            PERCENTILE_CONT(0.5) WITHIN GROUP (ORDER BY l.latency_ms) as p50_latency_ms,
            PERCENTILE_CONT(0.95) WITHIN GROUP (ORDER BY l.latency_ms) as p95_latency_ms,
            PERCENTILE_CONT(0.99) WITHIN GROUP (ORDER BY l.latency_ms) as p99_latency_ms
        FROM mcp_proxy_logs l
        LEFT JOIN api_keys k ON k.id = l.api_key_id
        LEFT JOIN organizations o ON o.id = k.org_id
        LEFT JOIN mcp_instances m ON m.id = l.mcp_id
        WHERE 1=1 {}
        "#,
        conditions
    );

    #[derive(sqlx::FromRow)]
    struct StatsRow {
        total_requests: i64,
        success_count: i64,
        error_count: i64,
        timeout_count: i64,
        avg_latency_ms: f64,
        p50_latency_ms: Option<f64>,
        p95_latency_ms: Option<f64>,
        p99_latency_ms: Option<f64>,
    }

    let stats_row: StatsRow = {
        let mut qb = sqlx::query_as::<_, StatsRow>(&stats_sql);

        if let Some(ref mcp_id) = query.mcp_id {
            qb = qb.bind(mcp_id);
        }
        if let Some(ref org_id) = query.org_id {
            qb = qb.bind(org_id);
        }
        if let Some(ref status) = query.status {
            qb = qb.bind(status);
        }
        if let Some(ref method) = query.method {
            qb = qb.bind(method);
        }
        if let Some(ref search) = query.search {
            qb = qb.bind(search);
        }
        if let Some(ref from) = from_date {
            qb = qb.bind(from);
        }
        if let Some(ref to) = to_date {
            qb = qb.bind(to);
        }

        qb.fetch_one(&state.pool).await?
    };

    let success_rate = if stats_row.total_requests > 0 {
        (stats_row.success_count as f64 / stats_row.total_requests as f64) * 100.0
    } else {
        0.0
    };

    let stats = McpLogStats {
        total_requests: stats_row.total_requests,
        success_count: stats_row.success_count,
        error_count: stats_row.error_count,
        timeout_count: stats_row.timeout_count,
        success_rate,
        avg_latency_ms: stats_row.avg_latency_ms,
        p50_latency_ms: stats_row.p50_latency_ms.map(|v| v as i32),
        p95_latency_ms: stats_row.p95_latency_ms.map(|v| v as i32),
        p99_latency_ms: stats_row.p99_latency_ms.map(|v| v as i32),
    };

    // Calculate total_pages
    let total_pages = (total.0 + per_page - 1) / per_page;

    Ok(Json(McpLogsResponse {
        logs: log_entries,
        stats,
        total: total.0,
        page,
        per_page,
        total_pages,
    }))
}

// =============================================================================
// Organization Overages Management
// =============================================================================

/// Request to toggle overages for an organization
#[derive(Debug, Deserialize)]
pub struct ToggleOveragesRequest {
    /// Whether to disable overages (true = block at limit, false = allow overage billing)
    pub disable_overages: bool,
    /// Optional reason for disabling overages
    pub reason: Option<String>,
}

/// Response from toggle overages
#[derive(Debug, Serialize)]
pub struct ToggleOveragesResponse {
    pub org_id: Uuid,
    pub org_name: String,
    pub overages_disabled: bool,
    pub subscription_tier: String,
    pub reason: Option<String>,
    pub updated_at: String,
    pub updated_by: Option<Uuid>,
}

/// Toggle overages for an organization (admin only)
///
/// When overages are disabled:
/// - Free tier: Always disabled (cannot be changed)
/// - Pro/Team/Enterprise: Normally enabled, but admin can disable
///
/// When disabled, users hitting their limit will see upgrade messages
/// instead of accruing overage charges.
pub async fn toggle_org_overages(
    State(state): State<AppState>,
    Extension(auth_user): Extension<AuthUser>,
    headers: HeaderMap,
    Path(org_id): Path<Uuid>,
    Json(req): Json<ToggleOveragesRequest>,
) -> ApiResult<Json<ToggleOveragesResponse>> {
    require_platform_admin(&state, &auth_user, true).await?;

    let admin_user_id = auth_user.user_id.ok_or(ApiError::Unauthorized)?;
    let (ip_address, user_agent, session_id) = extract_audit_context(&headers, &auth_user);

    // Verify org exists and get current state
    #[derive(sqlx::FromRow)]
    struct OrgInfo {
        name: String,
        subscription_tier: String,
        overages_disabled: bool,
    }

    let org: Option<OrgInfo> = sqlx::query_as(
        r#"
        SELECT name, subscription_tier, COALESCE(overages_disabled, false) as overages_disabled
        FROM organizations
        WHERE id = $1
        "#,
    )
    .bind(org_id)
    .fetch_optional(&state.pool)
    .await?;

    let org = org.ok_or(ApiError::NotFound)?;

    // Free tier always has overages disabled - can't be changed
    if org.subscription_tier == "free" && !req.disable_overages {
        return Err(ApiError::Validation(
            "Free tier organizations cannot enable overage billing".to_string(),
        ));
    }

    let old_value = org.overages_disabled;
    let new_value = req.disable_overages;

    // Update organization
    sqlx::query(
        r#"
        UPDATE organizations SET
            overages_disabled = $2,
            overages_disabled_at = CASE WHEN $2 THEN NOW() ELSE NULL END,
            overages_disabled_by = CASE WHEN $2 THEN $3 ELSE NULL END,
            overages_disabled_reason = $4,
            updated_at = NOW()
        WHERE id = $1
        "#,
    )
    .bind(org_id)
    .bind(new_value)
    .bind(admin_user_id)
    .bind(&req.reason)
    .execute(&state.pool)
    .await?;

    // Log to audit trail
    let metadata = serde_json::json!({
        "org_id": org_id,
        "org_name": org.name,
        "subscription_tier": org.subscription_tier,
        "old_value": old_value,
        "new_value": new_value,
        "reason": req.reason,
    });

    let event_type = if new_value {
        "admin_overages_disabled"
    } else {
        "admin_overages_enabled"
    };

    sqlx::query(
        r#"
        INSERT INTO auth_audit_log (
            user_id, email, event_type, severity,
            ip_address, user_agent, session_id, metadata, auth_method
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        "#,
    )
    .bind(admin_user_id)
    .bind(&auth_user.email)
    .bind(event_type)
    .bind("info")
    .bind(&ip_address)
    .bind(&user_agent)
    .bind(&session_id)
    .bind(&metadata)
    .bind("session")
    .execute(&state.pool)
    .await
    .ok(); // Best effort audit logging

    tracing::info!(
        admin_id = %admin_user_id,
        org_id = %org_id,
        org_name = %org.name,
        old_value = %old_value,
        new_value = %new_value,
        "Toggled overages for organization"
    );

    Ok(Json(ToggleOveragesResponse {
        org_id,
        org_name: org.name,
        overages_disabled: new_value,
        subscription_tier: org.subscription_tier,
        reason: req.reason,
        updated_at: time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default(),
        updated_by: Some(admin_user_id),
    }))
}

/// Get overages status for an organization
pub async fn get_org_overages(
    State(state): State<AppState>,
    Extension(auth_user): Extension<AuthUser>,
    Path(org_id): Path<Uuid>,
) -> ApiResult<Json<ToggleOveragesResponse>> {
    require_platform_admin(&state, &auth_user, false).await?;

    #[derive(sqlx::FromRow)]
    struct OrgInfo {
        name: String,
        subscription_tier: String,
        overages_disabled: bool,
        overages_disabled_at: Option<time::OffsetDateTime>,
        overages_disabled_by: Option<Uuid>,
        overages_disabled_reason: Option<String>,
    }

    let org: Option<OrgInfo> = sqlx::query_as(
        r#"
        SELECT name, subscription_tier,
               COALESCE(overages_disabled, false) as overages_disabled,
               overages_disabled_at, overages_disabled_by, overages_disabled_reason
        FROM organizations
        WHERE id = $1
        "#,
    )
    .bind(org_id)
    .fetch_optional(&state.pool)
    .await?;

    let org = org.ok_or(ApiError::NotFound)?;

    // For Free tier, overages are always considered disabled
    let effective_disabled = org.subscription_tier == "free" || org.overages_disabled;

    Ok(Json(ToggleOveragesResponse {
        org_id,
        org_name: org.name,
        overages_disabled: effective_disabled,
        subscription_tier: org.subscription_tier,
        reason: org.overages_disabled_reason,
        updated_at: org
            .overages_disabled_at
            .map(|t| {
                t.format(&time::format_description::well_known::Rfc3339)
                    .unwrap_or_default()
            })
            .unwrap_or_default(),
        updated_by: org.overages_disabled_by,
    }))
}

// =============================================================================
// Billing Diagnostics Routes (requires billing feature)
// =============================================================================

/// Response for billing invariant checks
#[cfg(feature = "billing")]
#[derive(Debug, Serialize)]
pub struct BillingInvariantsResponse {
    pub healthy: bool,
    pub checks_run: usize,
    pub checks_passed: usize,
    pub checks_failed: usize,
    pub violations: Vec<InvariantViolationResponse>,
    #[serde(with = "time::serde::rfc3339")]
    pub checked_at: OffsetDateTime,
}

#[cfg(feature = "billing")]
#[derive(Debug, Serialize)]
pub struct InvariantViolationResponse {
    pub invariant: String,
    pub severity: String,
    pub description: String,
    pub org_ids: Vec<Uuid>,
    pub context: serde_json::Value,
}

/// Run all billing invariant checks
///
/// Returns the health status of the billing system by checking:
/// - At most 1 active subscription per org
/// - Tier matches subscription price
/// - Canceled subscriptions have period_end
/// - All tier changes have audit records
/// - Spend cap consistency
/// - Stripe customer exists for paid tiers
#[cfg(feature = "billing")]
pub async fn check_billing_invariants(
    State(state): State<AppState>,
    Extension(auth_user): Extension<AuthUser>,
    headers: HeaderMap,
) -> ApiResult<Json<BillingInvariantsResponse>> {
    // Require admin or superadmin role (read-only, so staff can access too)
    let admin_user_id = require_platform_admin(&state, &auth_user, false).await?;

    let req_id = Uuid::new_v4();
    let client_ip = extract_client_ip(&headers);

    tracing::info!(
        %req_id,
        admin_id = %admin_user_id,
        client_ip = ?client_ip,
        "Admin checking billing invariants"
    );

    let checker = plexmcp_billing::InvariantChecker::new(state.pool.clone());
    let summary = checker.run_all_checks().await.map_err(|e| {
        tracing::error!(%req_id, error = %e, "Failed to run invariant checks");
        ApiError::Internal
    })?;

    let violations: Vec<InvariantViolationResponse> = summary
        .violations
        .into_iter()
        .map(|v| InvariantViolationResponse {
            invariant: v.invariant,
            severity: v.severity.to_string(),
            description: v.description,
            org_ids: v.org_ids,
            context: v.context,
        })
        .collect();

    Ok(Json(BillingInvariantsResponse {
        healthy: summary.healthy,
        checks_run: summary.checks_run,
        checks_passed: summary.checks_passed,
        checks_failed: summary.checks_failed,
        violations,
        checked_at: summary.checked_at,
    }))
}

/// Response for billing debug endpoint
#[cfg(feature = "billing")]
#[derive(Debug, Serialize)]
pub struct BillingDebugResponse {
    pub org_id: Uuid,
    pub org_name: String,
    pub entitlement: EntitlementDebugInfo,
    pub subscription: Option<SubscriptionDebugInfo>,
    pub spend_cap: Option<SpendCapDebugInfo>,
    pub recent_events: Vec<BillingEventDebugInfo>,
    pub diagnostics: Vec<DiagnosticMessage>,
}

#[cfg(feature = "billing")]
#[derive(Debug, Serialize)]
pub struct EntitlementDebugInfo {
    pub state: String,
    pub tier: String,
    pub source: String,
    pub api_allowed: bool,
    pub api_blocked_reason: Option<String>,
    pub limits: EffectiveLimitsDebug,
    #[serde(with = "time::serde::rfc3339")]
    pub computed_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub expires_at: Option<OffsetDateTime>,
}

#[cfg(feature = "billing")]
#[derive(Debug, Serialize)]
pub struct EffectiveLimitsDebug {
    pub max_mcps: u32,
    pub max_api_keys: u32,
    pub max_team_members: u32,
    pub max_requests_monthly: u64,
}

#[cfg(feature = "billing")]
#[derive(Debug, Serialize)]
pub struct SubscriptionDebugInfo {
    pub stripe_subscription_id: String,
    pub status: String,
    pub stripe_price_id: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub trial_end: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub current_period_start: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub current_period_end: Option<OffsetDateTime>,
    pub cancel_at_period_end: bool,
}

#[cfg(feature = "billing")]
#[derive(Debug, Serialize)]
pub struct SpendCapDebugInfo {
    pub cap_amount_cents: i64,
    pub current_period_spend_cents: i64,
    pub is_paused: bool,
    #[serde(with = "time::serde::rfc3339::option")]
    pub paused_at: Option<OffsetDateTime>,
    pub spend_percentage: f64,
}

#[cfg(feature = "billing")]
#[derive(Debug, Serialize)]
pub struct BillingEventDebugInfo {
    pub id: Uuid,
    pub event_type: String,
    pub actor_type: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    pub event_data: serde_json::Value,
}

#[cfg(feature = "billing")]
#[derive(Debug, Serialize)]
pub struct DiagnosticMessage {
    pub level: String,
    pub message: String,
}

/// Debug billing state for an organization
///
/// Returns comprehensive debugging information including:
/// - Current entitlement state and source
/// - Subscription details
/// - Spend cap status
/// - Recent billing events
/// - Diagnostic messages explaining the current state
#[cfg(feature = "billing")]
pub async fn debug_org_billing(
    State(state): State<AppState>,
    Extension(auth_user): Extension<AuthUser>,
    Path(org_id): Path<Uuid>,
    headers: HeaderMap,
) -> ApiResult<Json<BillingDebugResponse>> {
    // Require admin or superadmin role (read-only, so staff can access too)
    let admin_user_id = require_platform_admin(&state, &auth_user, false).await?;

    let req_id = Uuid::new_v4();
    let client_ip = extract_client_ip(&headers);

    tracing::info!(
        %req_id,
        admin_id = %admin_user_id,
        %org_id,
        client_ip = ?client_ip,
        "Admin debugging org billing"
    );

    // Get org basic info
    let org: Option<OrgBasicInfo> = sqlx::query_as(
        r#"
        SELECT id, name, subscription_tier, stripe_customer_id
        FROM organizations
        WHERE id = $1
        "#,
    )
    .bind(org_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| {
        log_db_err(req_id, "fetch_org", &e);
        ApiError::Database(e.to_string())
    })?;

    let org = org.ok_or(ApiError::NotFound)?;

    // Get entitlement
    let entitlement_service = plexmcp_billing::EntitlementService::new(state.pool.clone());
    let entitlement = entitlement_service
        .compute_entitlement(org_id)
        .await
        .map_err(|e| {
            tracing::error!(%req_id, error = %e, "Failed to compute entitlement");
            ApiError::Internal
        })?;

    // Get subscription info
    let subscription: Option<SubscriptionRow> = sqlx::query_as(
        r#"
        SELECT
            stripe_subscription_id,
            status,
            stripe_price_id,
            trial_end,
            current_period_start,
            current_period_end,
            COALESCE(cancel_at_period_end, false) as cancel_at_period_end
        FROM subscriptions
        WHERE org_id = $1
        ORDER BY created_at DESC
        LIMIT 1
        "#,
    )
    .bind(org_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| {
        log_db_err(req_id, "fetch_subscription", &e);
        ApiError::Database(e.to_string())
    })?;

    // Get spend cap info
    let spend_cap: Option<SpendCapRow> = sqlx::query_as(
        r#"
        SELECT
            cap_amount_cents,
            current_period_spend_cents,
            is_paused,
            paused_at
        FROM spend_caps
        WHERE org_id = $1
        "#,
    )
    .bind(org_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| {
        log_db_err(req_id, "fetch_spend_cap", &e);
        ApiError::Database(e.to_string())
    })?;

    // Get recent billing events (if table exists)
    let recent_events: Vec<BillingEventRow> = sqlx::query_as(
        r#"
        SELECT id, event_type, actor_type, created_at, event_data
        FROM billing_events
        WHERE org_id = $1
        ORDER BY created_at DESC
        LIMIT 10
        "#,
    )
    .bind(org_id)
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    // Build diagnostics
    let mut diagnostics = Vec::new();

    // Check entitlement state
    diagnostics.push(DiagnosticMessage {
        level: "info".to_string(),
        message: format!(
            "Entitlement state is {} (source: {})",
            entitlement.state,
            format!("{:?}", entitlement.source)
        ),
    });

    if !entitlement.api_allowed {
        diagnostics.push(DiagnosticMessage {
            level: "warning".to_string(),
            message: format!(
                "API access is blocked: {}",
                entitlement
                    .api_blocked_reason
                    .as_deref()
                    .unwrap_or("Unknown reason")
            ),
        });
    }

    // Check subscription status
    if let Some(ref sub) = subscription {
        if sub.status == "past_due" {
            diagnostics.push(DiagnosticMessage {
                level: "warning".to_string(),
                message: "Subscription is past due - payment required".to_string(),
            });
        }
        if sub.cancel_at_period_end {
            diagnostics.push(DiagnosticMessage {
                level: "info".to_string(),
                message: format!(
                    "Subscription scheduled to cancel at period end: {:?}",
                    sub.current_period_end
                ),
            });
        }
    } else if org.subscription_tier != "free" {
        diagnostics.push(DiagnosticMessage {
            level: "warning".to_string(),
            message: format!(
                "Organization is on {} tier but has no subscription record",
                org.subscription_tier
            ),
        });
    }

    // Check spend cap
    if let Some(ref cap) = spend_cap {
        if cap.is_paused {
            diagnostics.push(DiagnosticMessage {
                level: "warning".to_string(),
                message: format!(
                    "Organization is paused due to spend cap (${:.2} of ${:.2})",
                    cap.current_period_spend_cents as f64 / 100.0,
                    cap.cap_amount_cents as f64 / 100.0
                ),
            });
        }
    }

    // Build response
    let entitlement_debug = EntitlementDebugInfo {
        state: entitlement.state.to_string(),
        tier: format!("{:?}", entitlement.tier),
        source: format!("{:?}", entitlement.source),
        api_allowed: entitlement.api_allowed,
        api_blocked_reason: entitlement.api_blocked_reason,
        limits: EffectiveLimitsDebug {
            max_mcps: entitlement.limits.max_mcps,
            max_api_keys: entitlement.limits.max_api_keys,
            max_team_members: entitlement.limits.max_team_members,
            max_requests_monthly: entitlement.limits.max_requests_monthly,
        },
        computed_at: entitlement.computed_at,
        expires_at: entitlement.expires_at,
    };

    let subscription_debug = subscription.map(|s| SubscriptionDebugInfo {
        stripe_subscription_id: s.stripe_subscription_id,
        status: s.status,
        stripe_price_id: s.stripe_price_id,
        trial_end: s.trial_end,
        current_period_start: s.current_period_start,
        current_period_end: s.current_period_end,
        cancel_at_period_end: s.cancel_at_period_end,
    });

    let spend_cap_debug = spend_cap.map(|sc| {
        let percentage = if sc.cap_amount_cents > 0 {
            (sc.current_period_spend_cents as f64 / sc.cap_amount_cents as f64) * 100.0
        } else {
            0.0
        };
        SpendCapDebugInfo {
            cap_amount_cents: sc.cap_amount_cents,
            current_period_spend_cents: sc.current_period_spend_cents,
            is_paused: sc.is_paused,
            paused_at: sc.paused_at,
            spend_percentage: percentage,
        }
    });

    let events_debug: Vec<BillingEventDebugInfo> = recent_events
        .into_iter()
        .map(|e| BillingEventDebugInfo {
            id: e.id,
            event_type: e.event_type,
            actor_type: e.actor_type,
            created_at: e.created_at,
            event_data: e.event_data,
        })
        .collect();

    Ok(Json(BillingDebugResponse {
        org_id,
        org_name: org.name,
        entitlement: entitlement_debug,
        subscription: subscription_debug,
        spend_cap: spend_cap_debug,
        recent_events: events_debug,
        diagnostics,
    }))
}

// Helper row types for billing debug

#[derive(Debug, FromRow)]
#[allow(dead_code)]
struct OrgBasicInfo {
    id: Uuid,
    name: String,
    subscription_tier: String,
    stripe_customer_id: Option<String>,
}

#[derive(Debug, FromRow)]
struct SubscriptionRow {
    stripe_subscription_id: String,
    status: String,
    stripe_price_id: Option<String>,
    trial_end: Option<OffsetDateTime>,
    current_period_start: Option<OffsetDateTime>,
    current_period_end: Option<OffsetDateTime>,
    cancel_at_period_end: bool,
}

#[derive(Debug, FromRow)]
struct SpendCapRow {
    cap_amount_cents: i64,
    current_period_spend_cents: i64,
    is_paused: bool,
    paused_at: Option<OffsetDateTime>,
}

#[derive(Debug, FromRow)]
struct BillingEventRow {
    id: Uuid,
    event_type: String,
    actor_type: String,
    created_at: OffsetDateTime,
    event_data: serde_json::Value,
}
