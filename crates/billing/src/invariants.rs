//! Billing Invariants Module
//!
//! Provides runnable consistency checks for the billing system.
//! These invariants can be run after any mutation or webhook replay to ensure
//! the system is in a valid state.
//!
//! ## Design Principles
//!
//! 1. **Executable**: Each invariant is a real SQL query that can be run
//! 2. **Explanatory**: Violations include enough context to debug
//! 3. **Non-destructive**: Checks only read, never write
//! 4. **Complete**: Covers all critical billing consistency requirements

use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::BillingResult;

/// Result of running a single invariant check
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvariantViolation {
    /// Which invariant was violated
    pub invariant: String,
    /// Organization(s) affected
    pub org_ids: Vec<Uuid>,
    /// Human-readable description of the violation
    pub description: String,
    /// Additional context for debugging
    pub context: serde_json::Value,
    /// Severity level
    pub severity: ViolationSeverity,
}

/// Severity of an invariant violation
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ViolationSeverity {
    /// Critical - system may be charging incorrectly
    Critical,
    /// High - data inconsistency that needs attention
    High,
    /// Medium - potential issue, should investigate
    Medium,
    /// Low - minor inconsistency, informational
    Low,
}

impl std::fmt::Display for ViolationSeverity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ViolationSeverity::Critical => write!(f, "CRITICAL"),
            ViolationSeverity::High => write!(f, "HIGH"),
            ViolationSeverity::Medium => write!(f, "MEDIUM"),
            ViolationSeverity::Low => write!(f, "LOW"),
        }
    }
}

/// Summary of all invariant checks
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvariantCheckSummary {
    /// When the check was run
    pub checked_at: OffsetDateTime,
    /// Total number of checks run
    pub checks_run: usize,
    /// Number of checks that passed
    pub checks_passed: usize,
    /// Number of checks that failed
    pub checks_failed: usize,
    /// List of all violations found
    pub violations: Vec<InvariantViolation>,
    /// Overall health status
    pub healthy: bool,
}

/// Row type for multiple subscriptions violation
#[derive(Debug, sqlx::FromRow)]
struct MultipleSubsRow {
    org_id: Uuid,
    sub_count: i64,
}

/// Row type for tier mismatch violation
#[derive(Debug, sqlx::FromRow)]
struct TierMismatchRow {
    org_id: Uuid,
    org_name: String,
    db_tier: String,
    stripe_price_id: Option<String>,
}

/// Row type for canceled without period end violation
#[derive(Debug, sqlx::FromRow)]
#[allow(dead_code)]
struct CanceledNoPeriodEndRow {
    sub_id: Uuid,
    org_id: Uuid,
    status: String,
    current_period_end: Option<OffsetDateTime>,
}

/// Row type for unaudited tier change violation
#[derive(Debug, sqlx::FromRow)]
struct UnauditedTierChangeRow {
    org_id: Uuid,
    org_name: String,
    tier_changed_at: Option<OffsetDateTime>,
}

/// Row type for spend cap inconsistency violation
#[derive(Debug, sqlx::FromRow)]
struct SpendCapInconsistencyRow {
    org_id: Uuid,
    is_paused: bool,
    current_period_spend_cents: i64,
    cap_amount_cents: i64,
}

/// Row type for missing Stripe customer violation
#[derive(Debug, sqlx::FromRow)]
struct MissingStripeCustomerRow {
    org_id: Uuid,
    org_name: String,
    subscription_tier: String,
}

/// Service for running billing invariant checks
pub struct InvariantChecker {
    pool: PgPool,
}

impl InvariantChecker {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Run all invariant checks and return summary
    pub async fn run_all_checks(&self) -> BillingResult<InvariantCheckSummary> {
        let now = OffsetDateTime::now_utc();
        let mut violations = Vec::new();

        // Run all checks
        violations.extend(self.check_single_active_subscription().await?);
        violations.extend(self.check_tier_matches_subscription().await?);
        violations.extend(self.check_canceled_has_period_end().await?);
        violations.extend(self.check_tier_changes_audited().await?);
        violations.extend(self.check_spend_cap_consistency().await?);
        violations.extend(self.check_stripe_customer_exists().await?);

        let checks_run = 6;
        let checks_failed = violations
            .iter()
            .map(|v| &v.invariant)
            .collect::<std::collections::HashSet<_>>()
            .len();
        let checks_passed = checks_run - checks_failed;

        Ok(InvariantCheckSummary {
            checked_at: now,
            checks_run,
            checks_passed,
            checks_failed,
            healthy: violations.is_empty(),
            violations,
        })
    }

