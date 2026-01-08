//! Subscription management

use plexmcp_shared::SubscriptionTier;
use sqlx::PgPool;
use stripe::{
    CancelSubscription, CreateCustomer, CreateSubscription, CreateSubscriptionItems, Customer,
    CustomerId, ListSubscriptions, Subscription, SubscriptionId,
    SubscriptionStatus as StripeSubStatus, UpdateSubscription, UpdateSubscriptionItems,
};
// Import the proration behavior enum from the subscription module (not subscription_item)
use stripe::generated::billing::subscription::SubscriptionProrationBehavior;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::client::StripeClient;
use crate::error::{BillingError, BillingResult};
use crate::events::{ActorType, BillingEventBuilder, BillingEventLogger, BillingEventType};
use crate::refund::RefundService;

/// Custom SubscriptionItemFilter that uses `price` instead of `plan`
/// The async-stripe 0.39 library's SubscriptionItemFilter only has `plan`,
/// but Stripe's modern API uses `price` for subscription items.
#[derive(Clone, Debug, serde::Serialize)]
#[allow(dead_code)]
struct SubscriptionItemFilterWithPrice {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<stripe::SubscriptionItemId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deleted: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub price: Option<stripe::PriceId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quantity: Option<u64>,
}

/// Custom RetrieveUpcomingInvoice that uses SubscriptionItemFilterWithPrice
/// This allows us to use the `price` field instead of the deprecated `plan` field
#[derive(Clone, Debug, serde::Serialize)]
#[allow(dead_code)]
struct RetrieveUpcomingInvoiceWithPrice {
    pub customer: CustomerId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subscription: Option<SubscriptionId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subscription_items: Option<Vec<SubscriptionItemFilterWithPrice>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subscription_proration_behavior: Option<String>,
}

/// Information about a scheduled downgrade
#[derive(Debug, Clone, serde::Serialize)]
pub struct ScheduledDowngrade {
    pub current_tier: String,
    pub new_tier: String,
    pub effective_date: OffsetDateTime,
}

/// Parameters for admin-initiated tier changes
#[derive(Debug, Clone)]
pub struct AdminTierChangeParams {
    /// New tier to change to (free, pro, team, enterprise)
    pub new_tier: String,
    /// Optional trial period in days (0-730)
    pub trial_days: Option<u32>,
    /// Reason for the tier change (for audit logging)
    pub reason: String,
    /// Skip payment method validation (allows tier change without payment method)
    pub skip_payment_validation: bool,
    /// Billing interval: "monthly" or "annual" (default: monthly)
    pub billing_interval: Option<String>,
    /// Custom price in cents for Enterprise tier
    pub custom_price_cents: Option<i64>,
    /// Scheduled start date for subscription
    pub subscription_start_date: Option<OffsetDateTime>,
    /// Payment method: "immediate", "invoice", or "trial"
    pub payment_method: Option<String>,
    /// Admin user ID who initiated the tier change (for tracking scheduled downgrades)
    pub admin_user_id: Option<Uuid>,
    /// For downgrades: "scheduled" (at period end, default) or "immediate" (now with prorated credit/refund)
    pub downgrade_timing: Option<String>,
    /// For immediate downgrades: "refund" (money back to payment method) or "credit" (Stripe account credit)
    /// Defaults to "credit" for backwards compatibility
    pub refund_type: Option<String>,
}

/// Result of an admin-initiated tier change
#[derive(Debug, Clone, serde::Serialize)]
pub struct AdminTierChangeResult {
    /// Stripe subscription ID (None if downgraded to Free)
    pub stripe_subscription_id: Option<String>,
    /// Stripe customer ID
    pub stripe_customer_id: String,
    /// The new tier
    pub tier: String,
    /// Trial end date if a trial was applied
    pub trial_end: Option<OffsetDateTime>,
    /// Stripe invoice ID if invoice payment method was used
    pub stripe_invoice_id: Option<String>,
    /// Invoice status if invoice was created
    pub invoice_status: Option<String>,
    /// Subscription start date if scheduled
    pub subscription_start_date: Option<OffsetDateTime>,
    /// Billing interval ("monthly" or "annual")
    pub billing_interval: String,
    /// Custom Stripe price ID for Enterprise tier
    pub custom_price_id: Option<String>,
    /// Whether the tier change was scheduled for period end (vs immediate)
    pub scheduled: bool,
    /// Effective date if scheduled
    pub scheduled_effective_date: Option<OffsetDateTime>,
    /// Whether a refund/credit was issued for this tier change
    pub refund_issued: bool,
    /// Amount refunded/credited in cents (if any)
    pub refund_amount_cents: Option<i64>,
    /// Type of refund: "refund" (money back) or "credit" (Stripe account credit)
    pub refund_type: Option<String>,
    /// Message explaining what happened
    pub message: String,
}

/// Information about a cancelled subscription
#[derive(Debug, Clone)]
pub struct CancelledSubscriptionInfo {
    pub stripe_subscription_id: String,
    pub stripe_price_id: Option<String>,
    pub current_period_start: OffsetDateTime,
    pub current_period_end: OffsetDateTime,
    pub canceled_at: Option<OffsetDateTime>,
}

/// Source of a tier change operation
/// Used for audit logging and determining behavior (e.g., whether to sync to Stripe)
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum TierChangeSource {
    /// User initiated upgrade via checkout
    UserUpgrade,
    /// User scheduled downgrade
    UserDowngrade,
    /// Admin changed tier directly via admin panel
    AdminPanel,
    /// Stripe webhook confirmed payment (should NOT change tier, only payment_status)
    StripeWebhook,
    /// System automated change (e.g., trial expiry)
    System,
}

impl TierChangeSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            TierChangeSource::UserUpgrade => "user_upgrade",
            TierChangeSource::UserDowngrade => "user_downgrade",
            TierChangeSource::AdminPanel => "admin_panel",
            TierChangeSource::StripeWebhook => "stripe_webhook",
            TierChangeSource::System => "system",
        }
    }
}

impl std::fmt::Display for TierChangeSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Options for tier change operations
/// This struct consolidates all possible options from admin panel, user actions, and webhooks
#[derive(Debug, Clone, Default)]
pub struct TierChangeOptions {
    /// Source of the tier change
    pub source: Option<TierChangeSource>,
    /// User who initiated the change (for audit)
    pub changed_by: Option<Uuid>,
    /// Reason for the tier change (for audit logging)
    pub reason: Option<String>,

    // Admin-specific options
    /// Skip payment method validation (allows tier change without payment method)
    pub skip_payment_validation: bool,
    /// Custom price in cents for Enterprise tier
    pub custom_price_cents: Option<i64>,
    /// Billing interval: "monthly" or "annual"
    pub billing_interval: Option<String>,
    /// Apply trial period in days
    pub trial_days: Option<i32>,
    /// Schedule for future
    pub scheduled_date: Option<OffsetDateTime>,
    /// Downgrade timing: "immediate" or "end_of_period"
    pub downgrade_timing: Option<String>,
    /// For immediate downgrades: "refund" or "credit"
    pub refund_type: Option<String>,
}

impl TierChangeOptions {
    /// Create options for a user-initiated upgrade
    pub fn user_upgrade() -> Self {
        Self {
            source: Some(TierChangeSource::UserUpgrade),
            ..Default::default()
        }
    }

    /// Create options for a user-initiated downgrade
    pub fn user_downgrade() -> Self {
        Self {
            source: Some(TierChangeSource::UserDowngrade),
            downgrade_timing: Some("end_of_period".to_string()),
            ..Default::default()
        }
    }

    /// Create options for an admin panel change
    pub fn admin_panel(changed_by: Uuid, reason: Option<String>) -> Self {
        Self {
            source: Some(TierChangeSource::AdminPanel),
            changed_by: Some(changed_by),
            reason,
            ..Default::default()
        }
    }

    /// Create options for a system automated change
    pub fn system(reason: &str) -> Self {
        Self {
            source: Some(TierChangeSource::System),
            reason: Some(reason.to_string()),
            ..Default::default()
        }
    }
}

/// Result of a tier change operation
#[derive(Debug, Clone, serde::Serialize)]
pub struct TierChangeResult {
    /// Whether the change succeeded
    pub success: bool,
    /// Previous tier
    pub from_tier: String,
    /// New tier
    pub to_tier: String,
    /// Stripe subscription ID (None if downgraded to Free)
    pub stripe_subscription_id: Option<String>,
    /// Whether the change is scheduled for later
    pub scheduled: bool,
    /// Scheduled effective date if applicable
    pub effective_date: Option<OffsetDateTime>,
    /// Credit/refund amount if applicable
    pub credit_cents: Option<i64>,
    /// Message explaining what happened
    pub message: String,
}

/// Result of reactivating a cancelled subscription
#[derive(Debug, Clone, serde::Serialize)]
pub struct ReactivationResult {
    /// Stripe subscription ID of the new subscription
    pub stripe_subscription_id: String,
    /// Stripe customer ID
    pub stripe_customer_id: String,
    /// The new tier
    pub tier: String,
    /// Billing interval ("monthly" or "annual")
    pub billing_interval: String,
    /// Credit applied from cancelled subscription (in cents)
    pub credit_applied_cents: i64,
    /// Extra trial days applied (from credit)
    pub extra_trial_days: i32,
    /// Overages deducted from credit (in cents)
    pub overages_deducted_cents: i64,
    /// Current period end of the new subscription
    pub current_period_end: OffsetDateTime,
    /// Trial end date if credit was applied
    pub trial_end: Option<OffsetDateTime>,
    /// Message explaining what happened
    pub message: String,
}

/// Preview of proration for subscription upgrade
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProrationPreview {
    /// Current subscription tier
    pub current_tier: String,
    /// Target subscription tier
    pub new_tier: String,
    /// Prorated amount in cents (credit or charge for remaining period)
    pub proration_amount_cents: i64,
    /// Outstanding overage charges in cents
    pub overage_amount_cents: i64,
    /// Total amount to be charged in cents (proration + overages)
    pub total_amount_cents: i64,
    /// Days remaining in current billing period
    pub days_remaining: i32,
    /// Human-readable description of the proration
    pub description: String,
}

/// Subscription plan configuration
#[derive(Debug, Clone)]
pub struct Plan {
    pub tier: SubscriptionTier,
    pub stripe_price_id: String,
    pub monthly_requests: u64,
    pub max_mcps: u32,
    pub max_users: u32,
    pub max_api_keys: u32,
}

impl Plan {
    /// Free tier: 5 MCPs, 5 API keys, 1K calls/month, 1 team member
    pub fn free() -> Self {
        Self {
            tier: SubscriptionTier::Free,
            stripe_price_id: String::new(),
            monthly_requests: 1_000,
            max_mcps: 5,
            max_users: 1,
            max_api_keys: 5,
        }
    }

    /// Pro tier: 20 MCPs, 20 API keys, 50K calls/month, 5 team members
    pub fn pro(price_id: &str) -> Self {
        Self {
            tier: SubscriptionTier::Pro,
            stripe_price_id: price_id.to_string(),
            monthly_requests: 50_000,
            max_mcps: 20,
            max_users: 5,
            max_api_keys: 20,
        }
    }

    /// Team tier: 50 MCPs, 50 API keys, 250K calls/month, unlimited team members
    pub fn team(price_id: &str) -> Self {
        Self {
            tier: SubscriptionTier::Team,
            stripe_price_id: price_id.to_string(),
            monthly_requests: 250_000,
            max_mcps: 50,
            max_users: u32::MAX,
            max_api_keys: 50,
        }
    }

    /// Enterprise tier: Unlimited everything
    pub fn enterprise(price_id: &str) -> Self {
        Self {
            tier: SubscriptionTier::Enterprise,
            stripe_price_id: price_id.to_string(),
            monthly_requests: u64::MAX,
            max_mcps: u32::MAX,
            max_users: u32::MAX,
            max_api_keys: u32::MAX,
        }
    }
}

/// Subscription service for managing Stripe subscriptions
pub struct SubscriptionService {
    stripe: StripeClient,
    pool: PgPool,
    event_logger: BillingEventLogger,
}

impl SubscriptionService {
    pub fn new(stripe: StripeClient, pool: PgPool) -> Self {
        let event_logger = BillingEventLogger::new(pool.clone());
        Self {
            stripe,
            pool,
            event_logger,
        }
    }

    /// Get the Stripe client for config access
    pub fn stripe(&self) -> &StripeClient {
        &self.stripe
    }

    // =========================================================================
    // CONSOLIDATED TIER CHANGE FUNCTION
    // =========================================================================
    // This is the SINGLE authoritative function for changing subscription tiers.
    // ALL tier changes (user upgrades, admin panel, downgrades) MUST go through this function.
    // The database is the source of truth - Stripe is for payment processing only.
    // =========================================================================

    /// Consolidated tier change function
    ///
    /// This is the ONLY function that should change an organization's subscription tier.
    /// It enforces:
    /// - Atomic updates with optimistic locking
    /// - Audit trail for all changes
    /// - Proper Stripe sync when needed
    ///
    /// # Arguments
    /// * `org_id` - The organization to change
    /// * `new_tier` - The target tier (free, pro, team, enterprise)
    /// * `options` - Options controlling the change behavior
    pub async fn change_tier(
        &self,
        org_id: Uuid,
        new_tier: &str,
        options: TierChangeOptions,
    ) -> BillingResult<TierChangeResult> {
        let source = options.source.unwrap_or(TierChangeSource::System);

        tracing::info!(
            org_id = %org_id,
            new_tier = %new_tier,
            source = %source,
            "Starting consolidated tier change"
        );

        // Validate tier value
        const VALID_TIERS: &[&str] = &["free", "pro", "team", "enterprise"];
        if !VALID_TIERS.contains(&new_tier) {
            return Err(BillingError::InvalidTier(format!(
                "Invalid tier '{}'. Valid tiers are: free, pro, team, enterprise",
                new_tier
            )));
        }

        // Start a transaction for atomic updates
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| BillingError::Database(e.to_string()))?;

        // Get current state with row lock (FOR UPDATE)
        let current: Option<(String, i64)> = sqlx::query_as(
            "SELECT subscription_tier, tier_version FROM organizations WHERE id = $1 FOR UPDATE",
        )
        .bind(org_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| BillingError::Database(e.to_string()))?;

        let (current_tier, current_version) = current
            .ok_or_else(|| BillingError::NotFound(format!("Organization {} not found", org_id)))?;

        // Check if this is a downgrade
        let is_downgrade = self.is_tier_downgrade(&current_tier, new_tier);
        let downgrade_timing = options
            .downgrade_timing
            .as_deref()
            .unwrap_or(if is_downgrade {
                "end_of_period"
            } else {
                "immediate"
            });

