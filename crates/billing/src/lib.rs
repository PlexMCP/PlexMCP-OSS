// Billing crate clippy configuration
// These are intentional patterns in this crate:
#![allow(clippy::result_large_err)] // BillingError::UseCheckoutFlow contains checkout session data
#![allow(clippy::too_many_arguments)] // Some Stripe operations require many parameters
#![allow(clippy::type_complexity)] // Complex return types for Stripe API wrappers
#![allow(clippy::field_reassign_with_default)] // Used for conditional struct field setting
// Test code patterns (expected in test files):
#![cfg_attr(test, allow(clippy::unwrap_used))]
#![cfg_attr(test, allow(clippy::expect_used))]
#![cfg_attr(test, allow(clippy::absurd_extreme_comparisons))]

//! PlexMCP Billing Module
//!
//! Handles Stripe integration for subscriptions, usage metering, and invoicing.
//!
//! ## Features
//!
//! - **Subscription Management**: Create, upgrade, downgrade, cancel subscriptions
//! - **Add-ons**: Purchase individual add-ons (resource packs, features, premium)
//! - **Usage Tracking**: Track API requests per billing period
//! - **Overage Billing**: Charge for usage beyond plan limits
//! - **Instant Charges**: Auto-charge when overage exceeds $50 threshold
//! - **Spend Caps**: User-configurable budget limits with optional hard pause
//! - **Email Notifications**: Payment failed, trial ending, spend cap alerts, etc.
//! - **Webhooks**: Handle Stripe events

pub mod addons;
pub mod checkout;
pub mod client;
pub mod customer;
pub mod email;
pub mod entitlement;
pub mod error;
pub mod events;
pub mod history;
pub mod instant_charge;
pub mod invariants;
pub mod member_suspension;
pub mod metered;
pub mod overage;
pub mod portal;
pub mod rate_limit;
pub mod refund;
pub mod spend_cap;
pub mod subscriptions;
pub mod tax;
pub mod usage;
pub mod webhooks;

#[cfg(test)]
mod edge_case_tests;

// Add-ons
pub use addons::{
    AddonCategory, AddonInfo, AddonQuantities, AddonService, AddonType, AddonsListResponse,
    EnableAddonRequest, SubscriptionAddon,
};

// Checkout
pub use checkout::{BillingInterval, CheckoutResponse, CheckoutService};

// Client
pub use client::{PriceIds, StripeClient, StripeConfig};

// Customer
pub use customer::CustomerService;

// Email
pub use email::{BillingEmailService, EmailConfig};

// Error
pub use error::{BillingError, BillingResult};

// Metered
pub use metered::{
    MeteredBillingService, MeteredSubscription, UsageReportResponse, UsageReportResult,
};

// Instant Charge
pub use instant_charge::{
    InstantCharge, InstantChargeResult, InstantChargeService, INSTANT_CHARGE_THRESHOLD_CENTS,
};

// Overage
pub use overage::{
    AccumulatedOverage, OverageCharge, OverageRates, OverageService, OverageSummary, PayNowResult,
};

// Spend Cap
pub use spend_cap::{
    SpendCap, SpendCapCheckResult, SpendCapRequest, SpendCapService, SpendCapStatus,
    NOTIFICATION_THRESHOLDS,
};

// Portal
pub use portal::{PortalResponse, PortalService};

// Rate Limit
pub use rate_limit::{RateLimitConfig, RateLimitResult, RateLimiter};

// Refund
pub use refund::{AdminRefund, RefundResult, RefundService, RefundableCharge};

// Subscriptions
pub use subscriptions::{
    AdminTierChangeParams, AdminTierChangeResult, CancelledSubscriptionInfo, Plan,
    ProrationPreview, ReactivationResult, ScheduledDowngrade, SubscriptionPauseResult,
    SubscriptionPauseStatus, SubscriptionResumeResult, SubscriptionService,
};

// Usage
pub use usage::{BillingPeriodUsage, UsageEvent, UsageMeter, UsageSummary};

// Webhooks
pub use webhooks::{WebhookEventRecord, WebhookHandler, WebhookReplayResult};

