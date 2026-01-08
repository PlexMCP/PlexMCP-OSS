//! User session management for JWT revocation
//!
//! Provides functions to create, validate, and revoke JWT sessions.
//! Sessions are tracked in the `user_sessions` table with JTI (JWT ID) for revocation support.

use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::ApiResult;

/// SOC 2 CC6.1: Maximum concurrent sessions per user
/// Prevents session accumulation and limits attack surface
const MAX_SESSIONS_PER_USER: i64 = 10;

/// Save a new session to the database
///
/// This should be called immediately after generating a JWT token pair.
/// The JTI values are stored to allow revoking tokens before expiration.
/// SOC 2 CC6.1: Enforces max session limit by revoking oldest sessions
#[allow(clippy::too_many_arguments)]
pub async fn save_session(
    pool: &PgPool,
    user_id: Uuid,
    access_jti: &str,
    access_expires_at: OffsetDateTime,
    refresh_jti: &str,
    refresh_expires_at: OffsetDateTime,
    ip_address: Option<&str>,
    user_agent: Option<&str>,
) -> ApiResult<()> {
    // Start a transaction to ensure both sessions are created atomically
    let mut tx = pool.begin().await?;

    // SOC 2 CC6.1: Enforce max sessions per user
    // Count active refresh token sessions (parent sessions)
    let session_count: (i64,) = sqlx::query_as(
        r#"
        SELECT COUNT(*)
        FROM user_sessions
        WHERE user_id = $1
          AND revoked_at IS NULL
          AND expires_at > NOW()
          AND token_type = 'refresh'
        "#,
    )
    .bind(user_id)
    .fetch_one(&mut *tx)
    .await?;

    // If at limit, revoke the oldest session
    if session_count.0 >= MAX_SESSIONS_PER_USER {
        tracing::info!(
            user_id = %user_id,
            current_sessions = session_count.0,
            max_sessions = MAX_SESSIONS_PER_USER,
            "Revoking oldest session due to max sessions limit"
        );

        sqlx::query(
            r#"
            UPDATE user_sessions
            SET revoked_at = NOW(),
                revocation_reason = 'max_sessions_exceeded'
            WHERE id IN (
                SELECT id FROM user_sessions
                WHERE user_id = $1
                  AND revoked_at IS NULL
                  AND token_type = 'refresh'
                ORDER BY created_at ASC
                LIMIT 1
            )
            "#,
        )
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    }

    // Create refresh token session (parent)
    let refresh_session_id: Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO user_sessions (
            user_id,
            jti,
            expires_at,
            ip_address,
            user_agent,
            token_type
        ) VALUES ($1, $2, $3, $4, $5, 'refresh')
        RETURNING id
        "#,
    )
    .bind(user_id)
    .bind(refresh_jti)
    .bind(refresh_expires_at)
    .bind(ip_address)
    .bind(user_agent)
    .fetch_one(&mut *tx)
    .await?;

    // Create access token session (child of refresh token)
    sqlx::query(
        r#"
        INSERT INTO user_sessions (
            user_id,
            jti,
            expires_at,
            ip_address,
            user_agent,
            token_type,
            parent_session_id
        ) VALUES ($1, $2, $3, $4, $5, 'access', $6)
        "#,
    )
    .bind(user_id)
    .bind(access_jti)
    .bind(access_expires_at)
    .bind(ip_address)
    .bind(user_agent)
    .bind(refresh_session_id)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    Ok(())
}

/// Check if a session (by JTI) is valid (not revoked and not expired)
///
/// This should be called by the authentication middleware for every request.
/// SOC 2 CC6.1: Includes ownership validation to prevent token misuse attacks
pub async fn is_session_valid(pool: &PgPool, jti: &str, expected_user_id: Uuid) -> ApiResult<bool> {
    let result: Option<(bool,)> = sqlx::query_as(
        r#"
        SELECT TRUE
        FROM user_sessions
        WHERE jti = $1
          AND user_id = $2
          AND revoked_at IS NULL
          AND expires_at > NOW()
        "#,
    )
    .bind(jti)
    .bind(expected_user_id)
    .fetch_optional(pool)
    .await?;

    Ok(result.is_some())
}

/// Revoke a specific session by JTI
///
/// Returns true if the session was found and revoked, false if not found.
pub async fn revoke_session(pool: &PgPool, jti: &str, reason: &str) -> ApiResult<bool> {
    let rows_affected = sqlx::query(
        r#"
        UPDATE user_sessions
        SET revoked_at = NOW(),
            revocation_reason = $2
        WHERE jti = $1
          AND revoked_at IS NULL
        "#,
    )
    .bind(jti)
    .bind(reason)
    .execute(pool)
    .await?
    .rows_affected();

    Ok(rows_affected > 0)
}

/// Revoke all sessions for a user
///
/// This should be called when:
/// - User changes their password
/// - User is locked out or banned
/// - Security incident requires force logout
pub async fn revoke_all_sessions(pool: &PgPool, user_id: Uuid, reason: &str) -> ApiResult<u64> {
    let rows_affected = sqlx::query(
        r#"
        UPDATE user_sessions
        SET revoked_at = NOW(),
            revocation_reason = $2
        WHERE user_id = $1
          AND revoked_at IS NULL
        "#,
    )
    .bind(user_id)
    .bind(reason)
    .execute(pool)
    .await?
    .rows_affected();

    Ok(rows_affected)
}

/// List active sessions for a user
#[derive(Debug, sqlx::FromRow, serde::Serialize)]
pub struct UserSession {
    pub id: Uuid,
    pub jti: String,
    pub created_at: OffsetDateTime,
    pub expires_at: OffsetDateTime,
    pub last_used_at: OffsetDateTime,
    pub ip_address: Option<String>,
    pub user_agent: Option<String>,
    pub token_type: String,
}

pub async fn list_sessions(pool: &PgPool, user_id: Uuid) -> ApiResult<Vec<UserSession>> {
    let sessions = sqlx::query_as::<_, UserSession>(
        r#"
        SELECT
            id,
            jti,
            created_at,
            expires_at,
            last_used_at,
            ip_address,
            user_agent,
            token_type
        FROM user_sessions
        WHERE user_id = $1
          AND revoked_at IS NULL
          AND expires_at > NOW()
          AND token_type = 'refresh'  -- Only show refresh tokens (parent sessions)
        ORDER BY created_at DESC
        "#,
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;

    Ok(sessions)
}

#[cfg(test)]
mod tests {
    #[allow(unused_imports)]
    use super::*;

    #[test]
    fn test_session_functions_compile() {
        // This test just ensures the module compiles
        // Actual integration tests require a test database
    }
}