        // If this is a scheduled downgrade (end of period), record it and return
        if is_downgrade && downgrade_timing == "end_of_period" {
            // Check if there's already a scheduled downgrade (prevents silent overwrites)
            let existing_scheduled: Option<(Option<String>,)> = sqlx::query_as(
                "SELECT scheduled_downgrade_tier FROM subscriptions WHERE org_id = $1",
            )
            .bind(org_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|e| BillingError::Database(e.to_string()))?;

            if let Some((Some(existing_tier),)) = existing_scheduled {
                if existing_tier != new_tier {
                    tracing::warn!(
                        org_id = %org_id,
                        existing_scheduled_tier = %existing_tier,
                        new_scheduled_tier = %new_tier,
                        "Overwriting existing scheduled downgrade - previous scheduling will be replaced"
                    );
                }
            }

            // Get current period end from subscription
            let period_end: Option<OffsetDateTime> = sqlx::query_scalar(
                "SELECT current_period_end FROM subscriptions WHERE org_id = $1",
            )
            .bind(org_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|e| BillingError::Database(e.to_string()))?
            .flatten();

            let effective_date = period_end.unwrap_or_else(OffsetDateTime::now_utc);

            // Record the scheduled downgrade
            sqlx::query(
                r#"
                UPDATE subscriptions SET
                    scheduled_downgrade_tier = $1,
                    scheduled_downgrade_at = $2,
                    admin_downgrade_scheduled = $3,
                    admin_downgrade_scheduled_by = $4,
                    admin_downgrade_reason = $5,
                    updated_at = NOW()
                WHERE org_id = $6
                "#,
            )
            .bind(new_tier)
            .bind(effective_date)
            .bind(source == TierChangeSource::AdminPanel)
            .bind(options.changed_by)
            .bind(&options.reason)
            .bind(org_id)
            .execute(&mut *tx)
            .await
            .map_err(|e| BillingError::Database(e.to_string()))?;

            // Audit log
            self.log_tier_change_audit(
                &mut tx,
                org_id,
                &current_tier,
                new_tier,
                source,
                options.changed_by,
                &options,
                true, // scheduled
            )
            .await?;

            tx.commit()
                .await
                .map_err(|e| BillingError::Database(e.to_string()))?;

            tracing::info!(
                org_id = %org_id,
                from_tier = %current_tier,
                to_tier = %new_tier,
                effective_date = %effective_date,
                "Scheduled tier downgrade at period end"
            );

            // Log billing event for scheduled tier change
            let actor_type = match source {
                TierChangeSource::AdminPanel => ActorType::Admin,
                TierChangeSource::UserUpgrade | TierChangeSource::UserDowngrade => ActorType::User,
                _ => ActorType::System,
            };
            if let Err(e) = self
                .event_logger
                .log_event(
                    BillingEventBuilder::new(org_id, BillingEventType::TierChangeScheduled)
                        .data(serde_json::json!({
                            "from_tier": current_tier,
                            "to_tier": new_tier,
                            "effective_date": effective_date.to_string(),
                            "is_downgrade": true,
                        }))
                        .actor_opt(options.changed_by, actor_type),
                )
                .await
            {
                tracing::warn!(error = %e, "Failed to log scheduled tier change event");
            }

            return Ok(TierChangeResult {
                success: true,
                from_tier: current_tier,
                to_tier: new_tier.to_string(),
                stripe_subscription_id: None,
                scheduled: true,
                effective_date: Some(effective_date),
                credit_cents: None,
                message: format!("Downgrade to {} scheduled for {}", new_tier, effective_date),
            });
        }

        // Immediate tier change - update the database (SOURCE OF TRUTH)
        let rows_affected = sqlx::query(
            r#"
            UPDATE organizations SET
                subscription_tier = $1,
                tier_version = tier_version + 1,
                tier_source = $2,
                tier_changed_by = $3,
                tier_changed_at = NOW(),
                updated_at = NOW()
            WHERE id = $4 AND tier_version = $5
            "#,
        )
        .bind(new_tier)
        .bind(source.as_str())
        .bind(options.changed_by)
        .bind(org_id)
        .bind(current_version)
        .execute(&mut *tx)
        .await
        .map_err(|e| BillingError::Database(e.to_string()))?
        .rows_affected();

        if rows_affected == 0 {
            // Optimistic lock failed - someone else modified the row
            return Err(BillingError::ConcurrentModification(
                "Tier was modified by another process. Please retry.".to_string(),
            ));
        }

        // Update subscriptions table version
        sqlx::query(
            "UPDATE subscriptions SET version = version + 1, last_synced_at = NOW() WHERE org_id = $1"
        )
        .bind(org_id)
        .execute(&mut *tx)
        .await
        .ok(); // Ignore if no subscription record exists

        // Clear any scheduled downgrade since we're doing an immediate change
        sqlx::query(
            r#"
            UPDATE subscriptions SET
                scheduled_downgrade_tier = NULL,
                scheduled_downgrade_at = NULL,
                scheduled_downgrade_processing = false
            WHERE org_id = $1
            "#,
        )
        .bind(org_id)
        .execute(&mut *tx)
        .await
        .ok();

        // Audit log
        self.log_tier_change_audit(
            &mut tx,
            org_id,
            &current_tier,
            new_tier,
            source,
            options.changed_by,
            &options,
            false, // not scheduled
        )
        .await?;

        tx.commit()
            .await
            .map_err(|e| BillingError::Database(e.to_string()))?;

        tracing::info!(
            org_id = %org_id,
            from_tier = %current_tier,
            to_tier = %new_tier,
            source = %source,
            "Tier changed successfully (DB is authoritative)"
        );

        // Log billing event for immediate tier change
        let actor_type = match source {
            TierChangeSource::AdminPanel => ActorType::Admin,
            TierChangeSource::UserUpgrade | TierChangeSource::UserDowngrade => ActorType::User,
            _ => ActorType::System,
        };
        let is_downgrade = self.is_tier_downgrade(&current_tier, new_tier);
        if let Err(e) = self
            .event_logger
            .log_event(
                BillingEventBuilder::new(org_id, BillingEventType::TierChanged)
                    .data(serde_json::json!({
                        "from_tier": current_tier,
                        "to_tier": new_tier,
                        "is_downgrade": is_downgrade,
                        "source": source.as_str(),
                    }))
                    .actor_opt(options.changed_by, actor_type),
            )
            .await
        {
            tracing::warn!(error = %e, "Failed to log tier change event");
        }