// History
pub use history::{BillingHistoryRecord, BillingHistoryService, BillingSummary};

// Tax
pub use tax::{TaxBreakdown, TaxConfig, TaxId, TaxIdType, TaxService, TaxSummary};

// Entitlement
pub use entitlement::{
    Entitlement, EntitlementFeatures, EntitlementService, EntitlementSource, EntitlementState,
    RawBillingData,
};

// Invariants
pub use invariants::{
    InvariantCheckSummary, InvariantChecker, InvariantViolation, ViolationSeverity,
};

// Events
pub use events::{
    ActorType, BillingEvent, BillingEventBuilder, BillingEventLogger, BillingEventType,
};

// Member Suspension
pub use member_suspension::{
    AffectedMembersInfo, MemberStatusSummary, MemberSuspensionService, MemberToSuspend,
    SuspensionResult,
};

use sqlx::PgPool;

/// Main billing service that combines all billing functionality
pub struct BillingService {
    pub addons: AddonService,
    pub checkout: CheckoutService,
    pub customer: CustomerService,
    pub email: BillingEmailService,
    pub instant_charge: InstantChargeService,
    pub member_suspension: MemberSuspensionService,
    pub metered: MeteredBillingService,
    pub overage: OverageService,
    pub portal: PortalService,
    pub rate_limiter: RateLimiter,
    pub refund: RefundService,
    pub spend_cap: SpendCapService,
    pub subscriptions: SubscriptionService,
    pub usage: UsageMeter,
    pub webhooks: WebhookHandler,
}

impl BillingService {
    /// Create a new billing service from environment variables
    pub fn from_env(pool: PgPool) -> BillingResult<Self> {
        let stripe = StripeClient::from_env()?;
        let email_service = BillingEmailService::from_env();

        Ok(Self {
            addons: AddonService::new(stripe.clone(), pool.clone()),
            checkout: CheckoutService::new(stripe.clone(), pool.clone()),
            customer: CustomerService::new(stripe.clone(), pool.clone()),
            email: email_service.clone(),
            instant_charge: InstantChargeService::new(
                stripe.clone(),
                pool.clone(),
                email_service.clone(),
            ),
            member_suspension: MemberSuspensionService::new(pool.clone()),
            metered: MeteredBillingService::new(stripe.clone(), pool.clone()),
            overage: OverageService::new(stripe.clone(), pool.clone()),
            portal: PortalService::new(stripe.clone()),
            rate_limiter: RateLimiter::new_in_memory(),
            refund: RefundService::new(stripe.clone(), pool.clone()),
            spend_cap: SpendCapService::new(pool.clone(), email_service.clone()),
            subscriptions: SubscriptionService::new(stripe.clone(), pool.clone()),
            usage: UsageMeter::new(pool.clone()),
            webhooks: WebhookHandler::new(stripe, pool, email_service),
        })
    }

    /// Create a new billing service with explicit config
    pub fn new(config: StripeConfig, pool: PgPool) -> Self {
        let stripe = StripeClient::new(config);
        let email_service = BillingEmailService::from_env();

        Self {
            addons: AddonService::new(stripe.clone(), pool.clone()),
            checkout: CheckoutService::new(stripe.clone(), pool.clone()),
            customer: CustomerService::new(stripe.clone(), pool.clone()),
            email: email_service.clone(),
            instant_charge: InstantChargeService::new(
                stripe.clone(),
                pool.clone(),
                email_service.clone(),
            ),
            member_suspension: MemberSuspensionService::new(pool.clone()),
            metered: MeteredBillingService::new(stripe.clone(), pool.clone()),
            overage: OverageService::new(stripe.clone(), pool.clone()),
            portal: PortalService::new(stripe.clone()),
            rate_limiter: RateLimiter::new_in_memory(),
            refund: RefundService::new(stripe.clone(), pool.clone()),
            spend_cap: SpendCapService::new(pool.clone(), email_service.clone()),
            subscriptions: SubscriptionService::new(stripe.clone(), pool.clone()),
            usage: UsageMeter::new(pool.clone()),
            webhooks: WebhookHandler::new(stripe, pool, email_service),
        }
    }
}