    /// Invariant 1: At most 1 active subscription per org
    ///
    /// Having multiple active subscriptions would cause double-billing
    /// and entitlement confusion.
    async fn check_single_active_subscription(&self) -> BillingResult<Vec<InvariantViolation>> {
        let rows: Vec<MultipleSubsRow> = sqlx::query_as(
            r#"
            SELECT org_id, COUNT(*) as sub_count
            FROM subscriptions
            WHERE status IN ('active', 'trialing', 'past_due')
            GROUP BY org_id
            HAVING COUNT(*) > 1
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| InvariantViolation {
                invariant: "single_active_subscription".to_string(),
                org_ids: vec![row.org_id],
                description: format!(
                    "Organization has {} active subscriptions (expected 1)",
                    row.sub_count
                ),
                context: serde_json::json!({
                    "subscription_count": row.sub_count,
                }),
                severity: ViolationSeverity::Critical,
            })
            .collect())
    }

    /// Invariant 2: Organization tier matches subscription price
    ///
    /// If the tier in `organizations` doesn't match what Stripe is charging,
    /// customers may have wrong access or be charged incorrectly.
    async fn check_tier_matches_subscription(&self) -> BillingResult<Vec<InvariantViolation>> {
        // This query finds orgs where tier doesn't match subscription price
        // Note: Price IDs contain tier names (e.g., "price_xxx_pro_monthly")
        let rows: Vec<TierMismatchRow> = sqlx::query_as(
            r#"
            SELECT
                o.id as org_id,
                o.name as org_name,
                o.subscription_tier as db_tier,
                s.stripe_price_id
            FROM organizations o
            JOIN subscriptions s ON s.org_id = o.id
            WHERE s.status IN ('active', 'trialing')
              AND o.subscription_tier != 'enterprise'  -- Enterprise can have custom prices
              AND o.subscription_tier != 'free'        -- Free tier shouldn't have subscription
              AND NOT (
                  (o.subscription_tier = 'pro' AND s.stripe_price_id ILIKE '%pro%')
                  OR (o.subscription_tier = 'team' AND s.stripe_price_id ILIKE '%team%')
                  OR (o.subscription_tier = 'starter' AND s.stripe_price_id ILIKE '%starter%')
              )
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| InvariantViolation {
                invariant: "tier_matches_subscription".to_string(),
                org_ids: vec![row.org_id],
                description: format!(
                    "Organization '{}' has tier '{}' but subscription price '{}' doesn't match",
                    row.org_name,
                    row.db_tier,
                    row.stripe_price_id.as_deref().unwrap_or("(none)")
                ),
                context: serde_json::json!({
                    "org_name": row.org_name,
                    "db_tier": row.db_tier,
                    "stripe_price_id": row.stripe_price_id,
                }),
                severity: ViolationSeverity::Critical,
            })
            .collect())
    }

    /// Invariant 3: Canceled subscriptions have valid period_end
    ///
    /// A canceled subscription should have a period_end date so we know
    /// when to revoke access.
    async fn check_canceled_has_period_end(&self) -> BillingResult<Vec<InvariantViolation>> {
        let rows: Vec<CanceledNoPeriodEndRow> = sqlx::query_as(
            r#"
            SELECT
                s.id as sub_id,
                s.org_id,
                s.status,
                s.current_period_end
            FROM subscriptions s
            WHERE s.status = 'canceled'
              AND s.current_period_end IS NULL
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| InvariantViolation {
                invariant: "canceled_has_period_end".to_string(),
                org_ids: vec![row.org_id],
                description: "Canceled subscription has no period_end date".to_string(),
                context: serde_json::json!({
                    "subscription_id": row.sub_id,
                    "status": row.status,
                }),
                severity: ViolationSeverity::High,
            })
            .collect())
    }

    /// Invariant 4: All tier changes have audit records
    ///
    /// Every tier change should be recorded in tier_change_audit for
    /// compliance and debugging.
    async fn check_tier_changes_audited(&self) -> BillingResult<Vec<InvariantViolation>> {
        let rows: Vec<UnauditedTierChangeRow> = sqlx::query_as(
            r#"
            SELECT
                o.id as org_id,
                o.name as org_name,
                o.tier_changed_at
            FROM organizations o
            WHERE o.tier_changed_at IS NOT NULL
              AND NOT EXISTS (
                  SELECT 1 FROM tier_change_audit t
                  WHERE t.org_id = o.id
                    AND t.created_at BETWEEN o.tier_changed_at - INTERVAL '5 minutes'
                                         AND o.tier_changed_at + INTERVAL '5 minutes'
              )
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| InvariantViolation {
                invariant: "tier_changes_audited".to_string(),
                org_ids: vec![row.org_id],
                description: format!(
                    "Organization '{}' has tier change at {:?} with no audit record",
                    row.org_name, row.tier_changed_at
                ),
                context: serde_json::json!({
                    "org_name": row.org_name,
                    "tier_changed_at": row.tier_changed_at,
                }),
                severity: ViolationSeverity::High,
            })
            .collect())
    }

    /// Invariant 5: Spend cap consistency
    ///
    /// If org is paused, spend should be >= cap.
    /// If org is not paused, spend should be < cap (unless no cap set).
    async fn check_spend_cap_consistency(&self) -> BillingResult<Vec<InvariantViolation>> {
        // Find orgs that are paused but spend is under cap
        let rows: Vec<SpendCapInconsistencyRow> = sqlx::query_as(
            r#"
            SELECT
                sc.org_id,
                sc.is_paused,
                sc.current_period_spend_cents,
                sc.cap_amount_cents
            FROM spend_caps sc
            WHERE sc.is_paused = true
              AND sc.current_period_spend_cents < sc.cap_amount_cents
              AND sc.cap_amount_cents > 0
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| InvariantViolation {
                invariant: "spend_cap_consistency".to_string(),
                org_ids: vec![row.org_id],
                description: format!(
                    "Organization is paused but spend (${:.2}) is under cap (${:.2})",
                    row.current_period_spend_cents as f64 / 100.0,
                    row.cap_amount_cents as f64 / 100.0
                ),
                context: serde_json::json!({
                    "is_paused": row.is_paused,
                    "current_spend_cents": row.current_period_spend_cents,
                    "cap_amount_cents": row.cap_amount_cents,
                }),
                severity: ViolationSeverity::Medium,
            })
            .collect())
    }

    /// Invariant 6: Paid tiers have Stripe customer
    ///
    /// Organizations on paid tiers (not free or enterprise-manual) should
    /// have a Stripe customer ID.
    async fn check_stripe_customer_exists(&self) -> BillingResult<Vec<InvariantViolation>> {
        let rows: Vec<MissingStripeCustomerRow> = sqlx::query_as(
            r#"
            SELECT
                o.id as org_id,
                o.name as org_name,
                o.subscription_tier
            FROM organizations o
            WHERE o.subscription_tier NOT IN ('free')
              AND o.stripe_customer_id IS NULL
              AND NOT (
                  o.subscription_tier = 'enterprise'
                  AND o.custom_max_mcps IS NOT NULL  -- Manual enterprise setup
              )
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| InvariantViolation {
                invariant: "stripe_customer_exists".to_string(),
                org_ids: vec![row.org_id],
                description: format!(
                    "Organization '{}' on tier '{}' has no Stripe customer",
                    row.org_name, row.subscription_tier
                ),
                context: serde_json::json!({
                    "org_name": row.org_name,
                    "subscription_tier": row.subscription_tier,
                }),
                severity: ViolationSeverity::High,
            })
            .collect())
    }

    /// Run a single invariant check by name
    pub async fn run_check(&self, name: &str) -> BillingResult<Vec<InvariantViolation>> {
        match name {
            "single_active_subscription" => self.check_single_active_subscription().await,
            "tier_matches_subscription" => self.check_tier_matches_subscription().await,
            "canceled_has_period_end" => self.check_canceled_has_period_end().await,
            "tier_changes_audited" => self.check_tier_changes_audited().await,
            "spend_cap_consistency" => self.check_spend_cap_consistency().await,
            "stripe_customer_exists" => self.check_stripe_customer_exists().await,
            _ => Ok(vec![]),
        }
    }

    /// Get list of all available invariant checks
    pub fn available_checks() -> Vec<&'static str> {
        vec![
            "single_active_subscription",
            "tier_matches_subscription",
            "canceled_has_period_end",
            "tier_changes_audited",
            "spend_cap_consistency",
            "stripe_customer_exists",
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_violation_severity_display() {
        assert_eq!(ViolationSeverity::Critical.to_string(), "CRITICAL");
        assert_eq!(ViolationSeverity::High.to_string(), "HIGH");
        assert_eq!(ViolationSeverity::Medium.to_string(), "MEDIUM");
        assert_eq!(ViolationSeverity::Low.to_string(), "LOW");
    }

    #[test]
    fn test_available_checks() {
        let checks = InvariantChecker::available_checks();
        assert_eq!(checks.len(), 6);
        assert!(checks.contains(&"single_active_subscription"));
        assert!(checks.contains(&"tier_matches_subscription"));
    }
}