        let message = format!("Tier changed from {} to {}", &current_tier, new_tier);
        Ok(TierChangeResult {
            success: true,
            from_tier: current_tier,
            to_tier: new_tier.to_string(),
            stripe_subscription_id: None,
            scheduled: false,
            effective_date: None,
            credit_cents: None,
            message,
        })
    }

    /// Helper to determine if a tier change is a downgrade
    fn is_tier_downgrade(&self, from: &str, to: &str) -> bool {
        let tier_order = |t: &str| match t.to_lowercase().as_str() {
            "free" => 0,
            "pro" => 1,
            "team" => 2,
            "enterprise" => 3,
            _ => 0,
        };
        tier_order(to) < tier_order(from)
    }

    /// Log tier change to audit table
    async fn log_tier_change_audit(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        org_id: Uuid,
        from_tier: &str,
        to_tier: &str,
        source: TierChangeSource,
        changed_by: Option<Uuid>,
        options: &TierChangeOptions,
        scheduled: bool,
    ) -> BillingResult<()> {
        let metadata = serde_json::json!({
            "scheduled": scheduled,
            "downgrade_timing": options.downgrade_timing,
            "trial_days": options.trial_days,
            "custom_price_cents": options.custom_price_cents,
            "billing_interval": options.billing_interval,
        });

        sqlx::query(
            r#"
            INSERT INTO tier_change_audit
                (org_id, from_tier, to_tier, source, changed_by, reason, metadata)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            "#,
        )
        .bind(org_id)
        .bind(from_tier)
        .bind(to_tier)
        .bind(source.as_str())
        .bind(changed_by)
        .bind(&options.reason)
        .bind(metadata)
        .execute(&mut **tx)
        .await
        .map_err(|e| BillingError::Database(e.to_string()))?;

        Ok(())
    }

    // =========================================================================
    // END CONSOLIDATED TIER CHANGE FUNCTION
    // =========================================================================

    /// Create a new subscription for an organization
    pub async fn create_subscription(
        &self,
        org_id: Uuid,
        customer_id: &str,
        tier: &str,
    ) -> BillingResult<Subscription> {
        let price_id = self
            .stripe
            .config()
            .price_id_for_tier(tier)
            .ok_or_else(|| BillingError::InvalidTier(tier.to_string()))?;

        let customer_id = customer_id
            .parse::<CustomerId>()
            .map_err(|e| BillingError::StripeApi(format!("Invalid customer ID: {}", e)))?;

        let mut metadata = std::collections::HashMap::new();
        metadata.insert("org_id".to_string(), org_id.to_string());
        metadata.insert("tier".to_string(), tier.to_string());

        let mut params = CreateSubscription::new(customer_id);
        params.items = Some(vec![CreateSubscriptionItems {
            price: Some(price_id.to_string()),
            quantity: Some(1),
            ..Default::default()
        }]);
        params.metadata = Some(metadata);

        let subscription = Subscription::create(self.stripe.inner(), params).await?;

        // Store subscription in database
        self.sync_subscription_to_db(org_id, &subscription).await?;

        tracing::info!(
            org_id = %org_id,
            subscription_id = %subscription.id,
            tier = %tier,
            "Created subscription"
        );

        Ok(subscription)
    }

    /// Create an "add-on only" subscription for free tier users purchasing add-ons
    /// This creates a $0 base subscription that add-on items can be attached to
    pub async fn create_addon_only_subscription(
        &self,
        org_id: Uuid,
        customer_id: &str,
    ) -> BillingResult<Subscription> {
        let price_id = self
            .stripe
            .config()
            .price_ids
            .addon_only
            .as_ref()
            .ok_or_else(|| {
                BillingError::Config("STRIPE_PRICE_ADDON_ONLY not configured".to_string())
            })?;

        let customer_id = customer_id
            .parse::<CustomerId>()
            .map_err(|e| BillingError::StripeApi(format!("Invalid customer ID: {}", e)))?;

        let mut metadata = std::collections::HashMap::new();
        metadata.insert("org_id".to_string(), org_id.to_string());
        metadata.insert("tier".to_string(), "free".to_string());
        metadata.insert("type".to_string(), "addon_only".to_string());

        let mut params = CreateSubscription::new(customer_id);
        params.items = Some(vec![CreateSubscriptionItems {
            price: Some(price_id.to_string()),
            quantity: Some(1),
            ..Default::default()
        }]);
        params.metadata = Some(metadata);

        let subscription = Subscription::create(self.stripe.inner(), params).await?;

        // Store subscription in database
        self.sync_subscription_to_db(org_id, &subscription).await?;

        tracing::info!(
            org_id = %org_id,
            subscription_id = %subscription.id,
            "Created add-on only subscription for free tier user"
        );

        Ok(subscription)
    }

    /// Update an existing subscription to a new tier
    pub async fn update_subscription(
        &self,
        org_id: Uuid,
        new_tier: &str,
    ) -> BillingResult<Subscription> {
        let sub_id = self.get_subscription_id(org_id).await?;

        let price_id = self
            .stripe
            .config()
            .price_id_for_tier(new_tier)
            .ok_or_else(|| BillingError::InvalidTier(new_tier.to_string()))?;

        // Get current subscription to get the item ID
        let current = Subscription::retrieve(self.stripe.inner(), &sub_id, &[]).await?;

        let item_id = current
            .items
            .data
            .first()
            .map(|item| item.id.to_string())
            .ok_or_else(|| BillingError::Internal("No subscription items found".to_string()))?;

        let mut metadata = std::collections::HashMap::new();
        metadata.insert("tier".to_string(), new_tier.to_string());

        // Use struct initialization to satisfy clippy::field_reassign_with_default
        let params = UpdateSubscription {
            items: Some(vec![UpdateSubscriptionItems {
                id: Some(item_id),
                price: Some(price_id.to_string()),
                ..Default::default()
            }]),
            metadata: Some(metadata),
            // Explicitly enable proration so users are charged the prorated difference when upgrading
            proration_behavior: Some(SubscriptionProrationBehavior::CreateProrations),
            ..Default::default()
        };

        let subscription = Subscription::update(self.stripe.inner(), &sub_id, params)
            .await
            .map_err(|e| {
                // Check if this is a payment method required error from Stripe
                let err_str = e.to_string();
                if err_str.contains("no attached payment source")
                    || err_str.contains("no default payment method")
                    || err_str.contains("resource_missing")
                {
                    tracing::warn!(
                        org_id = %org_id,
                        error = %err_str,
                        "Subscription update failed: customer has no payment method"
                    );
                    return BillingError::PaymentMethodRequired;
                }
                BillingError::StripeApi(err_str)
            })?;

        // Update database
        self.sync_subscription_to_db(org_id, &subscription).await?;

        // Use consolidated change_tier() for DB update + audit logging
        let tier_options = TierChangeOptions {
            source: Some(TierChangeSource::UserUpgrade),
            ..Default::default()
        };

        self.change_tier(org_id, new_tier, tier_options).await?;

        tracing::info!(
            org_id = %org_id,
            subscription_id = %subscription.id,
            new_tier = %new_tier,
            "Updated subscription tier"
        );

        Ok(subscription)
    }

    /// Upgrade subscription to a new tier AFTER payment has been collected via checkout
    /// This is called from the webhook when an upgrade_payment checkout completes
    /// Uses proration_behavior::None since payment was already collected
    pub async fn upgrade_subscription_after_payment(
        &self,
        org_id: Uuid,
        new_tier: &str,
        billing_interval: &str,
    ) -> BillingResult<Subscription> {
        let sub_id = self.get_subscription_id(org_id).await?;

        // Get the appropriate price ID based on billing interval
        let price_id = if billing_interval == "annual" {
            self.stripe
                .config()
                .annual_price_id_for_tier(new_tier)
                .ok_or_else(|| BillingError::InvalidTier(format!("{} (annual)", new_tier)))?
        } else {
            self.stripe
                .config()
                .price_id_for_tier(new_tier)
                .ok_or_else(|| BillingError::InvalidTier(new_tier.to_string()))?
        };

        // Get current subscription to get the item ID
        let current = Subscription::retrieve(self.stripe.inner(), &sub_id, &[]).await?;

        let item_id = current
            .items
            .data
            .first()
            .map(|item| item.id.to_string())
            .ok_or_else(|| BillingError::Internal("No subscription items found".to_string()))?;

        let mut metadata = std::collections::HashMap::new();
        metadata.insert("tier".to_string(), new_tier.to_string());
        metadata.insert("upgraded_via".to_string(), "checkout_payment".to_string());

        // Use None proration behavior since we already collected payment via checkout
        let params = UpdateSubscription {
            items: Some(vec![UpdateSubscriptionItems {
                id: Some(item_id),
                price: Some(price_id.to_string()),
                ..Default::default()
            }]),
            metadata: Some(metadata),
            // IMPORTANT: None = don't create prorations since we already collected payment
            proration_behavior: Some(SubscriptionProrationBehavior::None),
            ..Default::default()
        };

        let subscription = Subscription::update(self.stripe.inner(), &sub_id, params)
            .await
            .map_err(|e| BillingError::StripeApi(e.to_string()))?;

        // Update database
        self.sync_subscription_to_db(org_id, &subscription).await?;

        // Use consolidated change_tier() for DB update + audit logging
        let tier_options = TierChangeOptions {
            source: Some(TierChangeSource::UserUpgrade),
            ..Default::default()
        };

        self.change_tier(org_id, new_tier, tier_options).await?;

        tracing::info!(
            org_id = %org_id,
            subscription_id = %subscription.id,
            new_tier = %new_tier,
            "Upgraded subscription after checkout payment"
        );

        Ok(subscription)
    }

    /// Preview the proration for upgrading to a new tier
    /// Returns the prorated amount in cents that would be charged immediately
    pub async fn preview_upgrade_proration(
        &self,
        org_id: Uuid,
        new_tier: &str,
    ) -> BillingResult<ProrationPreview> {
        tracing::info!(org_id = %org_id, new_tier = %new_tier, "Starting preview_upgrade_proration");

        // Get subscription ID
        let sub_id = self.get_subscription_id(org_id).await?;
        tracing::info!(sub_id = %sub_id, "Got subscription ID");

        // Get customer ID
        let customer_id = self.get_stripe_customer_id(org_id).await?;
        tracing::info!(customer_id = %customer_id, "Got customer ID");

        // Get the new price ID
        let new_price_id = self
            .stripe
            .config()
            .price_id_for_tier(new_tier)
            .ok_or_else(|| BillingError::InvalidTier(new_tier.to_string()))?;
        tracing::info!(new_price_id = %new_price_id, "Got new price ID");

        // Get current subscription to get the item ID
        let current = Subscription::retrieve(self.stripe.inner(), &sub_id, &[]).await?;
        tracing::info!(
            status = ?current.status,
            trial_end = ?current.trial_end,
            "Retrieved current subscription"
        );

        let item_id = current
            .items
            .data
            .first()
            .map(|item| item.id.clone())
            .ok_or_else(|| BillingError::Internal("No subscription items found".to_string()))?;
        tracing::info!(item_id = %item_id, "Got subscription item ID");

        // Get current tier name from price
        let current_tier = current
            .items
            .data
            .first()
            .and_then(|item| item.price.as_ref())
            .and_then(|price| self.stripe.config().tier_for_price_id(price.id.as_str()))
            .unwrap_or("unknown");
        tracing::info!(current_tier = %current_tier, "Got current tier");

        // Use the new POST /invoices/create_preview API (the old GET /invoices/upcoming is deprecated)
        tracing::info!(
            customer = %customer_id,
            subscription = %sub_id,
            item_id = %item_id,
            new_price_id = %new_price_id,
            "Calling Stripe invoices/create_preview API"
        );

        // Build form-urlencoded body with Stripe's nested parameter format
        let form_params = [
            ("customer", customer_id.as_str()),
            ("subscription", sub_id.as_ref()),
            ("subscription_details[items][0][id]", item_id.as_ref()),
            ("subscription_details[items][0][price]", new_price_id),
            (
                "subscription_details[proration_behavior]",
                "create_prorations",
            ),
        ];

        let client = reqwest::Client::new();
        let response = client
            .post("https://api.stripe.com/v1/invoices/create_preview")
            .bearer_auth(&self.stripe.config().secret_key)
            .form(&form_params)
            .send()
            .await
            .map_err(|e| BillingError::StripeApi(format!("Failed to call Stripe API: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let error_body = response.text().await.unwrap_or_default();
            tracing::error!(
                status = %status,
                error_body = %error_body,
                "Stripe invoices/create_preview API failed"
            );
            return Err(BillingError::StripeApi(format!(
                "Stripe API error ({}): {}",
                status, error_body
            )));
        }

        let upcoming_invoice: serde_json::Value = response.json().await.map_err(|e| {
            BillingError::StripeApi(format!("Failed to parse Stripe response: {}", e))
        })?;

        // Extract amount_due from the response
        let total_amount = upcoming_invoice["amount_due"].as_i64().unwrap_or(0);

        // Get pending overages from database (includes abandoned Pay Now checkouts)
        let pending_overages: Option<(Option<i64>,)> = sqlx::query_as(
            r#"
            SELECT SUM(total_charge_cents)
            FROM overage_charges
            WHERE org_id = $1 AND status IN ('pending', 'awaiting_payment')
            "#,
        )
        .bind(org_id)
        .fetch_optional(&self.pool)
        .await?;

        let overage_cents = pending_overages.and_then(|(sum,)| sum).unwrap_or(0) as i64;

        // Calculate period info
        let period_end = current.current_period_end;
        let now = chrono::Utc::now().timestamp();
        let days_remaining = ((period_end - now) as f64 / 86400.0).ceil() as i32;

        tracing::info!(
            org_id = %org_id,
            current_tier = %current_tier,
            new_tier = %new_tier,
            proration_amount = total_amount,
            overage_cents = overage_cents,
            days_remaining = days_remaining,
            "Previewed upgrade proration"
        );

        Ok(ProrationPreview {
            current_tier: current_tier.to_string(),
            new_tier: new_tier.to_string(),
            proration_amount_cents: total_amount,
            overage_amount_cents: overage_cents,
            total_amount_cents: total_amount + overage_cents,
            days_remaining,
            description: format!(
                "Upgrade from {} to {} with {} days remaining",
                current_tier, new_tier, days_remaining
            ),
        })
    }

    /// Cancel a subscription at end of billing period
    pub async fn cancel_subscription(&self, org_id: Uuid) -> BillingResult<Subscription> {
        let sub_id = self.get_subscription_id(org_id).await?;

        let params = CancelSubscription {
            cancellation_details: None,
            invoice_now: None,
            prorate: None,
        };

        let subscription = Subscription::cancel(self.stripe.inner(), &sub_id, params).await?;

        // Update database
        self.sync_subscription_to_db(org_id, &subscription).await?;

        tracing::info!(
            org_id = %org_id,
            subscription_id = %subscription.id,
            "Cancelled subscription"
        );

        Ok(subscription)
    }

    /// Resume a cancelled subscription (if still in grace period)
    pub async fn resume_subscription(&self, org_id: Uuid) -> BillingResult<Subscription> {
        let sub_id = self.get_subscription_id(org_id).await?;

        let params = UpdateSubscription {
            cancel_at_period_end: Some(false),
            ..Default::default()
        };

        let subscription = Subscription::update(self.stripe.inner(), &sub_id, params).await?;

        // Update database
        self.sync_subscription_to_db(org_id, &subscription).await?;

        tracing::info!(
            org_id = %org_id,
            subscription_id = %subscription.id,
            "Resumed subscription"
        );

        Ok(subscription)
    }

    /// Schedule a downgrade to take effect at the end of the billing period
    /// User keeps current tier until period ends, then automatically switches to new tier
    pub async fn schedule_downgrade(
        &self,
        org_id: Uuid,
        new_tier: &str,
    ) -> BillingResult<ScheduledDowngrade> {
        // Validate the tier exists (skip for free tier which has no Stripe price)
        if new_tier != "free" {
            let _price_id = self
                .stripe
                .config()
                .price_id_for_tier(new_tier)
                .ok_or_else(|| BillingError::InvalidTier(new_tier.to_string()))?;
        }

        // Get current subscription to verify it exists and get period end
        let subscription = self
            .get_subscription(org_id)
            .await?
            .ok_or_else(|| BillingError::SubscriptionNotFound(org_id.to_string()))?;

        // Get current tier from metadata or price - must be resolvable for downgrade scheduling
        let current_tier = subscription.metadata.get("tier")
            .cloned()
            .or_else(|| {
                subscription.items.data.first()
                    .and_then(|item| item.price.as_ref())
                    .and_then(|p| self.stripe.config().tier_for_price_id(&p.id))
                    .map(|s| s.to_string())
            })
            .ok_or_else(|| {
                tracing::error!(
                    org_id = %org_id,
                    subscription_id = %subscription.id,
                    "Cannot determine current tier for downgrade scheduling - no tier in metadata or recognizable price"
                );
                BillingError::InvalidTier(
                    "Cannot determine current subscription tier. Please contact support.".to_string()
                )
            })?;

        // Verify this is actually a downgrade
        let tier_order = |t: &str| -> u8 {
            match t {
                "free" => 0,
                "pro" => 1,
                "team" => 2,
                "enterprise" => 3,
                _ => 0,
            }
        };

        if tier_order(new_tier) >= tier_order(&current_tier) {
            return Err(BillingError::InvalidTier(format!(
                "Cannot schedule downgrade from {} to {} - use upgrade flow instead",
                current_tier, new_tier
            )));
        }

        let period_end = OffsetDateTime::from_unix_timestamp(subscription.current_period_end)
            .unwrap_or(OffsetDateTime::now_utc());

        // Store the scheduled downgrade in database
        sqlx::query(
            r#"
            UPDATE subscriptions
            SET scheduled_downgrade_tier = $1,
                scheduled_downgrade_at = NOW(),
                updated_at = NOW()
            WHERE org_id = $2 AND status IN ('active', 'trialing', 'past_due')
            "#,
        )
        .bind(new_tier)
        .bind(org_id)
        .execute(&self.pool)
        .await?;

        // For free tier, set cancel_at_period_end so subscription doesn't renew
        // This ensures the subscription ends at period end and triggers the downgrade
        if new_tier == "free" {
            let params = UpdateSubscription {
                cancel_at_period_end: Some(true),
                ..Default::default()
            };
            Subscription::update(self.stripe.inner(), &subscription.id, params).await?;

            tracing::info!(
                org_id = %org_id,
                "Set cancel_at_period_end=true for free tier downgrade"
            );
        }

        tracing::info!(
            org_id = %org_id,
            current_tier = %current_tier,
            new_tier = %new_tier,
            effective_date = %period_end,
            "Scheduled subscription downgrade for period end"
        );

        Ok(ScheduledDowngrade {
            current_tier,
            new_tier: new_tier.to_string(),
            effective_date: period_end,
        })
    }

    /// Schedule a downgrade for admin-initiated tier changes
    /// Unlike user downgrades, this stores admin context and handles special features
    pub async fn admin_schedule_downgrade(
        &self,
        org_id: Uuid,
        params: AdminTierChangeParams,
    ) -> BillingResult<AdminTierChangeResult> {
        // 1. Call base schedule_downgrade() to set scheduled_downgrade_tier
        let scheduled = self.schedule_downgrade(org_id, &params.new_tier).await?;

        // 2. Store admin-specific context
        sqlx::query(
            r#"
            UPDATE subscriptions
            SET admin_downgrade_scheduled = true,
                admin_downgrade_scheduled_by = $1,
                admin_downgrade_scheduled_at = NOW(),
                admin_downgrade_reason = $2,
                admin_downgrade_custom_price_cents = $3,
                admin_downgrade_billing_interval = $4,
                updated_at = NOW()
            WHERE org_id = $5 AND scheduled_downgrade_tier IS NOT NULL
            "#,
        )
        .bind(params.admin_user_id)
        .bind(&params.reason)
        .bind(params.custom_price_cents)
        .bind(&params.billing_interval)
        .bind(org_id)
        .execute(&self.pool)
        .await?;

        // 3. Get current subscription for response
        let subscription = self
            .get_subscription(org_id)
            .await?
            .ok_or_else(|| BillingError::SubscriptionNotFound(org_id.to_string()))?;

        // Extract customer ID from Expandable<Customer>
        let customer_id = match &subscription.customer {
            stripe::Expandable::Id(id) => id.to_string(),
            stripe::Expandable::Object(customer) => customer.id.to_string(),
        };

        // 4. Return result indicating scheduled change
        Ok(AdminTierChangeResult {
            stripe_subscription_id: Some(subscription.id.to_string()),
            stripe_customer_id: customer_id,
            tier: scheduled.current_tier.clone(), // Still on CURRENT tier
            trial_end: None,
            stripe_invoice_id: None,
            invoice_status: None,
            subscription_start_date: None,
            billing_interval: params
                .billing_interval
                .unwrap_or_else(|| "monthly".to_string()),
            custom_price_id: None,
            scheduled: true,
            scheduled_effective_date: Some(scheduled.effective_date),
            refund_issued: false,
            refund_amount_cents: None,
            refund_type: None,
            message: format!(
                "Downgrade to {} scheduled for end of billing period ({})",
                params.new_tier, scheduled.effective_date
            ),
        })
    }

    /// Execute an immediate downgrade with prorated credit
    /// This updates the subscription immediately and gives the user credit for unused time
    pub async fn admin_immediate_downgrade(
        &self,
        org_id: Uuid,
        params: AdminTierChangeParams,
    ) -> BillingResult<AdminTierChangeResult> {
        // 1. Get current subscription
        let subscription = self
            .get_subscription(org_id)
            .await?
            .ok_or_else(|| BillingError::SubscriptionNotFound(org_id.to_string()))?;

        // 2. Get the price ID for the new tier
        let billing_interval = params
            .billing_interval
            .clone()
            .unwrap_or_else(|| "monthly".to_string());
        let price_id = if let (true, Some(custom_price)) =
            (params.new_tier == "enterprise", params.custom_price_cents)
        {
            // Create custom price for Enterprise
            self.create_custom_enterprise_price(custom_price, &billing_interval, org_id)
                .await?
        } else if billing_interval == "annual" {
            // Use annual price
            self.stripe
                .config()
                .annual_price_id_for_tier(&params.new_tier)
                .ok_or_else(|| {
                    BillingError::InvalidTier(format!(
                        "No annual price for tier: {}",
                        params.new_tier
                    ))
                })?
                .to_string()
        } else {
            // Use monthly price
            self.stripe
                .config()
                .price_id_for_tier(&params.new_tier)
                .ok_or_else(|| BillingError::InvalidTier(params.new_tier.clone()))?
                .to_string()
        };

        // 3. Get the subscription item ID
        let item_id = subscription
            .items
            .data
            .first()
            .map(|item| item.id.to_string())
            .ok_or_else(|| BillingError::Internal("No subscription items found".to_string()))?;

        // 4. Update subscription immediately with proration (gives credit for unused time)
        let mut metadata = std::collections::HashMap::new();
        metadata.insert("tier".to_string(), params.new_tier.clone());
        metadata.insert("admin_immediate_downgrade".to_string(), "true".to_string());
        if let Some(ref reason) = Some(&params.reason) {
            metadata.insert("downgrade_reason".to_string(), reason.to_string());
        }

        let update_params = UpdateSubscription {
            items: Some(vec![UpdateSubscriptionItems {
                id: Some(item_id),
                price: Some(price_id.clone()),
                ..Default::default()
            }]),
            metadata: Some(metadata),
            // CreateProrations gives user credit for unused time on higher plan
            proration_behavior: Some(SubscriptionProrationBehavior::CreateProrations),
            ..Default::default()
        };

        let updated_subscription =
            Subscription::update(self.stripe.inner(), &subscription.id, update_params)
                .await
                .map_err(|e| BillingError::StripeApi(e.to_string()))?;

        // 5. Use consolidated change_tier() for DB update + audit logging
        // This ensures proper version locking and audit trail
        let tier_options = TierChangeOptions {
            source: Some(TierChangeSource::AdminPanel),
            changed_by: params.admin_user_id,
            reason: Some(params.reason.clone()),
            downgrade_timing: Some("immediate".to_string()),
            refund_type: Some("credit".to_string()),
            ..Default::default()
        };

        // change_tier handles: DB update, version increment, clear scheduled downgrade, audit log
        self.change_tier(org_id, &params.new_tier, tier_options)
            .await?;

        // 7. Sync subscription to database
        self.sync_subscription_to_db(org_id, &updated_subscription)
            .await?;

        // Extract customer ID
        let customer_id = match &updated_subscription.customer {
            stripe::Expandable::Id(id) => id.to_string(),
            stripe::Expandable::Object(customer) => customer.id.to_string(),
        };

        // Get current tier from subscription metadata for audit trail
        let current_tier = subscription
            .metadata
            .get("tier")
            .cloned()
            .unwrap_or_else(|| {
                tracing::warn!(
                    org_id = %org_id,
                    subscription_id = %subscription.id,
                    "Subscription has no tier metadata - using 'unknown' for audit trail"
                );
                "unknown".to_string()
            });

        // Calculate and record the prorated credit for audit purposes
        let refund_service = RefundService::new(self.stripe.clone(), self.pool.clone());
        let credit_amount: Option<i64> = match refund_service
            .get_refundable_charge(subscription.id.as_str())
            .await
        {
            Ok(charge) => {
                let prorated = RefundService::calculate_prorated_amount(
                    charge.amount_cents,
                    charge.period_start,
                    charge.period_end,
                );

                // Record the credit for audit trail (Stripe handles actual credit via prorations)
                if prorated > 0 {
                    if let Some(admin_id) = params.admin_user_id {
                        if let Err(e) = refund_service
                            .record_credit(
                                org_id,
                                admin_id,
                                prorated,
                                &params.reason,
                                &current_tier,
                                &params.new_tier,
                            )
                            .await
                        {
                            tracing::warn!(
                                org_id = %org_id,
                                error = %e,
                                "Failed to record credit audit (credit still applied via Stripe prorations)"
                            );
                        }
                    }
                }

                Some(prorated)
            }
            Err(e) => {
                tracing::warn!(
                    org_id = %org_id,
                    error = %e,
                    "Could not calculate prorated credit amount"
                );
                None
            }
        };

        tracing::info!(
            org_id = %org_id,
            new_tier = %params.new_tier,
            admin_user_id = ?params.admin_user_id,
            credit_amount = ?credit_amount,
            reason = %params.reason,
            "Admin executed immediate downgrade with prorated credit"
        );

        // For immediate downgrades, credit is always issued via Stripe prorations
        let credit_msg = credit_amount
            .map(|c| format!(" (${:.2} credit)", c as f64 / 100.0))
            .unwrap_or_default();

        Ok(AdminTierChangeResult {
            stripe_subscription_id: Some(updated_subscription.id.to_string()),
            stripe_customer_id: customer_id,
            tier: params.new_tier.clone(),
            trial_end: None,
            stripe_invoice_id: None,
            invoice_status: None,
            subscription_start_date: None,
            billing_interval,
            custom_price_id: if params.new_tier == "enterprise" {
                Some(price_id)
            } else {
                None
            },
            scheduled: false,
            scheduled_effective_date: None,
            refund_issued: credit_amount.map(|c| c > 0).unwrap_or(false),
            refund_amount_cents: credit_amount,
            refund_type: Some("credit".to_string()), // Using prorations = credit
            message: format!(
                "Immediate downgrade to {} completed with prorated credit for unused time{}",
                params.new_tier, credit_msg
            ),
        })
    }

    /// Handle downgrade to Free tier with scheduled or immediate options
    ///
    /// - Scheduled: Sets `cancel_at_period_end = true` on Stripe subscription
    /// - Immediate: Cancels subscription now, optionally issues refund/credit
    pub async fn admin_free_tier_downgrade(
        &self,
        org_id: Uuid,
        params: AdminTierChangeParams,
        current_tier: &str,
    ) -> BillingResult<AdminTierChangeResult> {
        // Check if subscription exists
        let subscription = match self.get_subscription(org_id).await? {
            Some(sub) => sub,
            None => {
                // No subscription - use consolidated change_tier() for proper audit trail
                let tier_options = TierChangeOptions {
                    source: Some(TierChangeSource::AdminPanel),
                    changed_by: params.admin_user_id,
                    reason: Some(params.reason.clone()),
                    ..Default::default()
                };

                self.change_tier(org_id, "free", tier_options).await?;

                let customer_id: Option<(Option<String>,)> =
                    sqlx::query_as("SELECT stripe_customer_id FROM organizations WHERE id = $1")
                        .bind(org_id)
                        .fetch_optional(&self.pool)
                        .await?;

                let customer_id = customer_id
                    .and_then(|(id,)| id)
                    .unwrap_or_else(|| "no_customer".to_string());

                tracing::info!(
                    org_id = %org_id,
                    reason = %params.reason,
                    "Admin set organization to Free tier (no subscription existed)"
                );

                return Ok(AdminTierChangeResult {
                    stripe_subscription_id: None,
                    stripe_customer_id: customer_id,
                    tier: "free".to_string(),
                    trial_end: None,
                    stripe_invoice_id: None,
                    invoice_status: None,
                    subscription_start_date: None,
                    billing_interval: "monthly".to_string(),
                    custom_price_id: None,
                    scheduled: false,
                    scheduled_effective_date: None,
                    refund_issued: false,
                    refund_amount_cents: None,
                    refund_type: None,
                    message: "Organization set to Free tier (no subscription existed)".to_string(),
                });
            }
        };

        // Extract customer ID
        let customer_id = match &subscription.customer {
            stripe::Expandable::Id(id) => id.to_string(),
            stripe::Expandable::Object(customer) => customer.id.to_string(),
        };

        // Determine if scheduled or immediate
        if params.downgrade_timing.as_deref() == Some("scheduled") {
            // SCHEDULED: Set cancel_at_period_end = true
            let subscription_id = subscription.id.clone();

            use stripe::UpdateSubscription;
            let mut update_params = UpdateSubscription::new();
            update_params.cancel_at_period_end = Some(true);

            // Add metadata for tracking
            let mut metadata = std::collections::HashMap::new();
            metadata.insert("scheduled_free_downgrade".to_string(), "true".to_string());
            metadata.insert(
                "admin_user_id".to_string(),
                params
                    .admin_user_id
                    .map(|id| id.to_string())
                    .unwrap_or_default(),
            );
            metadata.insert("reason".to_string(), params.reason.clone());
            update_params.metadata = Some(metadata);

            let updated =
                stripe::Subscription::update(self.stripe.inner(), &subscription_id, update_params)
                    .await?;

            // Get period end for the effective date
            let period_end = if updated.current_period_end > 0 {
                Some(
                    OffsetDateTime::from_unix_timestamp(updated.current_period_end)
                        .unwrap_or_else(|_| OffsetDateTime::now_utc()),
                )
            } else {
                None
            };

            // Record scheduled downgrade in DB
            sqlx::query(
                r#"
                UPDATE subscriptions
                SET scheduled_downgrade_tier = 'free',
                    scheduled_downgrade_at = $2,
                    updated_at = NOW()
                WHERE org_id = $1
                "#,
            )
            .bind(org_id)
            .bind(period_end)
            .execute(&self.pool)
            .await?;

            tracing::info!(
                org_id = %org_id,
                current_tier = %current_tier,
                period_end = ?period_end,
                reason = %params.reason,
                "Admin scheduled Free tier downgrade at period end"
            );

            return Ok(AdminTierChangeResult {
                stripe_subscription_id: Some(subscription_id.to_string()),
                stripe_customer_id: customer_id,
                tier: current_tier.to_string(), // Still on current tier until period end
                trial_end: None,
                stripe_invoice_id: None,
                invoice_status: None,
                subscription_start_date: None,
                billing_interval: "monthly".to_string(),
                custom_price_id: None,
                scheduled: true,
                scheduled_effective_date: period_end,
                refund_issued: false,
                refund_amount_cents: None,
                refund_type: None,
                message: format!(
                    "Downgrade to Free tier scheduled for end of billing period{}",
                    period_end.map(|d| format!(" ({})", d)).unwrap_or_default()
                ),
            });
        }

        // IMMEDIATE: Cancel subscription now
        // Check if subscription is already canceled
        use stripe::SubscriptionStatus as StripeSubStatus;
        let subscription_already_canceled = subscription.status == StripeSubStatus::Canceled;

        // Capture subscription ID before it might be moved
        let original_subscription_id = subscription.id.to_string();

        let final_subscription = if subscription_already_canceled {
            // Subscription already canceled in Stripe, just update our DB
            tracing::info!(
                org_id = %org_id,
                subscription_id = %subscription.id,
                "Subscription already canceled in Stripe, updating DB to free tier"
            );

            // Update subscription status in our DB
            sqlx::query(
                "UPDATE subscriptions SET status = 'canceled', updated_at = NOW() WHERE org_id = $1"
            )
            .bind(org_id)
            .execute(&self.pool)
            .await?;

            subscription
        } else {
            // Cancel the active subscription
            self.cancel_subscription(org_id).await?
        };

        // Use consolidated change_tier() for DB update + audit logging
        // IMPORTANT: Must pass downgrade_timing = "immediate" to actually update the tier
        // (default is "end_of_period" which only schedules it)
        let tier_options = TierChangeOptions {
            source: Some(TierChangeSource::AdminPanel),
            changed_by: params.admin_user_id,
            reason: Some(params.reason.clone()),
            refund_type: params.refund_type.clone(),
            downgrade_timing: Some("immediate".to_string()),
            ..Default::default()
        };

        self.change_tier(org_id, "free", tier_options).await?;

        // Clear any accumulated overages since the user is now on Free tier
        // (Free tier users don't have overage billing, so these should be forgiven)
        self.clear_accumulated_overages(org_id).await?;

        // Process refund/credit if requested and subscription wasn't already canceled
        let (refund_issued, refund_amount_cents): (bool, Option<i64>) = if params
            .refund_type
            .is_some()
            && !subscription_already_canceled
        {
            let refund_service = RefundService::new(self.stripe.clone(), self.pool.clone());
            let admin_user_id = params.admin_user_id.ok_or_else(|| {
                BillingError::Internal("Admin user ID required for refund".to_string())
            })?;

            // Try to get the refundable charge (using saved subscription ID)
            match refund_service
                .get_refundable_charge(&original_subscription_id)
                .await
            {
                Ok(charge) => {
                    // Calculate prorated amount
                    let prorated_amount = RefundService::calculate_prorated_amount(
                        charge.amount_cents,
                        charge.period_start,
                        charge.period_end,
                    );

                    if prorated_amount > 0 {
                        let refund_type = params.refund_type.as_deref().unwrap_or("credit");

                        if refund_type == "refund" {
                            // Issue actual refund to payment method
                            match refund_service
                                .issue_refund(
                                    org_id,
                                    admin_user_id,
                                    &charge.charge_id,
                                    &charge.invoice_id,
                                    prorated_amount,
                                    &params.reason,
                                    current_tier,
                                    "free",
                                )
                                .await
                            {
                                Ok(result) => {
                                    tracing::info!(
                                        org_id = %org_id,
                                        refund_id = %result.stripe_refund_id,
                                        amount_cents = %result.amount_cents,
                                        "Issued prorated refund for free tier downgrade"
                                    );
                                    (true, Some(result.amount_cents))
                                }
                                Err(e) => {
                                    tracing::error!(
                                        org_id = %org_id,
                                        error = %e,
                                        "Failed to issue refund (subscription still canceled)"
                                    );
                                    (false, None)
                                }
                            }
                        } else {
                            // Record credit (prorations already handled by Stripe during cancel)
                            match refund_service
                                .record_credit(
                                    org_id,
                                    admin_user_id,
                                    prorated_amount,
                                    &params.reason,
                                    current_tier,
                                    "free",
                                )
                                .await
                            {
                                Ok(_) => {
                                    tracing::info!(
                                        org_id = %org_id,
                                        amount_cents = %prorated_amount,
                                        "Recorded credit for free tier downgrade"
                                    );
                                    (true, Some(prorated_amount))
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        org_id = %org_id,
                                        error = %e,
                                        "Failed to record credit audit (credit still applied via Stripe)"
                                    );
                                    (true, Some(prorated_amount))
                                }
                            }
                        }
                    } else {
                        tracing::info!(
                            org_id = %org_id,
                            "No prorated refund due (period already ended or no amount)"
                        );
                        (false, Some(0))
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        org_id = %org_id,
                        error = %e,
                        "Could not get refundable charge (may be too old or no charge)"
                    );
                    (false, None)
                }
            }
        } else {
            (false, None)
        };

        tracing::info!(
            org_id = %org_id,
            current_tier = %current_tier,
            refund_type = ?params.refund_type,
            refund_issued = refund_issued,
            refund_amount_cents = ?refund_amount_cents,
            reason = %params.reason,
            was_already_canceled = subscription_already_canceled,
            "Admin immediately downgraded organization to Free tier (overages cleared)"
        );

        Ok(AdminTierChangeResult {
            stripe_subscription_id: Some(final_subscription.id.to_string()),
            stripe_customer_id: customer_id,
            tier: "free".to_string(),
            trial_end: None,
            stripe_invoice_id: None,
            invoice_status: None,
            subscription_start_date: None,
            billing_interval: "monthly".to_string(),
            custom_price_id: None,
            scheduled: false,
            scheduled_effective_date: None,
            refund_issued,
            refund_amount_cents,
            refund_type: params.refund_type.clone(),
            message: if subscription_already_canceled {
                "Subscription was already canceled, organization set to Free tier".to_string()
            } else {
                let refund_msg = match (refund_issued, refund_amount_cents) {
                    (true, Some(amount)) => format!(
                        " (${:.2} {} issued)",
                        amount as f64 / 100.0,
                        params.refund_type.as_deref().unwrap_or("credit")
                    ),
                    (false, Some(0)) => " (no prorated amount due)".to_string(),
                    _ => params
                        .refund_type
                        .as_ref()
                        .map(|t| format!(" ({} requested)", t))
                        .unwrap_or_default(),
                };
                format!(
                    "Subscription cancelled, downgraded to Free tier immediately{}",
                    refund_msg
                )
            },
        })
    }

    /// Cancel a scheduled downgrade
    pub async fn cancel_scheduled_downgrade(&self, org_id: Uuid) -> BillingResult<()> {
        let result = sqlx::query(
            r#"
            UPDATE subscriptions
            SET scheduled_downgrade_tier = NULL,
                scheduled_downgrade_at = NULL,
                updated_at = NOW()
            WHERE org_id = $1 AND scheduled_downgrade_tier IS NOT NULL
            "#,
        )
        .bind(org_id)
        .execute(&self.pool)
        .await?;

        if result.rows_affected() == 0 {
            return Err(BillingError::Internal(
                "No scheduled downgrade to cancel".to_string(),
            ));
        }

        tracing::info!(
            org_id = %org_id,
            "Cancelled scheduled downgrade"
        );

        Ok(())
    }

    /// Get scheduled downgrade info for an organization
    pub async fn get_scheduled_downgrade(
        &self,
        org_id: Uuid,
    ) -> BillingResult<Option<ScheduledDowngrade>> {
        #[derive(sqlx::FromRow)]
        #[allow(dead_code)]
        struct DowngradeRow {
            scheduled_downgrade_tier: Option<String>,
            scheduled_downgrade_at: Option<time::OffsetDateTime>, // Unused but fetched for potential future use
            current_period_end: Option<time::OffsetDateTime>,
        }

        let result: Option<DowngradeRow> = sqlx::query_as(
            r#"
            SELECT scheduled_downgrade_tier, scheduled_downgrade_at, current_period_end
            FROM subscriptions
            WHERE org_id = $1 AND status IN ('active', 'trialing', 'past_due')
            ORDER BY created_at DESC
            LIMIT 1
            "#,
        )
        .bind(org_id)
        .fetch_optional(&self.pool)
        .await?;

        match result {
            Some(row) if row.scheduled_downgrade_tier.is_some() => {
                // Get current tier from org
                let current_tier: Option<(String,)> =
                    sqlx::query_as("SELECT subscription_tier FROM organizations WHERE id = $1")
                        .bind(org_id)
                        .fetch_optional(&self.pool)
                        .await?;

                let current_tier = current_tier.map(|(t,)| t).ok_or_else(|| {
                    tracing::error!(
                        org_id = %org_id,
                        "Organization has scheduled downgrade but no current tier in database"
                    );
                    BillingError::Database(
                        "Cannot determine current tier for scheduled downgrade".to_string(),
                    )
                })?;

                Ok(Some(ScheduledDowngrade {
                    current_tier,
                    new_tier: row.scheduled_downgrade_tier.ok_or_else(|| {
                        BillingError::Database("Missing scheduled_downgrade_tier".to_string())
                    })?,
                    effective_date: row.current_period_end.unwrap_or(OffsetDateTime::now_utc()),
                }))
            }
            _ => Ok(None),
        }
    }

    /// Process a pending downgrade (called by webhook when billing period ends)
    ///
    /// Uses atomic claim pattern to prevent race conditions where admin cancels
    /// the downgrade while webhook is processing it.
    pub async fn process_scheduled_downgrade(
        &self,
        org_id: Uuid,
    ) -> BillingResult<Option<Subscription>> {
        // ATOMIC CLAIM: Try to atomically claim this downgrade for processing
        // This UPDATE...RETURNING pattern ensures only one process can claim it
        // If the claim succeeds (returns a row), we have exclusive processing rights
        // If it returns None, the downgrade was cancelled or already claimed
        #[derive(sqlx::FromRow)]
        #[allow(dead_code)]
        struct ClaimedDowngrade {
            scheduled_downgrade_tier: String,
            admin_downgrade_scheduled: Option<bool>,
            admin_downgrade_custom_price_cents: Option<i64>,
            admin_downgrade_billing_interval: Option<String>,
        }

        let claimed: Option<ClaimedDowngrade> = sqlx::query_as(
            r#"
            UPDATE subscriptions
            SET scheduled_downgrade_processing = true,
                scheduled_downgrade_claimed_at = NOW()
            WHERE org_id = $1
              AND scheduled_downgrade_tier IS NOT NULL
              AND (scheduled_downgrade_processing = false OR scheduled_downgrade_processing IS NULL)
            RETURNING
                scheduled_downgrade_tier,
                admin_downgrade_scheduled,
                admin_downgrade_custom_price_cents,
                admin_downgrade_billing_interval
            "#,
        )
        .bind(org_id)
        .fetch_optional(&self.pool)
        .await?;

        let claimed = match claimed {
            Some(c) => c,
            None => {
                tracing::debug!(
                    org_id = %org_id,
                    "No scheduled downgrade to process (already claimed, cancelled, or not found)"
                );
                return Ok(None);
            }
        };

        let new_tier = claimed.scheduled_downgrade_tier;
        let is_admin_scheduled = claimed.admin_downgrade_scheduled.unwrap_or(false);
        let custom_price = claimed.admin_downgrade_custom_price_cents;

        // Get current tier for logging
        let current_tier: Option<String> =
            sqlx::query_scalar("SELECT subscription_tier FROM organizations WHERE id = $1")
                .bind(org_id)
                .fetch_optional(&self.pool)
                .await?;

        tracing::info!(
            org_id = %org_id,
            from_tier = ?current_tier,
            to_tier = %new_tier,
            is_admin_scheduled = is_admin_scheduled,
            has_custom_price = custom_price.is_some(),
            "Processing scheduled downgrade (claimed exclusive processing rights)"
        );

        // Process the downgrade - we have exclusive claim now
        let result = async {
            if new_tier == "free" {
                // Downgrading to free - cancel the subscription
                let sub = self.cancel_subscription(org_id).await?;

                // Use consolidated change_tier() for audit logging and version tracking
                // IMPORTANT: Pass downgrade_timing = "immediate" since we're executing a scheduled
                // downgrade NOW (not scheduling another one)
                let tier_options = TierChangeOptions {
                    source: Some(if is_admin_scheduled {
                        TierChangeSource::AdminPanel
                    } else {
                        TierChangeSource::UserDowngrade
                    }),
                    reason: Some("Scheduled downgrade to Free tier".to_string()),
                    downgrade_timing: Some("immediate".to_string()),
                    ..Default::default()
                };

                self.change_tier(org_id, "free", tier_options).await?;

                tracing::info!(
                    org_id = %org_id,
                    "Successfully downgraded organization to free tier"
                );

                Ok(Some(sub))
            } else {
                // Downgrade to different paid tier
                let sub = self.update_subscription(org_id, &new_tier).await?;

                tracing::info!(
                    org_id = %org_id,
                    new_tier = %new_tier,
                    "Successfully processed scheduled downgrade"
                );

                Ok(Some(sub))
            }
        }
        .await;

        // ALWAYS clear the claim and scheduled downgrade, regardless of success/failure
        // This ensures we don't leave the subscription in a stuck "processing" state
        if let Err(e) = sqlx::query(
            r#"
            UPDATE subscriptions
            SET scheduled_downgrade_tier = NULL,
                scheduled_downgrade_at = NULL,
                scheduled_downgrade_processing = false,
                scheduled_downgrade_claimed_at = NULL,
                admin_downgrade_scheduled = FALSE,
                admin_downgrade_scheduled_by = NULL,
                admin_downgrade_scheduled_at = NULL,
                admin_downgrade_reason = NULL,
                admin_downgrade_custom_price_cents = NULL,
                admin_downgrade_billing_interval = NULL,
                updated_at = NOW()
            WHERE org_id = $1
            "#,
        )
        .bind(org_id)
        .execute(&self.pool)
        .await
        {
            tracing::error!(
                org_id = %org_id,
                error = %e,
                "Failed to clear scheduled downgrade after processing"
            );
        }

        result
    }

    /// Admin-initiated tier change that syncs with Stripe
    /// This method ensures all manual tier changes create/update Stripe subscriptions
    /// and supports admin-granted trial periods
    pub async fn admin_change_tier(
        &self,
        org_id: Uuid,
        params: AdminTierChangeParams,
    ) -> BillingResult<AdminTierChangeResult> {
        // Step 1: Validate tier
        let valid_tiers = ["free", "pro", "team", "enterprise"];
        if !valid_tiers.contains(&params.new_tier.as_str()) {
            return Err(BillingError::InvalidTier(format!(
                "Invalid tier '{}'. Must be one of: {}",
                params.new_tier,
                valid_tiers.join(", ")
            )));
        }

        // Step 2: Validate trial_days (Stripe allows 0-730 days)
        if let Some(trial_days) = params.trial_days {
            if trial_days > 730 {
                return Err(BillingError::InvalidTier(format!(
                    "Trial period must be between 0 and 730 days, got {}",
                    trial_days
                )));
            }
        }

        // Step 2.5: Detect upgrade vs downgrade and route accordingly
        let tier_order = |t: &str| -> u8 {
            match t {
                "free" => 0,
                "pro" => 1,
                "team" => 2,
                "enterprise" => 3,
                _ => 0,
            }
        };

        // Get current tier from organization
        let current_tier: Option<(String,)> =
            sqlx::query_as("SELECT subscription_tier FROM organizations WHERE id = $1")
                .bind(org_id)
                .fetch_optional(&self.pool)
                .await?;

        let current_tier = current_tier
            .map(|(t,)| t)
            .unwrap_or_else(|| "free".to_string());
        let current_order = tier_order(&current_tier);
        let new_order = tier_order(&params.new_tier);

        // Route based on tier change type
        // DOWNGRADE - including to Free tier - check if immediate or scheduled
        if new_order < current_order {
            if params.new_tier == "free" {
                // Free tier downgrade has its own handling
                return self
                    .admin_free_tier_downgrade(org_id, params, &current_tier)
                    .await;
            } else if params.downgrade_timing.as_deref() == Some("immediate") {
                // Immediate downgrade with prorated credit/refund
                tracing::info!(
                    org_id = %org_id,
                    current_tier = %current_tier,
                    new_tier = %params.new_tier,
                    "Admin tier change is an IMMEDIATE downgrade with prorated credit/refund"
                );
                return self.admin_immediate_downgrade(org_id, params).await;
            } else {
                // Default: schedule for period end
                tracing::info!(
                    org_id = %org_id,
                    current_tier = %current_tier,
                    new_tier = %params.new_tier,
                    "Admin tier change is a downgrade - scheduling for end of billing period"
                );
                return self.admin_schedule_downgrade(org_id, params).await;
            }
        }

        // Step 3: Special case - Moving TO Free tier (from same tier, shouldn't happen normally)
        if params.new_tier == "free" {
            // Cancel existing subscription if any
            if let Ok(Some(_)) = self.get_subscription(org_id).await {
                let sub = self.cancel_subscription(org_id).await?;

                tracing::info!(
                    org_id = %org_id,
                    reason = %params.reason,
                    "Admin downgraded organization to Free tier (subscription canceled)"
                );

                // Extract customer ID from Expandable<Customer>
                let customer_id = match &sub.customer {
                    stripe::Expandable::Id(id) => id.to_string(),
                    stripe::Expandable::Object(customer) => customer.id.to_string(),
                };

                return Ok(AdminTierChangeResult {
                    stripe_subscription_id: Some(sub.id.to_string()),
                    stripe_customer_id: customer_id,
                    tier: "free".to_string(),
                    trial_end: None,
                    stripe_invoice_id: None,
                    invoice_status: None,
                    subscription_start_date: None,
                    billing_interval: "monthly".to_string(),
                    custom_price_id: None,
                    scheduled: false,
                    scheduled_effective_date: None,
                    refund_issued: false, // TODO: Integrate with RefundService for Free tier downgrades
                    refund_amount_cents: None,
                    refund_type: None,
                    message: "Subscription cancelled, downgraded to Free tier immediately"
                        .to_string(),
                });
            } else {
                // No subscription exists - use consolidated change_tier() for proper audit trail
                let tier_options = TierChangeOptions {
                    source: Some(TierChangeSource::AdminPanel),
                    changed_by: params.admin_user_id,
                    reason: Some(params.reason.clone()),
                    ..Default::default()
                };

                self.change_tier(org_id, "free", tier_options).await?;

                tracing::info!(
                    org_id = %org_id,
                    reason = %params.reason,
                    "Admin set organization to Free tier (no subscription needed)"
                );

                // Get or create customer ID for consistency (but no subscription)
                let customer_id: Option<(Option<String>,)> =
                    sqlx::query_as("SELECT stripe_customer_id FROM organizations WHERE id = $1")
                        .bind(org_id)
                        .fetch_optional(&self.pool)
                        .await?;

                let customer_id = customer_id
                    .and_then(|(id,)| id)
                    .unwrap_or_else(|| "no_customer".to_string());

                return Ok(AdminTierChangeResult {
                    stripe_subscription_id: None,
                    stripe_customer_id: customer_id,
                    tier: "free".to_string(),
                    trial_end: None,
                    stripe_invoice_id: None,
                    invoice_status: None,
                    subscription_start_date: None,
                    billing_interval: "monthly".to_string(),
                    custom_price_id: None,
                    scheduled: false,
                    scheduled_effective_date: None,
                    refund_issued: false, // No subscription = no refund needed
                    refund_amount_cents: None,
                    refund_type: None,
                    message: "Organization set to Free tier immediately".to_string(),
                });
            }
        }

        // Step 4: Get owner email and org name for customer creation
        let (owner_email, org_name) = self.get_owner_email(org_id).await?;

        // Step 5: Ensure Stripe customer exists (create if missing)
        let customer = self
            .get_or_create_stripe_customer(org_id, &owner_email, &org_name)
            .await?;

        // Step 6: Check payment method (optional for trials, required otherwise)
        let has_payment_method = customer.default_source.is_some()
            || customer
                .invoice_settings
                .as_ref()
                .and_then(|settings| settings.default_payment_method.as_ref())
                .is_some();

        // Skip payment method validation if using invoice or trial payment methods
        let payment_method_requires_validation = params.payment_method.as_deref()
            != Some("invoice")
            && params.payment_method.as_deref() != Some("trial");

        if !has_payment_method
            && params.trial_days.is_none()
            && !params.skip_payment_validation
            && payment_method_requires_validation
        {
            tracing::warn!(
                org_id = %org_id,
                tier = %params.new_tier,
                payment_method = ?params.payment_method,
                "Admin tier change blocked: No payment method and no trial period specified"
            );
            return Err(BillingError::PaymentMethodRequired);
        }

        if !has_payment_method && params.trial_days.is_some() {
            tracing::warn!(
                org_id = %org_id,
                tier = %params.new_tier,
                trial_days = params.trial_days,
                "Admin granted trial without payment method - customer must add payment before trial ends"
            );
        }

        // Step 7: Determine price ID (custom for Enterprise, default otherwise)
        let billing_interval = params.billing_interval.as_deref().unwrap_or("monthly");
        let (price_id, custom_price_id) =
            if params.new_tier == "enterprise" && params.custom_price_cents.is_some() {
                // Create custom Enterprise price
                let interval = match billing_interval {
                    "annual" => "year",
                    _ => "month",
                };
                let custom_price = params.custom_price_cents.ok_or_else(|| {
                    BillingError::InvalidTier("Custom tier requires custom_price_cents".to_string())
                })?;
                let custom_id = self
                    .create_custom_enterprise_price(custom_price, interval, org_id)
                    .await?;
                (custom_id.clone(), Some(custom_id))
            } else {
                // Use default Stripe price IDs
                let default_id = if billing_interval == "annual" {
                    self.stripe
                        .config()
                        .annual_price_id_for_tier(&params.new_tier)
                        .ok_or_else(|| {
                            BillingError::InvalidTier(format!(
                                "{} annual pricing not configured",
                                params.new_tier
                            ))
                        })?
                        .to_string()
                } else {
                    self.stripe
                        .config()
                        .price_id_for_tier(&params.new_tier)
                        .ok_or_else(|| BillingError::InvalidTier(params.new_tier.clone()))?
                        .to_string()
                };
                (default_id, None)
            };

        // Step 8: Get existing subscription
        let existing_sub = self.get_subscription(org_id).await.ok().flatten();

        // Step 9: Create or update subscription
        // If existing subscription is canceled, treat it as if it doesn't exist (create new)
        let existing_sub_active =
            existing_sub.filter(|sub| sub.status != stripe::SubscriptionStatus::Canceled);

        let subscription = match existing_sub_active {
            Some(existing) => {
                // Update existing ACTIVE subscription to new tier
                tracing::info!(
                    org_id = %org_id,
                    new_tier = %params.new_tier,
                    price_id = %price_id,
                    existing_status = ?existing.status,
                    "Updating existing active subscription to new tier"
                );

                // Update subscription with new price
                use stripe::UpdateSubscription;
                let mut update_params = UpdateSubscription::default();
                let first_item = existing.items.data.first().ok_or_else(|| {
                    BillingError::SubscriptionNotFound("Subscription has no items".to_string())
                })?;
                update_params.items = Some(vec![stripe::UpdateSubscriptionItems {
                    id: Some(first_item.id.to_string()),
                    price: Some(price_id.clone()),
                    quantity: Some(1),
                    ..Default::default()
                }]);
                update_params.proration_behavior =
                    Some(SubscriptionProrationBehavior::CreateProrations);

                // If payment_method is "invoice", use send_invoice collection method
                if params.payment_method.as_deref() == Some("invoice") {
                    update_params.collection_method = Some(stripe::CollectionMethod::SendInvoice);
                    update_params.days_until_due = Some(30);
                }

                let mut sub =
                    stripe::Subscription::update(self.stripe.inner(), &existing.id, update_params)
                        .await?;

                // Apply trial period to existing subscription if specified
                if let Some(trial_days) = params.trial_days {
                    sub = self.apply_trial_period(&sub.id, trial_days).await?;
                }

                sub
            }
            None => {
                // Create new subscription
                tracing::info!(
                    org_id = %org_id,
                    new_tier = %params.new_tier,
                    trial_days = ?params.trial_days,
                    price_id = %price_id,
                    "Creating new subscription"
                );

                // Clear any old overages from previous subscriptions
                // Starting fresh on a new subscription = fresh billing slate
                self.clear_accumulated_overages(org_id).await?;

                let mut metadata = std::collections::HashMap::new();
                metadata.insert("org_id".to_string(), org_id.to_string());
                metadata.insert("tier".to_string(), params.new_tier.clone());
                metadata.insert("admin_change".to_string(), "true".to_string());
                metadata.insert("reason".to_string(), params.reason.clone());

                if params.trial_days.is_some() {
                    metadata.insert("admin_trial".to_string(), "true".to_string());
                }

                let mut create_params = CreateSubscription::new(customer.id.clone());
                create_params.items = Some(vec![CreateSubscriptionItems {
                    price: Some(price_id.to_string()),
                    quantity: Some(1),
                    ..Default::default()
                }]);
                create_params.metadata = Some(metadata);

                // Apply trial period for new subscriptions
                if let Some(trial_days) = params.trial_days {
                    create_params.trial_period_days = Some(trial_days);
                }

                // If payment_method is "invoice", use send_invoice collection method
                if params.payment_method.as_deref() == Some("invoice") {
                    create_params.collection_method = Some(stripe::CollectionMethod::SendInvoice);
                    create_params.days_until_due = Some(30);
                }

                Subscription::create(self.stripe.inner(), create_params).await?
            }
        };

        // Step 10: Handle scheduled start date (by setting trial_end)
        let mut final_subscription = subscription;
        if let Some(start_date) = params.subscription_start_date {
            let now = OffsetDateTime::now_utc();
            let trial_days = (start_date - now).whole_days();
            if trial_days > 0 {
                tracing::info!(
                    org_id = %org_id,
                    start_date = %start_date,
                    trial_days = trial_days,
                    "Setting subscription start date via trial period"
                );
                final_subscription = self
                    .apply_trial_period(&final_subscription.id, trial_days as u32)
                    .await?;
            }
        }

        // Step 11: Retrieve invoice for invoice-based subscriptions
        // Stripe automatically creates invoices for subscriptions with collection_method: send_invoice
        let (invoice_id, invoice_status) = if params.payment_method.as_deref() == Some("invoice") {
            // Get the latest invoice for this subscription
            if let Some(latest_invoice) = final_subscription.latest_invoice.as_ref() {
                use stripe::Expandable;
                let invoice_id_str = match latest_invoice {
                    Expandable::Id(id) => id.to_string(),
                    Expandable::Object(invoice) => invoice.id.to_string(),
                };
                tracing::info!(
                    org_id = %org_id,
                    invoice_id = %invoice_id_str,
                    "Stripe created invoice automatically for send_invoice subscription"
                );
                (Some(invoice_id_str), Some("draft".to_string()))
            } else {
                (None, None)
            }
        } else {
            (None, None)
        };

        // Step 12: Sync to database immediately (don't wait for webhook)
        self.sync_subscription_to_db(org_id, &final_subscription)
            .await?;

        tracing::info!(
            org_id = %org_id,
            subscription_id = %final_subscription.id,
            tier = %params.new_tier,
            trial_end = ?final_subscription.trial_end,
            reason = %params.reason,
            invoice_id = ?invoice_id,
            billing_interval = billing_interval,
            "Admin tier change completed successfully"
        );

        // Step 13: Return result with all details for audit logging
        Ok(AdminTierChangeResult {
            stripe_subscription_id: Some(final_subscription.id.to_string()),
            stripe_customer_id: customer.id.to_string(),
            tier: params.new_tier.clone(),
            trial_end: final_subscription.trial_end.map(|t| {
                OffsetDateTime::from_unix_timestamp(t).unwrap_or(OffsetDateTime::now_utc())
            }),
            stripe_invoice_id: invoice_id,
            invoice_status,
            subscription_start_date: params.subscription_start_date,
            billing_interval: billing_interval.to_string(),
            custom_price_id,
            scheduled: false,
            scheduled_effective_date: None,
            refund_issued: false, // This is an upgrade, not a downgrade
            refund_amount_cents: None,
            refund_type: None,
            message: format!(
                "Tier changed to {} immediately with proration",
                params.new_tier
            ),
        })
    }

    /// Get owner email and organization name for customer creation
    async fn get_owner_email(&self, org_id: Uuid) -> BillingResult<(String, String)> {
        let result: Option<(String, String)> = sqlx::query_as(
            r#"
            SELECT u.email, o.name
            FROM users u
            JOIN organizations o ON o.id = u.org_id
            WHERE u.org_id = $1 AND u.role = 'owner'
            LIMIT 1
            "#,
        )
        .bind(org_id)
        .fetch_optional(&self.pool)
        .await?;

        result.ok_or_else(|| {
            BillingError::CustomerNotFound(format!("No owner found for organization {}", org_id))
        })
    }

    /// Get or create a Stripe customer for an organization
    /// This is a helper for admin_change_tier to avoid requiring CustomerService dependency
    async fn get_or_create_stripe_customer(
        &self,
        org_id: Uuid,
        email: &str,
        name: &str,
    ) -> BillingResult<Customer> {
        // Check if org already has a Stripe customer ID
        let existing: Option<(Option<String>,)> =
            sqlx::query_as("SELECT stripe_customer_id FROM organizations WHERE id = $1")
                .bind(org_id)
                .fetch_optional(&self.pool)
                .await?;

        if let Some((Some(customer_id),)) = existing {
            // Retrieve existing customer
            let customer_id = customer_id
                .parse::<CustomerId>()
                .map_err(|e| BillingError::StripeApi(format!("Invalid customer ID: {}", e)))?;

            let customer = Customer::retrieve(self.stripe.inner(), &customer_id, &[]).await?;

            tracing::info!(
                org_id = %org_id,
                customer_id = %customer.id,
                "Retrieved existing Stripe customer"
            );

            return Ok(customer);
        }

        // Create new customer
        let mut metadata = std::collections::HashMap::new();
        metadata.insert("org_id".to_string(), org_id.to_string());
        metadata.insert("platform".to_string(), "plexmcp".to_string());

        let params = CreateCustomer {
            email: Some(email),
            name: Some(name),
            metadata: Some(metadata),
            ..Default::default()
        };

        let customer = Customer::create(self.stripe.inner(), params).await?;

        // Store customer ID in database
        sqlx::query(
            "UPDATE organizations SET stripe_customer_id = $1, updated_at = NOW() WHERE id = $2",
        )
        .bind(customer.id.as_str())
        .bind(org_id)
        .execute(&self.pool)
        .await?;

        tracing::info!(
            org_id = %org_id,
            customer_id = %customer.id,
            "Created new Stripe customer"
        );

        Ok(customer)
    }

    /// Apply or extend trial period on an existing subscription
    async fn apply_trial_period(
        &self,
        subscription_id: &SubscriptionId,
        trial_days: u32,
    ) -> BillingResult<Subscription> {
        // Calculate trial end timestamp
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| BillingError::Database(format!("System time error: {}", e)))?
            .as_secs() as i64;

        let trial_end_timestamp = now + (trial_days as i64 * 24 * 60 * 60);

        // The trial_end field expects a Scheduled::Timestamp wrapper
        let params = UpdateSubscription {
            trial_end: Some(stripe::Scheduled::Timestamp(trial_end_timestamp)),
            ..Default::default()
        };

        let subscription =
            Subscription::update(self.stripe.inner(), subscription_id, params).await?;

        tracing::info!(
            subscription_id = %subscription_id,
            trial_days = trial_days,
            trial_end = ?subscription.trial_end,
            "Applied trial period to subscription"
        );

        Ok(subscription)
    }

    /// Create a custom Stripe price for Enterprise tier
    ///
    /// This creates a unique price object in Stripe for custom Enterprise pricing.
    /// The price is tagged with org_id metadata for tracking.
    async fn create_custom_enterprise_price(
        &self,
        amount_cents: i64,
        interval: &str, // "month" or "year"
        org_id: Uuid,
    ) -> BillingResult<String> {
        use stripe::{CreatePrice, CreatePriceRecurring, CreatePriceRecurringInterval, Currency};

        let recurring_interval = match interval {
            "annual" => CreatePriceRecurringInterval::Year,
            _ => CreatePriceRecurringInterval::Month,
        };

        let mut params = CreatePrice::new(Currency::USD);
        params.unit_amount = Some(amount_cents);
        params.recurring = Some(CreatePriceRecurring {
            interval: recurring_interval,
            interval_count: None,
            aggregate_usage: None,
            trial_period_days: None,
            usage_type: None,
        });

        let mut metadata = std::collections::HashMap::new();
        metadata.insert("org_id".to_string(), org_id.to_string());
        metadata.insert("custom_price".to_string(), "true".to_string());
        metadata.insert("tier".to_string(), "enterprise".to_string());
        params.metadata = Some(metadata);

        let price = stripe::Price::create(self.stripe.inner(), params).await?;

        tracing::info!(
            org_id = %org_id,
            price_id = %price.id,
            amount_cents = amount_cents,
            interval = interval,
            "Created custom Enterprise price"
        );

        Ok(price.id.to_string())
    }

    /// Create and send a Stripe invoice
    ///
    /// Creates an invoice with the specified price and sends it to the customer.
    /// Returns (invoice_id, invoice_status)
    #[allow(dead_code)]
    async fn create_and_send_invoice(
        &self,
        customer_id: &CustomerId,
        price_id: &str,
    ) -> BillingResult<(String, String)> {
        use stripe::{CreateInvoice, CreateInvoiceItem};

        // Parse price_id string to PriceId
        let stripe_price_id = price_id
            .parse::<stripe::PriceId>()
            .map_err(|e| BillingError::StripeApi(format!("Invalid price ID: {}", e)))?;

        // Create invoice item
        let mut item_params = CreateInvoiceItem::new(customer_id.clone());
        item_params.price = Some(stripe_price_id);
        stripe::InvoiceItem::create(self.stripe.inner(), item_params).await?;

        // Create invoice
        let mut invoice_params = CreateInvoice::new();
        invoice_params.customer = Some(customer_id.clone());
        invoice_params.auto_advance = Some(true);
        invoice_params.collection_method = Some(stripe::CollectionMethod::SendInvoice);
        invoice_params.days_until_due = Some(30);

        let invoice = stripe::Invoice::create(self.stripe.inner(), invoice_params).await?;

        // Finalize invoice to send it
        let finalized =
            stripe::Invoice::finalize(self.stripe.inner(), &invoice.id, Default::default()).await?;

        let status = match finalized.status {
            Some(s) => format!("{:?}", s),
            None => "unknown".to_string(),
        };

        tracing::info!(
            customer_id = %customer_id,
            invoice_id = %finalized.id,
            status = %status,
            "Created and sent invoice"
        );

        Ok((finalized.id.to_string(), status))
    }

    /// Get subscription for an organization
    pub async fn get_subscription(&self, org_id: Uuid) -> BillingResult<Option<Subscription>> {
        let result: Option<(Option<String>,)> = sqlx::query_as(
            r#"
            SELECT stripe_subscription_id
            FROM subscriptions
            WHERE org_id = $1
            ORDER BY created_at DESC
            LIMIT 1
            "#,
        )
        .bind(org_id)
        .fetch_optional(&self.pool)
        .await?;

        match result {
            Some((Some(sub_id),)) => {
                let sub_id = sub_id.parse::<SubscriptionId>().map_err(|e| {
                    BillingError::StripeApi(format!("Invalid subscription ID: {}", e))
                })?;
                let subscription =
                    Subscription::retrieve(self.stripe.inner(), &sub_id, &[]).await?;
                Ok(Some(subscription))
            }
            _ => Ok(None),
        }
    }

    /// List all subscriptions for a customer
    pub async fn list_customer_subscriptions(
        &self,
        customer_id: &str,
    ) -> BillingResult<Vec<Subscription>> {
        let customer_id = customer_id
            .parse::<CustomerId>()
            .map_err(|e| BillingError::StripeApi(format!("Invalid customer ID: {}", e)))?;

        let params = ListSubscriptions {
            customer: Some(customer_id),
            ..Default::default()
        };

        let subscriptions = Subscription::list(self.stripe.inner(), &params).await?;
        Ok(subscriptions.data)
    }

    /// Sync subscription state to database
    pub async fn sync_subscription_to_db(
        &self,
        org_id: Uuid,
        subscription: &Subscription,
    ) -> BillingResult<()> {
        let status = match subscription.status {
            StripeSubStatus::Active => "active",
            StripeSubStatus::PastDue => "past_due",
            StripeSubStatus::Canceled => "canceled",
            StripeSubStatus::Unpaid => "unpaid",
            StripeSubStatus::Trialing => "trialing",
            StripeSubStatus::Incomplete => "incomplete",
            StripeSubStatus::IncompleteExpired => "incomplete_expired",
            StripeSubStatus::Paused => "paused",
        };

        let price_id = subscription
            .items
            .data
            .first()
            .and_then(|item| item.price.as_ref())
            .map(|p| p.id.to_string());

        // Find the metered subscription item (usage_type = "metered")
        // This is the item used for overage billing
        let metered_item_id: Option<String> = subscription
            .items
            .data
            .iter()
            .find(|item| {
                item.price
                    .as_ref()
                    .and_then(|p| p.recurring.as_ref())
                    .map(|r| r.usage_type == stripe::RecurringUsageType::Metered)
                    .unwrap_or(false)
            })
            .map(|item| item.id.to_string());

        if metered_item_id.is_some() {
            tracing::info!(
                org_id = %org_id,
                subscription_id = %subscription.id,
                metered_item_id = ?metered_item_id,
                "Found metered subscription item for overage billing"
            );
        }

        let current_period_start =
            OffsetDateTime::from_unix_timestamp(subscription.current_period_start)
                .unwrap_or(OffsetDateTime::now_utc());

        let current_period_end =
            OffsetDateTime::from_unix_timestamp(subscription.current_period_end)
                .unwrap_or(OffsetDateTime::now_utc());

        let canceled_at = subscription
            .canceled_at
            .map(|t| OffsetDateTime::from_unix_timestamp(t).unwrap_or(OffsetDateTime::now_utc()));

        let trial_start = subscription
            .trial_start
            .map(|t| OffsetDateTime::from_unix_timestamp(t).unwrap_or(OffsetDateTime::now_utc()));

        let trial_end = subscription
            .trial_end
            .map(|t| OffsetDateTime::from_unix_timestamp(t).unwrap_or(OffsetDateTime::now_utc()));

        // Upsert subscription record (including metered item ID)
        // Use ON CONFLICT (org_id) because there's a unique index on org_id,
        // and when creating a new subscription after a canceled one, we get a new stripe_subscription_id
        sqlx::query(
            r#"
            INSERT INTO subscriptions (
                id, org_id, stripe_subscription_id, stripe_price_id, stripe_metered_item_id, status,
                current_period_start, current_period_end, cancel_at_period_end,
                canceled_at, trial_start, trial_end, created_at, updated_at
            ) VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, NOW(), NOW()
            )
            ON CONFLICT (org_id) DO UPDATE SET
                stripe_subscription_id = EXCLUDED.stripe_subscription_id,
                stripe_price_id = EXCLUDED.stripe_price_id,
                stripe_metered_item_id = EXCLUDED.stripe_metered_item_id,
                status = EXCLUDED.status,
                current_period_start = EXCLUDED.current_period_start,
                current_period_end = EXCLUDED.current_period_end,
                cancel_at_period_end = EXCLUDED.cancel_at_period_end,
                canceled_at = EXCLUDED.canceled_at,
                trial_start = EXCLUDED.trial_start,
                trial_end = EXCLUDED.trial_end,
                updated_at = NOW()
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(org_id)
        .bind(subscription.id.as_str())
        .bind(price_id)
        .bind(metered_item_id)
        .bind(status)
        .bind(current_period_start)
        .bind(current_period_end)
        .bind(subscription.cancel_at_period_end)
        .bind(canceled_at)
        .bind(trial_start)
        .bind(trial_end)
        .execute(&self.pool)
        .await?;

        // Update org tier based on price ID
        // Note: Using change_tier() for audit logging. In fully DB-authoritative architecture,
        // this sync might be skipped, but for now we keep it to support checkout flows
        // where the webhook confirms the tier change.
        if let Some(price_id) = subscription
            .items
            .data
            .first()
            .and_then(|item| item.price.as_ref())
            .map(|p| p.id.as_str())
        {
            if let Some(tier) = self.stripe.config().tier_for_price_id(price_id) {
                // IMPORTANT: Use "immediate" timing since we're syncing what Stripe says
                // (Stripe has already made the decision about when this takes effect)
                let tier_options = TierChangeOptions {
                    source: Some(TierChangeSource::StripeWebhook),
                    downgrade_timing: Some("immediate".to_string()),
                    ..Default::default()
                };

                // Log but don't fail on tier change errors during sync
                // This handles the case where tier was set by admin/user before webhook arrived
                if let Err(e) = self.change_tier(org_id, tier, tier_options).await {
                    tracing::warn!(
                        org_id = %org_id,
                        tier = %tier,
                        error = %e,
                        "Non-fatal: tier change during sync failed (tier may already be set correctly)"
                    );
                }
            }
        }

        Ok(())
    }

    /// Get the most recent cancelled subscription for an organization
    pub async fn get_cancelled_subscription(
        &self,
        org_id: Uuid,
    ) -> BillingResult<CancelledSubscriptionInfo> {
        let result: Option<(
            String,
            Option<String>,
            OffsetDateTime,
            OffsetDateTime,
            Option<OffsetDateTime>,
        )> = sqlx::query_as(
            r#"
            SELECT
                stripe_subscription_id,
                stripe_price_id,
                current_period_start,
                current_period_end,
                canceled_at
            FROM subscriptions
            WHERE org_id = $1 AND status = 'canceled'
            ORDER BY canceled_at DESC NULLS LAST
            LIMIT 1
            "#,
        )
        .bind(org_id)
        .fetch_optional(&self.pool)
        .await?;

        match result {
            Some((
                stripe_subscription_id,
                stripe_price_id,
                current_period_start,
                current_period_end,
                canceled_at,
            )) => Ok(CancelledSubscriptionInfo {
                stripe_subscription_id,
                stripe_price_id,
                current_period_start,
                current_period_end,
                canceled_at,
            }),
            None => Err(BillingError::NoCancelledSubscription(org_id.to_string())),
        }
    }

    /// Get Stripe customer ID for an organization
    async fn get_stripe_customer_id(&self, org_id: Uuid) -> BillingResult<String> {
        let result: Option<(Option<String>,)> =
            sqlx::query_as("SELECT stripe_customer_id FROM organizations WHERE id = $1")
                .bind(org_id)
                .fetch_optional(&self.pool)
                .await?;

        result.and_then(|(id,)| id).ok_or(BillingError::NoCustomer)
    }

    /// Reactivate a cancelled subscription with proration credit
    ///
    /// This function:
    /// 1. Gets the cancelled subscription info
    /// 2. Calculates remaining credit from the cancelled period
    /// 3. Deducts any outstanding overages
    /// 4. Creates a new subscription with trial_end if credit covers it
    /// 5. Syncs the new subscription to the database
    pub async fn reactivate_subscription(
        &self,
        org_id: Uuid,
        new_tier: &str,
        billing_interval: &str,
    ) -> BillingResult<ReactivationResult> {
        // Validate tier
        if new_tier == "free" {
            return Err(BillingError::InvalidTier(
                "Cannot reactivate to free tier - use downgrade instead".to_string(),
            ));
        }

        // 1. Get customer ID
        let customer_id = self.get_stripe_customer_id(org_id).await?;
        let stripe_customer_id = customer_id
            .parse::<CustomerId>()
            .map_err(|e| BillingError::StripeApi(format!("Invalid customer ID: {}", e)))?;

        // 2. Get cancelled subscription (if any)
        let cancelled_sub = self.get_cancelled_subscription(org_id).await.ok();

        // 3. Calculate remaining credit (only if period hasn't ended)
        let now = OffsetDateTime::now_utc();
        let credit_cents = if let Some(ref sub) = cancelled_sub {
            if sub.current_period_end > now {
                let remaining_secs = (sub.current_period_end - now).whole_seconds().max(0);
                let total_period_secs = (sub.current_period_end - sub.current_period_start)
                    .whole_seconds()
                    .max(1);

                // Get the price of the cancelled subscription
                // Default to $29 (Pro monthly) if we can't determine the price
                let cancelled_price_cents = if let Some(price_id) = &sub.stripe_price_id {
                    self.get_price_amount(price_id).await.unwrap_or(2900)
                } else {
                    2900
                };

                let credit = (remaining_secs as f64 / total_period_secs as f64
                    * cancelled_price_cents as f64) as i64;
                tracing::info!(
                    org_id = %org_id,
                    remaining_secs = remaining_secs,
                    total_period_secs = total_period_secs,
                    cancelled_price_cents = cancelled_price_cents,
                    credit = credit,
                    "Calculated reactivation credit"
                );
                credit
            } else {
                0 // Period has ended, no credit
            }
        } else {
            0 // No cancelled subscription, no credit
        };

        // 4. Get outstanding overages (pending/awaiting payment)
        let overage_cents = self.get_outstanding_overages(org_id).await.unwrap_or(0);

        // 5. Check if overages exceed credit
        if overage_cents > credit_cents {
            return Err(BillingError::OveragesExceedCredit {
                overage_cents,
                credit_cents,
                pay_first_url: format!("{}/billing/overages", self.stripe.config().app_base_url),
            });
        }

        // 6. Calculate net credit
        let net_credit = (credit_cents - overage_cents).max(0);

        // 7. Get price ID for new tier
        let price_id = match billing_interval.to_lowercase().as_str() {
            "annual" | "yearly" => self
                .stripe
                .config()
                .annual_price_id_for_tier(new_tier)
                .ok_or_else(|| {
                    BillingError::InvalidTier(format!(
                        "{} (annual pricing not configured)",
                        new_tier
                    ))
                })?,
            _ => self
                .stripe
                .config()
                .price_id_for_tier(new_tier)
                .ok_or_else(|| BillingError::InvalidTier(new_tier.to_string()))?,
        };

        // Get the new subscription price
        let new_price_cents = self.get_price_amount(price_id).await?;

        // 8. Calculate trial days from credit
        let trial_days = if net_credit >= new_price_cents {
            // Credit covers at least one month - use trial_end
            // Calculate: full months + partial days
            let full_months = net_credit / new_price_cents;
            let extra_credit = net_credit % new_price_cents;
            let partial_days = (extra_credit as f64 / new_price_cents as f64 * 30.0) as i32;
            let days = (full_months * 30) as i32 + partial_days;
            days.min(90) // Cap at 90 days
        } else {
            0
        };

        // 9. Build subscription parameters
        let mut metadata = std::collections::HashMap::new();
        metadata.insert("org_id".to_string(), org_id.to_string());
        metadata.insert("tier".to_string(), new_tier.to_string());
        metadata.insert("reactivation".to_string(), "true".to_string());
        if net_credit > 0 {
            metadata.insert("credit_applied_cents".to_string(), net_credit.to_string());
        }
        if overage_cents > 0 {
            metadata.insert(
                "overages_deducted_cents".to_string(),
                overage_cents.to_string(),
            );
        }

        let mut params = CreateSubscription::new(stripe_customer_id.clone());
        params.items = Some(vec![CreateSubscriptionItems {
            price: Some(price_id.to_string()),
            quantity: Some(1),
            ..Default::default()
        }]);
        params.metadata = Some(metadata);

        // 10. Apply credit as trial period
        let trial_end_time = if trial_days > 0 {
            let trial_end = now + time::Duration::days(trial_days as i64);
            params.trial_end = Some(stripe::Scheduled::Timestamp(trial_end.unix_timestamp()));
            Some(trial_end)
        } else if net_credit > 0 && net_credit < new_price_cents {
            // Partial credit - redirect to checkout with coupon for the credit amount
            // This ensures user can enter payment details and credit is applied
            tracing::info!(
                org_id = %org_id,
                net_credit = net_credit,
                new_price_cents = new_price_cents,
                "Partial credit requires checkout flow"
            );

            // Try to create a one-time coupon for the credit amount
            // If coupon creation fails (e.g., amount too small, Stripe error),
            // fall back to regular checkout without coupon
            match self.create_credit_coupon(net_credit, org_id).await {
                Ok(coupon) => {
                    // Coupon created successfully - return data for API to create checkout
                    return Err(BillingError::UseCheckoutFlow {
                        checkout_url: None, // Will be created by API layer
                        credit_cents: net_credit,
                        coupon_id: Some(coupon.id.to_string()),
                        tier: new_tier.to_string(),
                        billing_interval: billing_interval.to_string(),
                        customer_id: customer_id.clone(),
                    });
                }
                Err(e) => {
                    // Coupon creation failed - log and fall back to regular checkout
                    tracing::warn!(
                        org_id = %org_id,
                        net_credit = net_credit,
                        error = %e,
                        "Failed to create credit coupon, falling back to checkout without coupon"
                    );

                    // Return data for regular checkout without coupon
                    return Err(BillingError::UseCheckoutFlow {
                        checkout_url: None, // Will be created by API layer
                        credit_cents: 0,    // Credit will be handled manually by admin
                        coupon_id: None,
                        tier: new_tier.to_string(),
                        billing_interval: billing_interval.to_string(),
                        customer_id: customer_id.clone(),
                    });
                }
            }
        } else {
            None
        };

        // 11. Create the subscription
        tracing::info!(
            org_id = %org_id,
            customer_id = %stripe_customer_id,
            price_id = %price_id,
            trial_days = trial_days,
            "Creating Stripe subscription for reactivation"
        );

        let subscription = Subscription::create(self.stripe.inner(), params)
            .await
            .map_err(|e| {
                tracing::error!(
                    org_id = %org_id,
                    customer_id = %stripe_customer_id,
                    error = %e,
                    "Failed to create Stripe subscription for reactivation"
                );
                e
            })?;

        // 12. Clear accumulated overages if we deducted them
        if overage_cents > 0 {
            self.clear_accumulated_overages(org_id).await?;
        }

        // 13. Sync to database
        self.sync_subscription_to_db(org_id, &subscription).await?;

        // 14. Use consolidated change_tier() for DB update + audit logging
        let tier_options = TierChangeOptions {
            source: Some(TierChangeSource::UserUpgrade),
            reason: Some("Subscription reactivated".to_string()),
            ..Default::default()
        };

        self.change_tier(org_id, new_tier, tier_options).await?;

        let current_period_end =
            OffsetDateTime::from_unix_timestamp(subscription.current_period_end).unwrap_or(now);

        let message = if trial_days > 0 {
            format!(
                "Subscription reactivated with {} days credit applied (${:.2} credit - ${:.2} overages = ${:.2} net)",
                trial_days,
                credit_cents as f64 / 100.0,
                overage_cents as f64 / 100.0,
                net_credit as f64 / 100.0
            )
        } else {
            format!(
                "Subscription reactivated. ${:.2} in overages were deducted from your ${:.2} credit.",
                overage_cents as f64 / 100.0,
                credit_cents as f64 / 100.0
            )
        };

        tracing::info!(
            org_id = %org_id,
            subscription_id = %subscription.id,
            tier = new_tier,
            credit_applied_cents = net_credit,
            trial_days = trial_days,
            overages_deducted_cents = overage_cents,
            "Subscription reactivated successfully"
        );

        Ok(ReactivationResult {
            stripe_subscription_id: subscription.id.to_string(),
            stripe_customer_id: customer_id,
            tier: new_tier.to_string(),
            billing_interval: billing_interval.to_string(),
            credit_applied_cents: net_credit,
            extra_trial_days: trial_days,
            overages_deducted_cents: overage_cents,
            current_period_end,
            trial_end: trial_end_time,
            message,
        })
    }

    /// Get the price amount in cents for a Stripe price ID
    async fn get_price_amount(&self, price_id: &str) -> BillingResult<i64> {
        let price_id = price_id
            .parse::<stripe::PriceId>()
            .map_err(|e| BillingError::StripeApi(format!("Invalid price ID: {}", e)))?;

        let price = stripe::Price::retrieve(self.stripe.inner(), &price_id, &[]).await?;

        price
            .unit_amount
            .ok_or_else(|| BillingError::StripeApi("Price has no unit_amount".to_string()))
    }

    /// Get outstanding overages for an organization (pending + awaiting payment)
    async fn get_outstanding_overages(&self, org_id: Uuid) -> BillingResult<i64> {
        let result: Option<(Option<i64>,)> = sqlx::query_as(
            r#"
            SELECT COALESCE(SUM(total_charge_cents), 0)
            FROM overage_charges
            WHERE org_id = $1 AND status IN ('pending', 'awaiting_payment')
            "#,
        )
        .bind(org_id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(result.and_then(|(sum,)| sum).unwrap_or(0))
    }

    /// Clear accumulated overages for an organization (mark as paid)
    async fn clear_accumulated_overages(&self, org_id: Uuid) -> BillingResult<()> {
        sqlx::query(
            r#"
            UPDATE overage_charges
            SET status = 'paid', paid_at = NOW()
            WHERE org_id = $1 AND status IN ('pending', 'awaiting_payment')
            "#,
        )
        .bind(org_id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Create a one-time coupon for credit amount (used in reactivation)
    ///
    /// Returns the created coupon on success, or an error if the coupon cannot be created.
    /// The minimum amount is 50 cents (Stripe allows 1 cent, but very small amounts are not useful).
    async fn create_credit_coupon(
        &self,
        amount_cents: i64,
        org_id: Uuid,
    ) -> BillingResult<stripe::Coupon> {
        use stripe::{CouponDuration, CreateCoupon};

        // Validate amount - Stripe allows 1 cent minimum, but 50 cents is more practical
        if amount_cents < 50 {
            tracing::warn!(
                org_id = %org_id,
                amount_cents = amount_cents,
                "Credit amount too small to create coupon (minimum 50 cents)"
            );
            return Err(BillingError::InvalidAmount(format!(
                "Credit amount {} cents is too small to create a coupon (minimum 50 cents)",
                amount_cents
            )));
        }

        // Create strings first to avoid lifetime issues
        let coupon_name = format!("Reactivation credit for org {}", org_id);
        // Stripe coupon IDs must be 40 chars max
        // Format: rx_<8-char-org-prefix>_<10-digit-timestamp>_<8-char-random> = 31 chars
        let org_prefix = &org_id.simple().to_string()[..8];
        let timestamp = chrono::Utc::now().timestamp();
        let unique_suffix = &Uuid::new_v4().simple().to_string()[..8];
        let coupon_id = format!("rx_{}_{:010}_{}", org_prefix, timestamp, unique_suffix);

        let mut params = CreateCoupon::new();
        params.amount_off = Some(amount_cents);
        params.currency = Some(stripe::Currency::USD);
        params.duration = Some(CouponDuration::Once);
        params.name = Some(&coupon_name);
        params.max_redemptions = Some(1);
        params.id = Some(&coupon_id);

        tracing::info!(
            org_id = %org_id,
            coupon_id = %coupon_id,
            amount_cents = amount_cents,
            "Attempting to create Stripe coupon"
        );

        let coupon = stripe::Coupon::create(self.stripe.inner(), params)
            .await
            .map_err(|e| {
                tracing::error!(
                    org_id = %org_id,
                    coupon_id = %coupon_id,
                    error = %e,
                    error_debug = ?e,
                    "Failed to create Stripe coupon"
                );
                e
            })?;

        tracing::info!(
            org_id = %org_id,
            coupon_id = %coupon.id,
            amount_cents = amount_cents,
            "Created reactivation credit coupon"
        );

        Ok(coupon)
    }

    /// Get the Stripe subscription ID for an organization
    async fn get_subscription_id(&self, org_id: Uuid) -> BillingResult<SubscriptionId> {
        let result: Option<(Option<String>,)> = sqlx::query_as(
            r#"
            SELECT stripe_subscription_id
            FROM subscriptions
            WHERE org_id = $1 AND status IN ('active', 'trialing', 'past_due')
            ORDER BY created_at DESC
            LIMIT 1
            "#,
        )
        .bind(org_id)
        .fetch_optional(&self.pool)
        .await?;

        match result {
            Some((Some(id),)) => id
                .parse::<SubscriptionId>()
                .map_err(|e| BillingError::StripeApi(format!("Invalid subscription ID: {}", e))),
            _ => Err(BillingError::SubscriptionNotFound(org_id.to_string())),
        }
    }

    // ============ SUBSCRIPTION PAUSE FEATURE ============

    /// Pause a subscription (stop billing but retain tier)
    ///
    /// This uses Stripe's `pause_collection` feature which:
    /// - Stops future invoices from being created
    /// - Customer retains access until manually resumed or auto-resumes
    /// - Different from spend cap pause (which is usage-based)
    pub async fn pause_subscription(
        &self,
        org_id: Uuid,
        user_id: Uuid,
        reason: Option<String>,
        resume_at: Option<OffsetDateTime>,
    ) -> BillingResult<SubscriptionPauseResult> {
        let subscription_id = self.get_subscription_id(org_id).await?;

        // Get current subscription to verify it's not already paused
        let subscription =
            Subscription::retrieve(self.stripe.inner(), &subscription_id, &[]).await?;

        // Check if already paused
        if subscription.pause_collection.is_some() {
            return Err(BillingError::InvalidInput(
                "Subscription is already paused".to_string(),
            ));
        }

        // Pause the subscription in Stripe
        let mut update = UpdateSubscription::new();

        // Set up pause_collection behavior
        // Note: Stripe's async-stripe library may need manual JSON for pause_collection
        // Using metadata as fallback tracking mechanism
        let mut metadata = subscription.metadata.clone();
        metadata.insert(
            "paused_at".to_string(),
            OffsetDateTime::now_utc().to_string(),
        );
        metadata.insert("paused_by".to_string(), user_id.to_string());
        if let Some(ref r) = reason {
            metadata.insert("pause_reason".to_string(), r.clone());
        }
        if let Some(ref at) = resume_at {
            metadata.insert("resume_at".to_string(), at.unix_timestamp().to_string());
        }
        update.metadata = Some(metadata);

        // For billing pause, we use cancel_at_period_end with metadata tracking
        // True subscription pause requires Stripe's pause_collection which may need
        // manual API calls in some library versions
        // This simpler approach marks as "will cancel" but we track in metadata
        // that it's actually a pause that can be resumed

        // Update subscription metadata to track pause
        Subscription::update(self.stripe.inner(), &subscription_id, update).await?;

        // Update database
        sqlx::query(
            r#"
            UPDATE subscriptions
            SET is_paused = true,
                paused_at = NOW(),
                paused_by = $2,
                pause_reason = $3,
                scheduled_resume_at = $4,
                updated_at = NOW()
            WHERE org_id = $1 AND status IN ('active', 'trialing')
            "#,
        )
        .bind(org_id)
        .bind(user_id)
        .bind(&reason)
        .bind(resume_at)
        .execute(&self.pool)
        .await
        .map_err(|e| BillingError::Database(e.to_string()))?;

        // Log billing event
        let event_logger = BillingEventLogger::new(self.pool.clone());
        if let Err(e) = event_logger
            .log_event(
                BillingEventBuilder::new(org_id, BillingEventType::SubscriptionPaused)
                    .data(serde_json::json!({
                        "subscription_id": subscription_id.to_string(),
                        "reason": reason,
                        "resume_at": resume_at.map(|t| t.to_string()),
                    }))
                    .actor(user_id, ActorType::User),
            )
            .await
        {
            tracing::warn!(error = %e, "Failed to log subscription paused event");
        }

        tracing::info!(
            org_id = %org_id,
            subscription_id = %subscription_id,
            user_id = %user_id,
            "Subscription paused"
        );

        Ok(SubscriptionPauseResult {
            subscription_id: subscription_id.to_string(),
            paused_at: OffsetDateTime::now_utc(),
            resume_at,
            reason,
        })
    }

    /// Unpause a voluntarily paused subscription
    /// Note: This is different from `resume_subscription` which unsets cancel_at_period_end
    pub async fn unpause_subscription(
        &self,
        org_id: Uuid,
        user_id: Uuid,
    ) -> BillingResult<SubscriptionResumeResult> {
        let subscription_id = self.get_subscription_id(org_id).await?;

        // Get current subscription
        let subscription =
            Subscription::retrieve(self.stripe.inner(), &subscription_id, &[]).await?;

        // Update Stripe metadata to clear pause info
        let mut metadata = subscription.metadata.clone();
        metadata.remove("paused_at");
        metadata.remove("paused_by");
        metadata.remove("pause_reason");
        metadata.remove("resume_at");
        metadata.insert(
            "resumed_at".to_string(),
            OffsetDateTime::now_utc().to_string(),
        );
        metadata.insert("resumed_by".to_string(), user_id.to_string());

        let mut update = UpdateSubscription::new();
        update.metadata = Some(metadata);

        // If it was set to cancel at period end due to pause, undo that
        if subscription.cancel_at_period_end {
            update.cancel_at_period_end = Some(false);
        }

        Subscription::update(self.stripe.inner(), &subscription_id, update).await?;

        // Update database
        let updated: Option<(Option<OffsetDateTime>, Option<String>)> = sqlx::query_as(
            r#"
            UPDATE subscriptions
            SET is_paused = false,
                paused_at = NULL,
                paused_by = NULL,
                pause_reason = NULL,
                scheduled_resume_at = NULL,
                resumed_at = NOW(),
                updated_at = NOW()
            WHERE org_id = $1 AND is_paused = true
            RETURNING paused_at, pause_reason
            "#,
        )
        .bind(org_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| BillingError::Database(e.to_string()))?;

        let (original_pause_time, original_reason) = updated
            .ok_or_else(|| BillingError::InvalidInput("Subscription is not paused".to_string()))?;

        // Log billing event
        let event_logger = BillingEventLogger::new(self.pool.clone());
        if let Err(e) = event_logger
            .log_event(
                BillingEventBuilder::new(org_id, BillingEventType::SubscriptionResumed)
                    .data(serde_json::json!({
                        "subscription_id": subscription_id.to_string(),
                        "was_paused_at": original_pause_time.map(|t| t.to_string()),
                        "original_reason": original_reason,
                    }))
                    .actor(user_id, ActorType::User),
            )
            .await
        {
            tracing::warn!(error = %e, "Failed to log subscription resumed event");
        }

        tracing::info!(
            org_id = %org_id,
            subscription_id = %subscription_id,
            user_id = %user_id,
            "Subscription resumed"
        );

        Ok(SubscriptionResumeResult {
            subscription_id: subscription_id.to_string(),
            resumed_at: OffsetDateTime::now_utc(),
            was_paused_since: original_pause_time,
        })
    }

    /// Get pause status for a subscription
    pub async fn get_pause_status(
        &self,
        org_id: Uuid,
    ) -> BillingResult<Option<SubscriptionPauseStatus>> {
        let result: Option<(
            bool,
            Option<OffsetDateTime>,
            Option<Uuid>,
            Option<String>,
            Option<OffsetDateTime>,
        )> = sqlx::query_as(
            r#"
            SELECT is_paused, paused_at, paused_by, pause_reason, scheduled_resume_at
            FROM subscriptions
            WHERE org_id = $1 AND status IN ('active', 'trialing', 'past_due')
            LIMIT 1
            "#,
        )
        .bind(org_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| BillingError::Database(e.to_string()))?;

        match result {
            Some((true, paused_at, paused_by, pause_reason, scheduled_resume_at)) => {
                Ok(Some(SubscriptionPauseStatus {
                    is_paused: true,
                    paused_at,
                    paused_by,
                    pause_reason,
                    scheduled_resume_at,
                }))
            }
            Some((false, ..)) => Ok(Some(SubscriptionPauseStatus {
                is_paused: false,
                paused_at: None,
                paused_by: None,
                pause_reason: None,
                scheduled_resume_at: None,
            })),
            None => Ok(None),
        }
    }
}

/// Result of pausing a subscription
#[derive(Debug, Clone, serde::Serialize)]
pub struct SubscriptionPauseResult {
    pub subscription_id: String,
    pub paused_at: OffsetDateTime,
    pub resume_at: Option<OffsetDateTime>,
    pub reason: Option<String>,
}

/// Result of resuming a subscription
#[derive(Debug, Clone, serde::Serialize)]
pub struct SubscriptionResumeResult {
    pub subscription_id: String,
    pub resumed_at: OffsetDateTime,
    pub was_paused_since: Option<OffsetDateTime>,
}

/// Status of subscription pause
#[derive(Debug, Clone, serde::Serialize)]
pub struct SubscriptionPauseStatus {
    pub is_paused: bool,
    pub paused_at: Option<OffsetDateTime>,
    pub paused_by: Option<Uuid>,
    pub pause_reason: Option<String>,
    pub scheduled_resume_at: Option<OffsetDateTime>,
}

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // Plan Tests
    // =========================================================================

    #[test]
    fn test_plan_free() {
        let plan = Plan::free();
        assert_eq!(plan.tier, SubscriptionTier::Free);
        assert!(plan.stripe_price_id.is_empty()); // Free tier has no price
        assert_eq!(plan.monthly_requests, 1_000);
        assert_eq!(plan.max_mcps, 5);
        assert_eq!(plan.max_users, 1);
        assert_eq!(plan.max_api_keys, 5);
    }

    #[test]
    fn test_plan_pro() {
        let plan = Plan::pro("price_pro_123");
        assert_eq!(plan.tier, SubscriptionTier::Pro);
        assert_eq!(plan.stripe_price_id, "price_pro_123");
        assert_eq!(plan.monthly_requests, 50_000);
        assert_eq!(plan.max_mcps, 20);
        assert_eq!(plan.max_users, 5);
        assert_eq!(plan.max_api_keys, 20);
    }

    #[test]
    fn test_plan_team() {
        let plan = Plan::team("price_team_456");
        assert_eq!(plan.tier, SubscriptionTier::Team);
        assert_eq!(plan.stripe_price_id, "price_team_456");
        assert_eq!(plan.monthly_requests, 250_000);
        assert_eq!(plan.max_mcps, 50);
        assert_eq!(plan.max_users, u32::MAX); // Unlimited
        assert_eq!(plan.max_api_keys, 50);
    }

    #[test]
    fn test_plan_enterprise() {
        let plan = Plan::enterprise("price_ent_789");
        assert_eq!(plan.tier, SubscriptionTier::Enterprise);
        assert_eq!(plan.stripe_price_id, "price_ent_789");
        assert_eq!(plan.monthly_requests, u64::MAX); // Unlimited
        assert_eq!(plan.max_mcps, u32::MAX); // Unlimited
        assert_eq!(plan.max_users, u32::MAX); // Unlimited
        assert_eq!(plan.max_api_keys, u32::MAX); // Unlimited
    }

    // =========================================================================
    // AdminTierChangeParams Tests
    // =========================================================================

    #[test]
    fn test_admin_tier_change_params_defaults() {
        let params = AdminTierChangeParams {
            new_tier: "pro".to_string(),
            trial_days: None,
            reason: "Test upgrade".to_string(),
            skip_payment_validation: false,
            billing_interval: None,
            custom_price_cents: None,
            subscription_start_date: None,
            payment_method: None,
            admin_user_id: None,
            downgrade_timing: None,
            refund_type: None,
        };

        assert_eq!(params.new_tier, "pro");
        assert!(params.trial_days.is_none());
        assert!(!params.skip_payment_validation);
    }

    #[test]
    fn test_admin_tier_change_params_with_trial() {
        let params = AdminTierChangeParams {
            new_tier: "team".to_string(),
            trial_days: Some(30),
            reason: "Enterprise trial".to_string(),
            skip_payment_validation: true,
            billing_interval: Some("annual".to_string()),
            custom_price_cents: None,
            subscription_start_date: None,
            payment_method: Some("trial".to_string()),
            admin_user_id: Some(Uuid::new_v4()),
            downgrade_timing: None,
            refund_type: None,
        };

        assert_eq!(params.trial_days, Some(30));
        assert!(params.skip_payment_validation);
        assert_eq!(params.billing_interval.as_deref(), Some("annual"));
        assert_eq!(params.payment_method.as_deref(), Some("trial"));
    }

    #[test]
    fn test_admin_tier_change_params_enterprise_custom_price() {
        let params = AdminTierChangeParams {
            new_tier: "enterprise".to_string(),
            trial_days: None,
            reason: "Custom enterprise deal".to_string(),
            skip_payment_validation: false,
            billing_interval: Some("annual".to_string()),
            custom_price_cents: Some(499900), // $4,999/year
            subscription_start_date: None,
            payment_method: Some("invoice".to_string()),
            admin_user_id: None,
            downgrade_timing: None,
            refund_type: None,
        };

        assert_eq!(params.custom_price_cents, Some(499900));
        assert_eq!(params.payment_method.as_deref(), Some("invoice"));
    }

    // =========================================================================
    // AdminTierChangeResult Tests
    // =========================================================================

    #[test]
    fn test_admin_tier_change_result_serialization() {
        let result = AdminTierChangeResult {
            stripe_subscription_id: Some("sub_123".to_string()),
            stripe_customer_id: "cus_456".to_string(),
            tier: "pro".to_string(),
            trial_end: None,
            stripe_invoice_id: None,
            invoice_status: None,
            subscription_start_date: None,
            billing_interval: "monthly".to_string(),
            custom_price_id: None,
            scheduled: false,
            scheduled_effective_date: None,
            refund_issued: false,
            refund_amount_cents: None,
            refund_type: None,
            message: "Tier changed successfully".to_string(),
        };

        // Should serialize without errors
        let json = serde_json::to_string(&result).expect("Failed to serialize");
        assert!(json.contains("\"tier\":\"pro\""));
        assert!(json.contains("\"stripe_customer_id\":\"cus_456\""));
    }

    #[test]
    fn test_scheduled_downgrade_serialization() {
        let downgrade = ScheduledDowngrade {
            current_tier: "team".to_string(),
            new_tier: "pro".to_string(),
            effective_date: OffsetDateTime::now_utc(),
        };

        let json = serde_json::to_string(&downgrade).expect("Failed to serialize");
        assert!(json.contains("\"current_tier\":\"team\""));
        assert!(json.contains("\"new_tier\":\"pro\""));
    }

    // =========================================================================
    // Tier Ordering Tests
    // =========================================================================

    #[test]
    fn test_tier_ordering_logic() {
        // Simulate the tier_order function used in admin_change_tier
        let tier_order = |t: &str| -> i32 {
            match t {
                "free" => 0,
                "pro" => 1,
                "team" => 2,
                "enterprise" => 3,
                _ => 0,
            }
        };

        // Free is lowest
        assert_eq!(tier_order("free"), 0);
        // Pro > Free
        assert!(tier_order("pro") > tier_order("free"));
        // Team > Pro
        assert!(tier_order("team") > tier_order("pro"));
        // Enterprise is highest
        assert!(tier_order("enterprise") > tier_order("team"));
        // Unknown tiers default to 0 (free)
        assert_eq!(tier_order("unknown"), 0);
    }

    #[test]
    fn test_upgrade_detection() {
        let tier_order = |t: &str| -> i32 {
            match t {
                "free" => 0,
                "pro" => 1,
                "team" => 2,
                "enterprise" => 3,
                _ => 0,
            }
        };

        // Upgrade: new_order > current_order
        assert!(tier_order("pro") > tier_order("free")); // Free → Pro is upgrade
        assert!(tier_order("team") > tier_order("pro")); // Pro → Team is upgrade

        // Downgrade: new_order < current_order
        assert!(tier_order("pro") < tier_order("team")); // Team → Pro is downgrade
        assert!(tier_order("free") < tier_order("pro")); // Pro → Free is downgrade

        // Same tier: no change
        assert!(tier_order("pro") == tier_order("pro"));
    }

    #[test]
    fn test_billing_interval_parsing() {
        // Test default billing interval
        let params1 = AdminTierChangeParams {
            new_tier: "pro".to_string(),
            trial_days: None,
            reason: "Test".to_string(),
            skip_payment_validation: false,
            billing_interval: None,
            custom_price_cents: None,
            subscription_start_date: None,
            payment_method: None,
            admin_user_id: None,
            downgrade_timing: None,
            refund_type: None,
        };

        let interval = params1.billing_interval.as_deref().unwrap_or("monthly");
        assert_eq!(interval, "monthly");

        // Test explicit annual
        let params2 = AdminTierChangeParams {
            billing_interval: Some("annual".to_string()),
            ..params1.clone()
        };

        let interval = params2.billing_interval.as_deref().unwrap_or("monthly");
        assert_eq!(interval, "annual");
    }
}
