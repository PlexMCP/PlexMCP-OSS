//! Stripe webhook handling
//!
//! Handles Stripe events for subscriptions, invoices, and billing updates.
//! Also handles instant charges, early payments, and spend cap unpause.

use hmac::{Hmac, Mac};
use sha2::Sha256;
use sqlx::PgPool;
use stripe::{Event, EventObject, EventType, Invoice, Subscription, Webhook};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::client::StripeClient;
use crate::email::BillingEmailService;
use crate::error::{BillingError, BillingResult};
use crate::events::{ActorType, BillingEventBuilder, BillingEventLogger, BillingEventType};
use crate::instant_charge::InstantChargeService;
use crate::member_suspension::MemberSuspensionService;
use crate::overage::OverageService;
use crate::spend_cap::SpendCapService;
use crate::subscriptions::SubscriptionService;

type HmacSha256 = Hmac<Sha256>;

/// Webhook handler for Stripe events
pub struct WebhookHandler {
    stripe: StripeClient,
    pool: PgPool,
    email: BillingEmailService,
    event_logger: BillingEventLogger,
}

impl WebhookHandler {
    pub fn new(stripe: StripeClient, pool: PgPool, email: BillingEmailService) -> Self {
        let event_logger = BillingEventLogger::new(pool.clone());
        Self {
            stripe,
            pool,
            email,
            event_logger,
        }
    }

    /// Verify and parse a Stripe webhook event
    ///
    /// Uses manual signature verification to work around async-stripe version
    /// incompatibility with newer Stripe API versions.
    pub fn verify_event(&self, payload: &str, signature: &str) -> BillingResult<Event> {
        let webhook_secret = &self.stripe.config().webhook_secret;

        // Debug logging for webhook troubleshooting
        tracing::info!(
            payload_len = payload.len(),
            signature_len = signature.len(),
            secret_prefix = &webhook_secret[..std::cmp::min(12, webhook_secret.len())],
            secret_len = webhook_secret.len(),
            "Webhook verify_event - debug info"
        );

        // Try the standard method first
        match Webhook::construct_event(payload, signature, webhook_secret) {
            Ok(event) => {
                tracing::info!("Standard webhook parsing succeeded");
                return Ok(event);
            }
            Err(e) => {
                tracing::warn!(
                    stripe_error = %e,
                    "Standard webhook parsing failed, trying manual verification"
                );
            }
        }

        // Manual signature verification for newer Stripe API versions
        // Parse the signature header: t=timestamp,v1=signature,v0=signature
        let mut timestamp: Option<i64> = None;
        let mut v1_signature: Option<String> = None;

        for part in signature.split(',') {
            let kv: Vec<&str> = part.splitn(2, '=').collect();
            if kv.len() == 2 {
                match kv[0] {
                    "t" => timestamp = kv[1].parse().ok(),
                    "v1" => v1_signature = Some(kv[1].to_string()),
                    _ => {}
                }
            }
        }

        let timestamp = timestamp.ok_or_else(|| {
            tracing::error!("Missing timestamp in signature header");
            BillingError::WebhookSignatureInvalid
        })?;

        let v1_signature = v1_signature.ok_or_else(|| {
            tracing::error!("Missing v1 signature in signature header");
            BillingError::WebhookSignatureInvalid
        })?;

        // Check timestamp tolerance (5 minutes)
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| {
                tracing::error!("System time error: {}", e);
                BillingError::WebhookSignatureInvalid
            })?
            .as_secs() as i64;

        if (now - timestamp).abs() > 300 {
            tracing::error!(
                timestamp = timestamp,
                now = now,
                diff = (now - timestamp).abs(),
                "Webhook timestamp too old"
            );
            return Err(BillingError::WebhookSignatureInvalid);
        }

        // Compute expected signature
        // The secret starts with "whsec_" which is a base64-encoded key
        let secret_key = webhook_secret
            .strip_prefix("whsec_")
            .unwrap_or(webhook_secret);
        let signed_payload = format!("{}.{}", timestamp, payload);

        let mut mac = HmacSha256::new_from_slice(secret_key.as_bytes()).map_err(|_| {
            tracing::error!("Invalid webhook secret key");
            BillingError::WebhookSignatureInvalid
        })?;
        mac.update(signed_payload.as_bytes());
        let computed = hex::encode(mac.finalize().into_bytes());

        // Compare signatures
        if computed != v1_signature {
            tracing::error!(
                computed_sig = %computed,
                received_sig = %v1_signature,
                "Signature mismatch"
            );
            return Err(BillingError::WebhookSignatureInvalid);
        }

        tracing::info!("Manual signature verification passed");

        // Now parse the event using serde_json with default handling for unknown fields
        // The stripe crate's Event type should handle this with #[serde(default)]
        let event: Event = serde_json::from_str(payload).map_err(|e| {
            tracing::error!(
                parse_error = %e,
                "Failed to parse webhook event JSON"
            );
            BillingError::WebhookSignatureInvalid
        })?;

        tracing::info!(
            event_type = %event.type_,
            event_id = %event.id,
            "Manual webhook parsing succeeded"
        );

        Ok(event)
    }

    /// Handle a verified Stripe event
    ///
    /// SOC 2 CC7.1: Implements atomic idempotency to prevent replay attacks.
    /// Uses INSERT...ON CONFLICT...RETURNING to atomically claim exclusive processing rights.
    /// This prevents race conditions where two concurrent webhooks could both pass an EXISTS check.
    pub async fn handle_event(&self, event: Event) -> BillingResult<()> {
        let event_id = event.id.to_string();
        let event_type_str = event.type_.to_string();

        // Extract Stripe event timestamp for temporal ordering
        let event_timestamp = OffsetDateTime::from_unix_timestamp(event.created)
            .unwrap_or_else(|_| OffsetDateTime::now_utc());

        // SOC 2 CC7.1: ATOMIC idempotency check with timeout recovery
        // This INSERT...ON CONFLICT...RETURNING pattern ensures only ONE concurrent
        // request can claim processing rights. If the insert succeeds (returns a row),
        // we have exclusive claim. If it returns None, another process already claimed it.
        //
        // Additionally, we allow re-claiming events that have been stuck in "processing"
        // for over 30 minutes (timeout recovery).
        const PROCESSING_TIMEOUT_MINUTES: i32 = 30;

        let claimed: Option<(Uuid,)> = sqlx::query_as(
            r#"
            INSERT INTO stripe_webhook_events
                (stripe_event_id, event_type, event_timestamp, processing_result, processing_started_at)
            VALUES ($1, $2, $3, 'processing', NOW())
            ON CONFLICT (stripe_event_id) DO UPDATE SET
                processing_result = 'processing',
                processing_started_at = NOW(),
                error_message = CONCAT('Recovered from stuck state at ', NOW()::TEXT)
            WHERE stripe_webhook_events.processing_result = 'processing'
              AND stripe_webhook_events.processing_started_at < NOW() - ($4 || ' minutes')::INTERVAL
            RETURNING id
            "#
        )
        .bind(&event_id)
        .bind(&event_type_str)
        .bind(event_timestamp)
        .bind(PROCESSING_TIMEOUT_MINUTES)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(
                event_id = %event_id,
                error = %e,
                "Failed to claim webhook event for processing"
            );
            BillingError::Database(e.to_string())
        })?;

        if claimed.is_none() {
            // Check if it was already processed successfully or is being actively processed
            let existing_status: Option<(String,)> = sqlx::query_as(
                "SELECT processing_result FROM stripe_webhook_events WHERE stripe_event_id = $1",
            )
            .bind(&event_id)
            .fetch_optional(&self.pool)
            .await
            .ok()
            .flatten();

            let reason = match existing_status {
                Some((status,)) if status == "success" => "already processed successfully",
                Some((status,)) if status == "processing" => {
                    "currently being processed by another worker"
                }
                Some(_) => "exists with another status",
                None => "unknown (race condition?)",
            };

            tracing::info!(
                event_id = %event_id,
                event_type = %event_type_str,
                reason = %reason,
                "Duplicate webhook event - atomic idempotency check"
            );
            return Ok(());
        }

        tracing::info!(
            event_type = %event.type_,
            event_id = %event.id,
            "Processing Stripe webhook event (claimed exclusive processing rights)"
        );

        // Process the event
        let result = self.process_event_internal(&event).await;

        // Update the event record with processing result
        let (processing_result, error_message) = match &result {
            Ok(()) => ("success".to_string(), None),
            Err(e) => ("error".to_string(), Some(e.to_string())),
        };

        // Update the event record - retry once on failure since this is important for audit
        let update_result = sqlx::query(
            r#"
            UPDATE stripe_webhook_events
            SET processing_result = $1, error_message = $2
            WHERE stripe_event_id = $3
            "#,
        )
        .bind(&processing_result)
        .bind(&error_message)
        .bind(&event_id)
        .execute(&self.pool)
        .await;

        if let Err(e) = update_result {
            // Retry once - audit record is important for idempotency
            tracing::warn!(
                event_id = %event_id,
                error = %e,
                "First attempt to update webhook event failed, retrying..."
            );

            if let Err(retry_err) = sqlx::query(
                r#"
                UPDATE stripe_webhook_events
                SET processing_result = $1, error_message = $2
                WHERE stripe_event_id = $3
                "#,
            )
            .bind(&processing_result)
            .bind(&error_message)
            .bind(&event_id)
            .execute(&self.pool)
            .await
            {
                // Log at error level with full context for debugging
                tracing::error!(
                    event_id = %event_id,
                    event_type = %event.type_,
                    processing_result = %processing_result,
                    error_message = ?error_message,
                    first_error = %e,
                    retry_error = %retry_err,
                    "CRITICAL: Failed to update webhook audit record after retry. \
                     Event may appear stuck in 'processing' state. \
                     Manual intervention may be required."
                );
            }
        }

        result
    }

    /// Internal event processing logic
    async fn process_event_internal(&self, event: &Event) -> BillingResult<()> {
        // Clone the event for handlers that need ownership
        let event_owned = event.clone();

        match event.type_ {
            // Subscription events
            EventType::CustomerSubscriptionCreated => {
                self.handle_subscription_created(event_owned).await?;
            }
            EventType::CustomerSubscriptionUpdated => {
                self.handle_subscription_updated(event_owned).await?;
            }
            EventType::CustomerSubscriptionDeleted => {
                self.handle_subscription_deleted(event_owned).await?;
            }
            EventType::CustomerSubscriptionTrialWillEnd => {
                self.handle_trial_will_end(event_owned).await?;
            }

            // Invoice events
            EventType::InvoicePaid => {
                self.handle_invoice_paid(event_owned).await?;
            }
            EventType::InvoicePaymentFailed => {
                self.handle_invoice_payment_failed(event_owned).await?;
            }
            EventType::InvoiceFinalized => {
                self.handle_invoice_finalized(event_owned).await?;
            }
            EventType::InvoiceUpcoming => {
                self.handle_invoice_upcoming(event_owned).await?;
            }

            // Checkout events
            EventType::CheckoutSessionCompleted => {
                self.handle_checkout_completed(event_owned).await?;
            }

            // Charge events
            EventType::ChargeFailed => {
                self.handle_charge_failed(event_owned).await?;
            }
            EventType::ChargeRefunded => {
                self.handle_charge_refunded(event_owned).await?;
            }
            EventType::ChargeDisputeCreated => {
                self.handle_charge_dispute_created(event_owned).await?;
            }

            // Payment intent events
            EventType::PaymentIntentPaymentFailed => {
                self.handle_payment_intent_failed(event_owned).await?;
            }

            // Customer events
            EventType::CustomerCreated => {
                self.handle_customer_created(event_owned).await?;
            }
            EventType::CustomerUpdated => {
                self.handle_customer_updated(event_owned).await?;
            }
            EventType::CustomerDeleted => {
                self.handle_customer_deleted(event_owned).await?;
            }

            _ => {
                // Log at info level so we can track which events we're not handling
                // This helps identify new events that may need handlers
                tracing::info!(
                    event_type = %event.type_,
                    event_id = %event.id,
                    "Received unhandled Stripe event type - no handler configured"
                );
            }
        }

        Ok(())
    }

    async fn handle_subscription_created(&self, event: Event) -> BillingResult<()> {
        let event_id = event.id.to_string();
        let subscription = self.extract_subscription(event)?;
        let org_id = self.get_org_id_from_metadata(&subscription.metadata)?;

        let sub_service = SubscriptionService::new(self.stripe.clone(), self.pool.clone());
        sub_service
            .sync_subscription_to_db(org_id, &subscription)
            .await?;

        // Log billing event
        let tier = subscription
            .metadata
            .get("tier")
            .cloned()
            .unwrap_or_default();
        if let Err(e) = self
            .event_logger
            .log_event(
                BillingEventBuilder::new(org_id, BillingEventType::SubscriptionCreated)
                    .data(serde_json::json!({
                        "tier": tier,
                        "status": format!("{:?}", subscription.status),
                    }))
                    .stripe_event(&event_id)
                    .stripe_subscription(subscription.id.to_string())
                    .actor_type(ActorType::Stripe),
            )
            .await
        {
            tracing::warn!(error = %e, "Failed to log subscription created event");
        }

        tracing::info!(
            org_id = %org_id,
            subscription_id = %subscription.id,
            "Subscription created"
        );

        Ok(())
    }

    async fn handle_subscription_updated(&self, event: Event) -> BillingResult<()> {
        let event_id = event.id.to_string();
        let subscription = self.extract_subscription(event)?;
        let org_id = self.get_org_id_from_metadata(&subscription.metadata)?;

        let sub_service = SubscriptionService::new(self.stripe.clone(), self.pool.clone());
        sub_service
            .sync_subscription_to_db(org_id, &subscription)
            .await?;

        // Log billing event
        if let Err(e) = self
            .event_logger
            .log_event(
                BillingEventBuilder::new(org_id, BillingEventType::SubscriptionUpdated)
                    .data(serde_json::json!({
                        "status": format!("{:?}", subscription.status),
                        "cancel_at_period_end": subscription.cancel_at_period_end,
                    }))
                    .stripe_event(&event_id)
                    .stripe_subscription(subscription.id.to_string())
                    .actor_type(ActorType::Stripe),
            )
            .await
        {
            tracing::warn!(error = %e, "Failed to log subscription updated event");
        }

        // Check if subscription is past due and needs attention
        if subscription.status == stripe::SubscriptionStatus::PastDue {
            tracing::warn!(
                org_id = %org_id,
                subscription_id = %subscription.id,
                "Subscription is past due"
            );
            // Send notification email to org owner
            if let Ok(Some((email, org_name))) = self.get_org_owner_email(org_id).await {
                if let Err(e) = self
                    .email
                    .send_subscription_past_due(&email, &org_name)
                    .await
                {
                    tracing::error!(error = %e, "Failed to send past due email");
                }
            }
        }

        tracing::info!(
            org_id = %org_id,
            subscription_id = %subscription.id,
            status = ?subscription.status,
            "Subscription updated"
        );

        Ok(())
    }

    async fn handle_subscription_deleted(&self, event: Event) -> BillingResult<()> {
        let event_id = event.id.to_string();
        let subscription = self.extract_subscription(event)?;
        let org_id = self.get_org_id_from_metadata(&subscription.metadata)?;

        // Log billing event
        if let Err(e) = self
            .event_logger
            .log_event(
                BillingEventBuilder::new(org_id, BillingEventType::SubscriptionCanceled)
                    .data(serde_json::json!({
                        "previous_status": format!("{:?}", subscription.status),
                        "period_end": subscription.current_period_end,
                    }))
                    .stripe_event(&event_id)
                    .stripe_subscription(subscription.id.to_string())
                    .actor_type(ActorType::Stripe),
            )
            .await
        {
            tracing::warn!(error = %e, "Failed to log subscription deleted event");
        }

        // Downgrade to free tier
        sqlx::query(
            "UPDATE organizations SET subscription_tier = 'free', updated_at = NOW() WHERE id = $1",
        )
        .bind(org_id)
        .execute(&self.pool)
        .await?;

        // Update subscription status
        sqlx::query(
            r#"
            UPDATE subscriptions SET status = 'canceled', updated_at = NOW()
            WHERE stripe_subscription_id = $1
            "#,
        )
        .bind(subscription.id.as_str())
        .execute(&self.pool)
        .await?;

        // Send cancellation confirmation email
        if let Ok(Some((email, org_name))) = self.get_org_owner_email(org_id).await {
            let end_date = OffsetDateTime::from_unix_timestamp(subscription.current_period_end)
                .map(|dt| dt.date().to_string())
                .unwrap_or_else(|_| "soon".to_string());
            if let Err(e) = self
                .email
                .send_subscription_cancelled(&email, &org_name, &end_date)
                .await
            {
                tracing::error!(error = %e, "Failed to send cancellation email");
            }
        }

        tracing::info!(
            org_id = %org_id,
            subscription_id = %subscription.id,
            "Subscription cancelled, downgraded to free tier"
        );

        Ok(())
    }

    async fn handle_trial_will_end(&self, event: Event) -> BillingResult<()> {
        let event_id = event.id.to_string();
        let subscription = self.extract_subscription(event)?;
        let org_id = self.get_org_id_from_metadata(&subscription.metadata)?;

        // Log billing event
        if let Err(e) = self
            .event_logger
            .log_event(
                BillingEventBuilder::new(org_id, BillingEventType::TrialEnding)
                    .data(serde_json::json!({
                        "trial_end": subscription.trial_end,
                        "days_remaining": 3,
                    }))
                    .stripe_event(&event_id)
                    .stripe_subscription(subscription.id.to_string())
                    .actor_type(ActorType::Stripe),
            )
            .await
        {
            tracing::warn!(error = %e, "Failed to log trial ending event");
        }

        tracing::info!(
            org_id = %org_id,
            subscription_id = %subscription.id,
            trial_end = ?subscription.trial_end,
            "Trial period ending soon"
        );

        // Send trial ending notification email
        if let Ok(Some((email, org_name))) = self.get_org_owner_email(org_id).await {
            // Calculate days remaining (trial_will_end fires 3 days before)
            let days_remaining = if let Some(end) = subscription.trial_end {
                let now = OffsetDateTime::now_utc().unix_timestamp();
                ((end - now) / 86400).max(1) // At least 1 day
            } else {
                3 // Default to 3 days
            };

            // Get tier from plan
            let tier = subscription
                .items
                .data
                .first()
                .and_then(|item| item.price.as_ref())
                .and_then(|price| price.nickname.as_ref())
                .map(|s| s.as_str())
                .unwrap_or("Pro");

            if let Err(e) = self
                .email
                .send_trial_ending(&email, &org_name, days_remaining, tier)
                .await
            {
                tracing::error!(error = %e, "Failed to send trial ending email");
            }
        }

        Ok(())
    }

    async fn handle_invoice_paid(&self, event: Event) -> BillingResult<()> {
        let event_id = event.id.to_string();
        let invoice = self.extract_invoice(event)?;

        // Get org_id from customer
        let org_id = self.get_org_id_from_customer(&invoice.customer).await?;

        // Store invoice record
        self.store_invoice(org_id, &invoice, "paid").await?;

        let invoice_id = invoice.id.to_string();

        // Log billing event
        if let Err(e) = self.event_logger.log_event(
            BillingEventBuilder::new(org_id, BillingEventType::InvoicePaid)
                .data(serde_json::json!({
                    "amount_paid_cents": invoice.amount_paid,
                    "total_cents": invoice.total,
                    "billing_reason": invoice.billing_reason.as_ref().map(|r| format!("{:?}", r)),
                }))
                .stripe_event(&event_id)
                .stripe_invoice(&invoice_id)
                .actor_type(ActorType::Stripe)
        ).await {
            tracing::warn!(error = %e, "Failed to log invoice paid event");
        }

        // Check for instant charge payment
        let instant_charge_service =
            InstantChargeService::new(self.stripe.clone(), self.pool.clone(), self.email.clone());
        if let Ok(Some(charge)) = instant_charge_service.mark_paid(&invoice_id).await {
            tracing::info!(
                org_id = %org_id,
                charge_id = %charge.id,
                invoice_id = %invoice_id,
                "Instant charge marked as paid"
            );
        }

        // Check for early overage payment (Pay Now feature)
        let overage_service = OverageService::new(self.stripe.clone(), self.pool.clone());
        let early_payments_marked = overage_service.mark_early_payment_paid(&invoice_id).await?;
        if early_payments_marked > 0 {
            tracing::info!(
                org_id = %org_id,
                invoice_id = %invoice_id,
                charges_marked = early_payments_marked,
                "Early overage payments marked as paid"
            );

            // Send confirmation email
            if let Ok(Some((email, org_name))) = self.get_org_owner_email(org_id).await {
                let amount_cents = invoice.amount_paid.unwrap_or(0) as i32;
                if let Err(e) = self
                    .email
                    .send_pay_now_confirmation(
                        &email,
                        &org_name,
                        amount_cents,
                        early_payments_marked,
                    )
                    .await
                {
                    tracing::error!(error = %e, "Failed to send pay now confirmation email");
                }
            }
        }

        // Also check for regular invoiced overage charges (from billing cycle)
        let invoiced_charges_marked = overage_service
            .mark_invoiced_charges_paid(org_id, &invoice_id)
            .await
            .unwrap_or_else(|e| {
                // Log at error level - this is a data inconsistency that needs attention
                // We continue processing because the invoice is already paid in Stripe,
                // retrying won't help, but we need to flag this for manual reconciliation
                tracing::error!(
                    org_id = %org_id,
                    invoice_id = %invoice_id,
                    error = %e,
                    "RECONCILIATION NEEDED: Failed to mark invoiced overage charges as paid. \
                     Invoice is paid in Stripe but internal overage records may be inconsistent."
                );
                0
            });
        if invoiced_charges_marked > 0 {
            tracing::info!(
                org_id = %org_id,
                invoice_id = %invoice_id,
                charges_marked = invoiced_charges_marked,
                "Regular overage charges marked as paid"
            );
        }

        // Also mark any pending charges for the billing period as paid
        // This handles charges tracked for display that were billed via metered usage
        if let (Some(period_start), Some(period_end)) = (invoice.period_start, invoice.period_end) {
            let period_charges_marked = overage_service
                .mark_period_charges_paid(
                    org_id,
                    OffsetDateTime::from_unix_timestamp(period_start).unwrap_or(OffsetDateTime::now_utc()),
                    OffsetDateTime::from_unix_timestamp(period_end).unwrap_or(OffsetDateTime::now_utc()),
                    &invoice_id,
                )
                .await
                .unwrap_or_else(|e| {
                    // Log at error level - this is a data inconsistency that needs attention
                    tracing::error!(
                        org_id = %org_id,
                        invoice_id = %invoice_id,
                        error = %e,
                        "RECONCILIATION NEEDED: Failed to mark period overage charges as paid. \
                         Invoice is paid in Stripe but internal overage records may be inconsistent."
                    );
                    0
                });
            if period_charges_marked > 0 {
                tracing::info!(
                    org_id = %org_id,
                    invoice_id = %invoice_id,
                    charges_marked = period_charges_marked,
                    "Period overage charges marked as paid via metered billing"
                );
            }
        }

        // Check if org was paused due to spend cap and should be unpaused
        let spend_cap_service = SpendCapService::new(self.pool.clone(), self.email.clone());
        if spend_cap_service.check_paused(org_id).await? {
            // Unpause since payment was received
            spend_cap_service.unpause_org(org_id).await?;
            tracing::info!(org_id = %org_id, "Organization unpaused after payment");
        }

        // Check for scheduled downgrades at billing period renewal
        // This invoice payment means a new billing period has started
        // If user scheduled a downgrade, now is the time to process it
        let sub_service = SubscriptionService::new(self.stripe.clone(), self.pool.clone());
        match sub_service.process_scheduled_downgrade(org_id).await {
            Ok(Some(subscription)) => {
                let new_tier = subscription
                    .metadata
                    .get("tier")
                    .cloned()
                    .unwrap_or_else(|| "lower tier".to_string());
                tracing::info!(
                    org_id = %org_id,
                    new_tier = %new_tier,
                    "Processed scheduled downgrade at billing period renewal"
                );

                // Suspend excess members due to plan downgrade
                let member_service = MemberSuspensionService::new(self.pool.clone());
                match member_service
                    .suspend_excess_members(org_id, &new_tier, "plan_downgrade")
                    .await
                {
                    Ok(result) if result.suspended_count > 0 => {
                        tracing::info!(
                            org_id = %org_id,
                            suspended_count = result.suspended_count,
                            new_tier = %new_tier,
                            "Suspended excess members after downgrade"
                        );

                        // Send notification to suspended members
                        for member in &result.suspended_members {
                            if let Err(e) = self
                                .email
                                .send_member_suspended(&member.email, &new_tier)
                                .await
                            {
                                tracing::error!(
                                    error = %e,
                                    email = %member.email,
                                    "Failed to send member suspension email"
                                );
                            }
                        }
                    }
                    Ok(_) => {
                        // No excess members to suspend
                    }
                    Err(e) => {
                        tracing::error!(
                            org_id = %org_id,
                            error = %e,
                            "Failed to suspend excess members after downgrade"
                        );
                    }
                }

                // Send email notification about the downgrade
                if let Ok(Some((email, org_name))) = self.get_org_owner_email(org_id).await {
                    if let Err(e) = self
                        .email
                        .send_subscription_downgraded(&email, &org_name, &new_tier)
                        .await
                    {
                        tracing::error!(error = %e, "Failed to send downgrade confirmation email");
                    }
                }
            }
            Ok(None) => {
                // No scheduled downgrade, nothing to do
            }
            Err(e) => {
                tracing::error!(
                    org_id = %org_id,
                    error = %e,
                    "Failed to process scheduled downgrade"
                );
            }
        }

        tracing::info!(
            org_id = %org_id,
            invoice_id = %invoice_id,
            amount = invoice.amount_paid,
            "Invoice paid"
        );

        Ok(())
    }

    async fn handle_invoice_payment_failed(&self, event: Event) -> BillingResult<()> {
        let event_id = event.id.to_string();
        let invoice = self.extract_invoice(event)?;

        // Get org_id from customer
        let org_id = self.get_org_id_from_customer(&invoice.customer).await?;

        // Store invoice record
        self.store_invoice(org_id, &invoice, "uncollectible")
            .await?;

        let invoice_id = invoice.id.to_string();
        let attempt_count = invoice.attempt_count.unwrap_or(0) as i32;

        // Track retry attempts in database for escalation logic
        let _updated: Option<(i32,)> = sqlx::query_as(
            r#"
            UPDATE invoices
            SET attempt_count = COALESCE(attempt_count, 0) + 1,
                last_attempt_at = NOW(),
                updated_at = NOW()
            WHERE stripe_invoice_id = $1
            RETURNING attempt_count
            "#,
        )
        .bind(&invoice_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| BillingError::Database(e.to_string()))?;

        // Log billing event with retry information
        if let Err(e) = self.event_logger.log_event(
            BillingEventBuilder::new(org_id, BillingEventType::InvoiceFailed)
                .data(serde_json::json!({
                    "amount_due_cents": invoice.amount_due,
                    "billing_reason": invoice.billing_reason.as_ref().map(|r| format!("{:?}", r)),
                    "attempt_count": attempt_count,
                    "next_attempt": invoice.next_payment_attempt.map(|t| t.to_string()),
                }))
                .stripe_event(&event_id)
                .stripe_invoice(&invoice_id)
                .actor_type(ActorType::Stripe)
        ).await {
            tracing::warn!(error = %e, "Failed to log invoice payment failed event");
        }

        // Check if this was an instant charge and mark it failed
        let instant_charge_service =
            InstantChargeService::new(self.stripe.clone(), self.pool.clone(), self.email.clone());
        instant_charge_service
            .mark_failed(&invoice_id, "Payment failed")
            .await?;

        tracing::warn!(
            org_id = %org_id,
            invoice_id = %invoice_id,
            amount = invoice.amount_due,
            attempt_count = attempt_count,
            "Invoice payment failed"
        );

        // Send payment failed notification email (content varies by attempt count)
        if let Ok(Some((email, org_name))) = self.get_org_owner_email(org_id).await {
            let amount_cents = invoice.amount_due.unwrap_or(0);
            let invoice_url = invoice.hosted_invoice_url.as_deref();

            // Escalate notification based on attempt count
            match attempt_count {
                0..=1 => {
                    // First attempt - standard notification
                    if let Err(e) = self
                        .email
                        .send_payment_failed_invoice(&email, &org_name, amount_cents, invoice_url)
                        .await
                    {
                        tracing::error!(error = %e, "Failed to send payment failed email");
                    }
                }
                2..=3 => {
                    // Second/third attempt - urgent notification
                    tracing::warn!(
                        org_id = %org_id,
                        attempt_count = attempt_count,
                        "Payment failed multiple times - sending urgent notification"
                    );
                    if let Err(e) = self
                        .email
                        .send_payment_failed_invoice(&email, &org_name, amount_cents, invoice_url)
                        .await
                    {
                        tracing::error!(error = %e, "Failed to send urgent payment failed email");
                    }
                }
                _ => {
                    // 4+ attempts - final warning, service may be suspended
                    tracing::error!(
                        org_id = %org_id,
                        attempt_count = attempt_count,
                        "Payment failed repeatedly - service suspension may occur"
                    );
                    // Send final warning
                    if let Err(e) = self
                        .email
                        .send_payment_failed_invoice(&email, &org_name, amount_cents, invoice_url)
                        .await
                    {
                        tracing::error!(error = %e, "Failed to send final payment warning email");
                    }

                    // Mark org as past_due_locked if too many failures
                    // (SOC 2: Access control based on payment status)
                    if let Err(e) = sqlx::query(
                        "UPDATE organizations SET billing_blocked_at = NOW() WHERE id = $1 AND billing_blocked_at IS NULL"
                    )
                    .bind(org_id)
                    .execute(&self.pool)
                    .await {
                        tracing::error!(
                            org_id = %org_id,
                            error = %e,
                            "Failed to block billing for org after repeated payment failures"
                        );
                    }
                }
            }
        }

        Ok(())
    }

    async fn handle_invoice_finalized(&self, event: Event) -> BillingResult<()> {
        let invoice = self.extract_invoice(event)?;

        // Get org_id from customer
        let org_id = self.get_org_id_from_customer(&invoice.customer).await?;

        // Store invoice record
        self.store_invoice(org_id, &invoice, "open").await?;

        tracing::info!(
            org_id = %org_id,
            invoice_id = %invoice.id,
            "Invoice finalized"
        );

        Ok(())
    }

    /// Handle invoice.upcoming webhook
    ///
    /// Stripe sends this ~3 days before a subscription renews.
    /// We use it to notify customers about upcoming charges and pending overages.
    async fn handle_invoice_upcoming(&self, event: Event) -> BillingResult<()> {
        let invoice = self.extract_invoice(event)?;

        // Get org_id from customer
        let org_id = self.get_org_id_from_customer(&invoice.customer).await?;

        let invoice_id = invoice.id.to_string();
        let amount_due = invoice.amount_due.unwrap_or(0);

        // Check for pending overages that will be added to this invoice
        let overage_service = OverageService::new(self.stripe.clone(), self.pool.clone());
        let pending_overages = overage_service
            .get_pending_overage_total(org_id)
            .await
            .unwrap_or(0);

        let total_expected = amount_due + pending_overages;

        tracing::info!(
            org_id = %org_id,
            invoice_id = %invoice_id,
            subscription_amount_cents = amount_due,
            pending_overage_cents = pending_overages,
            total_expected_cents = total_expected,
            "Upcoming invoice notification"
        );

        // Send notification email if there are pending overages or significant amount due
        if total_expected > 0 {
            if let Ok(Some((email, org_name))) = self.get_org_owner_email(org_id).await {
                if let Err(e) = self
                    .email
                    .send_upcoming_invoice(
                        &email,
                        &org_name,
                        amount_due as i32,
                        pending_overages as i32,
                    )
                    .await
                {
                    tracing::error!(
                        org_id = %org_id,
                        error = %e,
                        "Failed to send upcoming invoice notification email"
                    );
                }
            }
        }

        // Log billing event
        if let Err(e) = self
            .event_logger
            .log_event(
                BillingEventBuilder::new(org_id, BillingEventType::InvoiceUpcoming)
                    .data(serde_json::json!({
                        "invoice_id": invoice_id,
                        "subscription_amount_cents": amount_due,
                        "pending_overage_cents": pending_overages,
                        "total_expected_cents": total_expected,
                    }))
                    .stripe_event(&invoice_id)
                    .actor_type(ActorType::Stripe),
            )
            .await
        {
            tracing::warn!(error = %e, "Failed to log invoice upcoming event");
        }

        Ok(())
    }

    async fn handle_checkout_completed(&self, event: Event) -> BillingResult<()> {
        let session = match event.data.object {
            EventObject::CheckoutSession(session) => session,
            _ => {
                return Err(BillingError::WebhookEventNotSupported(
                    "Expected CheckoutSession".to_string(),
                ))
            }
        };

        if let Some(metadata) = &session.metadata {
            if let Some(org_id_str) = metadata.get("org_id") {
                let org_id = Uuid::parse_str(org_id_str)
                    .map_err(|e| BillingError::Internal(format!("Invalid org_id: {}", e)))?;

                // Check if this is an overage payment checkout
                let checkout_type = metadata.get("checkout_type").map(|s| s.as_str());

                if checkout_type == Some("overage_payment") {
                    // Handle overage payment completion
                    let session_id = session.id.to_string();
                    let overage_service =
                        OverageService::new(self.stripe.clone(), self.pool.clone());

                    match overage_service.mark_early_payment_paid(&session_id).await {
                        Ok(count) if count > 0 => {
                            tracing::info!(
                                org_id = %org_id,
                                session_id = %session_id,
                                charges_marked_paid = count,
                                "Overage payment completed via checkout"
                            );

                            // Create invoice record for billing history
                            let amount_cents = session.amount_total.unwrap_or(0) as i32;
                            if amount_cents > 0 {
                                let invoice_result = sqlx::query(
                                    r#"
                                    INSERT INTO invoices (
                                        org_id, stripe_invoice_id, amount_cents, amount_paid_cents,
                                        currency, status, description, paid_at, billing_reason
                                    )
                                    VALUES ($1, $2, $3, $3, 'usd', 'paid', 'Early overage payment', NOW(), 'overage_payment')
                                    ON CONFLICT (stripe_invoice_id) DO NOTHING
                                    "#
                                )
                                .bind(org_id)
                                .bind(&session_id)
                                .bind(amount_cents)
                                .execute(&self.pool)
                                .await;

                                match invoice_result {
                                    Ok(_) => {
                                        tracing::info!(
                                            org_id = %org_id,
                                            amount_cents = amount_cents,
                                            "Created invoice record for overage payment"
                                        );
                                    }
                                    Err(e) => {
                                        tracing::error!(
                                            org_id = %org_id,
                                            error = %e,
                                            "Failed to create invoice record for overage payment"
                                        );
                                    }
                                }
                            }
                        }
                        Ok(_) => {
                            tracing::warn!(
                                org_id = %org_id,
                                session_id = %session_id,
                                "Overage checkout completed but no charges found to mark as paid"
                            );
                        }
                        Err(e) => {
                            tracing::error!(
                                org_id = %org_id,
                                session_id = %session_id,
                                error = %e,
                                "Failed to mark overage charges as paid"
                            );
                        }
                    }

                    return Ok(());
                }

                // Handle upgrade payment checkout completion
                if checkout_type == Some("upgrade_payment") {
                    let new_tier = metadata.get("new_tier").map(|s| s.as_str()).unwrap_or("");
                    let billing_interval = metadata
                        .get("billing_interval")
                        .map(|s| s.as_str())
                        .unwrap_or("monthly");
                    let overage_cents: i64 = metadata
                        .get("overage_cents")
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(0);

                    if new_tier.is_empty() {
                        tracing::error!(
                            org_id = %org_id,
                            "Upgrade checkout completed but no new_tier in metadata"
                        );
                        return Ok(());
                    }

                    tracing::info!(
                        org_id = %org_id,
                        new_tier = %new_tier,
                        billing_interval = %billing_interval,
                        overage_cents = overage_cents,
                        "Processing upgrade payment checkout completion"
                    );

                    // 1. Update the subscription to the new tier
                    let sub_service =
                        SubscriptionService::new(self.stripe.clone(), self.pool.clone());
                    match sub_service
                        .upgrade_subscription_after_payment(org_id, new_tier, billing_interval)
                        .await
                    {
                        Ok(_) => {
                            tracing::info!(
                                org_id = %org_id,
                                new_tier = %new_tier,
                                "Successfully upgraded subscription after payment"
                            );
                        }
                        Err(e) => {
                            tracing::error!(
                                org_id = %org_id,
                                new_tier = %new_tier,
                                error = %e,
                                "Failed to upgrade subscription after payment - MANUAL INTERVENTION REQUIRED"
                            );
                            // Don't return error - payment was successful, just log for manual fix
                        }
                    }

                    // 2. Mark overages as paid (if any were included)
                    if overage_cents > 0 {
                        let overage_service =
                            OverageService::new(self.stripe.clone(), self.pool.clone());
                        match overage_service.mark_upgrade_overages_paid(org_id).await {
                            Ok(count) => {
                                tracing::info!(
                                    org_id = %org_id,
                                    charges_paid = count,
                                    "Marked upgrade overages as paid"
                                );
                            }
                            Err(e) => {
                                tracing::error!(
                                    org_id = %org_id,
                                    error = %e,
                                    "Failed to mark upgrade overages as paid"
                                );
                            }
                        }
                    }

                    // 3. Create invoice record for billing history
                    let amount_cents = session.amount_total.unwrap_or(0) as i32;
                    if amount_cents > 0 {
                        let session_id = session.id.to_string();
                        let _ = sqlx::query(
                            r#"
                            INSERT INTO invoices (
                                org_id, stripe_invoice_id, amount_cents, amount_paid_cents,
                                currency, status, description, paid_at, billing_reason
                            )
                            VALUES ($1, $2, $3, $3, 'usd', 'paid', $4, NOW(), 'upgrade_payment')
                            ON CONFLICT (stripe_invoice_id) DO NOTHING
                            "#,
                        )
                        .bind(org_id)
                        .bind(&session_id)
                        .bind(amount_cents)
                        .bind(format!("Upgrade to {} plan", new_tier))
                        .execute(&self.pool)
                        .await;
                    }

                    return Ok(());
                }

                // Handle addon checkout completion
                if checkout_type == Some("addon") {
                    let addon_type = metadata.get("addon_type").map(|s| s.as_str()).unwrap_or("");
                    let quantity: u32 = metadata
                        .get("quantity")
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(1);

                    if addon_type.is_empty() {
                        tracing::error!(
                            org_id = %org_id,
                            "Addon checkout completed but no addon_type in metadata"
                        );
                        return Ok(());
                    }

                    tracing::info!(
                        org_id = %org_id,
                        addon_type = %addon_type,
                        quantity = %quantity,
                        "Processing addon checkout completion"
                    );

                    // Get the EXISTING internal subscription ID first
                    // (Don't sync the new addon subscription - we use the existing one)
                    let internal_sub_id: Option<(Uuid,)> = sqlx::query_as(
                        "SELECT id FROM subscriptions WHERE org_id = $1 AND status = 'active' LIMIT 1"
                    )
                    .bind(org_id)
                    .fetch_optional(&self.pool)
                    .await?;

                    let internal_sub_id = match internal_sub_id {
                        Some((id,)) => id,
                        None => {
                            // Addon purchases require an existing subscription
                            // This is an error condition that should not happen in normal flow
                            tracing::error!(
                                org_id = %org_id,
                                addon_type = %addon_type,
                                session_id = %session.id,
                                "Cannot process addon checkout: no active subscription found for org. \
                                 Addon purchases require an existing subscription. \
                                 This may indicate a checkout flow bug or data inconsistency."
                            );
                            return Err(BillingError::SubscriptionRequired(
                                "Addon purchase requires an active subscription".to_string(),
                            ));
                        }
                    };

                    // Get the new subscription from the session to extract addon item info
                    if let Some(subscription_id) = session.subscription {
                        let parsed_sub_id = subscription_id.id().parse().map_err(|e| {
                            tracing::error!("Failed to parse subscription ID: {}", e);
                            BillingError::SubscriptionNotFound(subscription_id.id().to_string())
                        })?;
                        let subscription = stripe::Subscription::retrieve(
                            self.stripe.inner(),
                            &parsed_sub_id,
                            &[],
                        )
                        .await?;

                        // Note: We intentionally DO NOT sync this new subscription to DB
                        // as the org already has an existing subscription. The addon checkout
                        // creates a separate Stripe subscription which we track via subscription_addons.
                        tracing::info!(
                            org_id = %org_id,
                            new_stripe_subscription_id = %subscription.id,
                            "Addon checkout created new Stripe subscription (not syncing to main subscriptions table)"
                        );

                        // Find the addon price ID from config
                        let config_price_id = self.stripe.config().addon_price_id(addon_type);

                        // Find the subscription item for this addon
                        // First try to match by config price, then take the last item (addon)
                        let addon_item = subscription
                            .items
                            .data
                            .iter()
                            .find(|item| {
                                item.price
                                    .as_ref()
                                    .map(|p| {
                                        let item_price = p.id.as_str();
                                        config_price_id
                                            .as_ref()
                                            .map(|pid| item_price == pid)
                                            .unwrap_or(false)
                                    })
                                    .unwrap_or(false)
                            })
                            // If not found by config, take the last item (likely the addon)
                            .or_else(|| subscription.items.data.last());

                        let stripe_item_id = addon_item.map(|item| item.id.to_string());

                        // Get price ID from the item or fall back to config
                        let price_id = addon_item
                            .and_then(|item| item.price.as_ref())
                            .map(|p| p.id.to_string())
                            .or(config_price_id)
                            .unwrap_or_else(|| format!("unknown_{}", addon_type));

                        // Get addon price cents
                        let addon_price_cents = crate::AddonType::from_str(addon_type)
                            .map(|a| a.price_cents())
                            .unwrap_or(0);

                        // Create the subscription_addons record
                        let result = sqlx::query(
                            r#"
                            INSERT INTO subscription_addons (
                                org_id, subscription_id, addon_type, stripe_item_id, stripe_price_id,
                                status, metadata, quantity, unit_price_cents
                            )
                            VALUES ($1, $2, $3, $4, $5, 'active', '{}'::jsonb, $6, $7)
                            ON CONFLICT (org_id, addon_type)
                            DO UPDATE SET
                                status = 'active',
                                stripe_item_id = EXCLUDED.stripe_item_id,
                                stripe_price_id = EXCLUDED.stripe_price_id,
                                subscription_id = EXCLUDED.subscription_id,
                                quantity = CASE
                                    WHEN subscription_addons.status = 'active'
                                    THEN subscription_addons.quantity + EXCLUDED.quantity
                                    ELSE EXCLUDED.quantity
                                END,
                                canceled_at = NULL,
                                updated_at = NOW()
                            "#
                        )
                        .bind(org_id)
                        .bind(internal_sub_id)
                        .bind(addon_type)
                        .bind(stripe_item_id.as_ref())
                        .bind(&price_id)
                        .bind(quantity as i32)
                        .bind(addon_price_cents)
                        .execute(&self.pool)
                        .await;

                        match result {
                            Ok(_) => {
                                tracing::info!(
                                    org_id = %org_id,
                                    addon_type = %addon_type,
                                    quantity = %quantity,
                                    "Successfully created subscription_addons record after checkout"
                                );
                            }
                            Err(e) => {
                                tracing::error!(
                                    org_id = %org_id,
                                    addon_type = %addon_type,
                                    error = %e,
                                    "Failed to create subscription_addons record after checkout"
                                );
                            }
                        }
                    }

                    return Ok(());
                }

                // Get the subscription from the session (for subscription checkouts)
                if let Some(subscription_id) = session.subscription {
                    let parsed_sub_id = subscription_id.id().parse().map_err(|e| {
                        tracing::error!("Failed to parse subscription ID: {}", e);
                        BillingError::SubscriptionNotFound(subscription_id.id().to_string())
                    })?;
                    let subscription =
                        stripe::Subscription::retrieve(self.stripe.inner(), &parsed_sub_id, &[])
                            .await?;

                    let sub_service =
                        SubscriptionService::new(self.stripe.clone(), self.pool.clone());
                    sub_service
                        .sync_subscription_to_db(org_id, &subscription)
                        .await?;

                    tracing::info!(
                        org_id = %org_id,
                        subscription_id = %subscription.id,
                        "Checkout completed, subscription created"
                    );
                }

                // Mark any overages that were included in this upgrade checkout as paid
                // This handles the case where overages were added as invoice items
                let overage_service = OverageService::new(self.stripe.clone(), self.pool.clone());
                match overage_service.mark_upgrade_overages_paid(org_id).await {
                    Ok(count) if count > 0 => {
                        tracing::info!(
                            org_id = %org_id,
                            overages_marked_paid = count,
                            "Marked upgrade overages as paid after checkout completion"
                        );
                    }
                    Err(e) => {
                        tracing::warn!(
                            org_id = %org_id,
                            error = %e,
                            "Failed to mark upgrade overages as paid"
                        );
                    }
                    _ => {}
                }
            }
        }

        Ok(())
    }

    fn extract_subscription(&self, event: Event) -> BillingResult<Subscription> {
        match event.data.object {
            EventObject::Subscription(subscription) => Ok(subscription),
            _ => Err(BillingError::WebhookEventNotSupported(
                "Expected Subscription".to_string(),
            )),
        }
    }

    fn extract_invoice(&self, event: Event) -> BillingResult<Invoice> {
        match event.data.object {
            EventObject::Invoice(invoice) => Ok(invoice),
            _ => Err(BillingError::WebhookEventNotSupported(
                "Expected Invoice".to_string(),
            )),
        }
    }

    fn get_org_id_from_metadata(
        &self,
        metadata: &std::collections::HashMap<String, String>,
    ) -> BillingResult<Uuid> {
        metadata
            .get("org_id")
            .and_then(|id| Uuid::parse_str(id).ok())
            .ok_or_else(|| BillingError::Internal("org_id not found in metadata".to_string()))
    }

    async fn get_org_id_from_customer(
        &self,
        customer: &Option<stripe::Expandable<stripe::Customer>>,
    ) -> BillingResult<Uuid> {
        let customer_id = match customer {
            Some(stripe::Expandable::Id(id)) => id.to_string(),
            Some(stripe::Expandable::Object(c)) => c.id.to_string(),
            None => return Err(BillingError::Internal("No customer on invoice".to_string())),
        };

        let result: Option<(Uuid,)> =
            sqlx::query_as("SELECT id FROM organizations WHERE stripe_customer_id = $1")
                .bind(&customer_id)
                .fetch_optional(&self.pool)
                .await?;

        result
            .map(|(id,)| id)
            .ok_or(BillingError::CustomerNotFound(customer_id))
    }

    /// Get the org owner's email and org name for sending notifications
    async fn get_org_owner_email(&self, org_id: Uuid) -> BillingResult<Option<(String, String)>> {
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

        Ok(result)
    }

    /// Store invoice record with line items and grace period tracking
    async fn store_invoice(
        &self,
        org_id: Uuid,
        invoice: &Invoice,
        status: &str,
    ) -> BillingResult<Uuid> {
        let due_date = invoice
            .due_date
            .map(|t| OffsetDateTime::from_unix_timestamp(t).unwrap_or(OffsetDateTime::now_utc()));

        let paid_at = if status == "paid" {
            Some(OffsetDateTime::now_utc())
        } else {
            None
        };

        // Extract period dates from subscription if available
        let period_start = invoice
            .period_start
            .map(|t| OffsetDateTime::from_unix_timestamp(t).unwrap_or(OffsetDateTime::now_utc()));
        let period_end = invoice
            .period_end
            .map(|t| OffsetDateTime::from_unix_timestamp(t).unwrap_or(OffsetDateTime::now_utc()));

        // Calculate grace period (due_date + 30 days)
        let grace_period_ends_at = due_date.map(|d| d + time::Duration::days(30));

        // Extract customer and subscription IDs
        let stripe_customer_id = match &invoice.customer {
            Some(stripe::Expandable::Id(id)) => Some(id.to_string()),
            Some(stripe::Expandable::Object(c)) => Some(c.id.to_string()),
            None => None,
        };

        let stripe_subscription_id = match &invoice.subscription {
            Some(stripe::Expandable::Id(id)) => Some(id.to_string()),
            Some(stripe::Expandable::Object(s)) => Some(s.id.to_string()),
            None => None,
        };

        let invoice_db_id = Uuid::new_v4();

        // Upsert invoice with all fields including new ones
        sqlx::query(
            r#"
            INSERT INTO invoices (
                id, org_id, stripe_invoice_id, amount_cents, amount_due_cents, amount_paid_cents,
                currency, status, description, billing_reason,
                period_start, period_end, due_date, grace_period_ends_at,
                paid_at, invoice_pdf_url, hosted_invoice_url,
                stripe_customer_id, stripe_subscription_id, created_at
            ) VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, NOW()
            )
            ON CONFLICT (stripe_invoice_id) DO UPDATE SET
                status = EXCLUDED.status,
                amount_due_cents = EXCLUDED.amount_due_cents,
                amount_paid_cents = EXCLUDED.amount_paid_cents,
                paid_at = EXCLUDED.paid_at,
                invoice_pdf_url = EXCLUDED.invoice_pdf_url,
                hosted_invoice_url = EXCLUDED.hosted_invoice_url,
                updated_at = NOW()
            RETURNING id
            "#
        )
        .bind(invoice_db_id)
        .bind(org_id)
        .bind(Some(invoice.id.to_string()))
        .bind(invoice.total.unwrap_or(0) as i32)
        .bind(invoice.amount_due.unwrap_or(0) as i32)
        .bind(invoice.amount_paid.unwrap_or(0) as i32)
        .bind(invoice.currency.as_ref().map(|c| c.to_string()).unwrap_or_else(|| "usd".to_string()))
        .bind(status)
        .bind(invoice.description.as_ref())
        .bind(invoice.billing_reason.as_ref().map(|r| format!("{:?}", r)))
        .bind(period_start)
        .bind(period_end)
        .bind(due_date)
        .bind(grace_period_ends_at)
        .bind(paid_at)
        .bind(invoice.invoice_pdf.as_ref())
        .bind(invoice.hosted_invoice_url.as_ref())
        .bind(stripe_customer_id)
        .bind(stripe_subscription_id)
        .execute(&self.pool)
        .await?;

        // Get the actual invoice ID (either new or existing from upsert)
        let actual_invoice_id: (Uuid,) =
            sqlx::query_as("SELECT id FROM invoices WHERE stripe_invoice_id = $1")
                .bind(invoice.id.to_string())
                .fetch_one(&self.pool)
                .await?;

        let invoice_id = actual_invoice_id.0;

        // Store line items (only on first insert, skip if already exists)
        self.store_invoice_line_items(invoice_id, invoice).await?;

        tracing::debug!(
            org_id = %org_id,
            invoice_id = %invoice_id,
            stripe_invoice_id = %invoice.id,
            line_items_count = invoice.lines.as_ref().map(|l| l.data.len()).unwrap_or(0),
            "Stored invoice with line items"
        );

        Ok(invoice_id)
    }

    /// Store invoice line items from Stripe invoice
    async fn store_invoice_line_items(
        &self,
        invoice_id: Uuid,
        invoice: &Invoice,
    ) -> BillingResult<()> {
        let lines = match &invoice.lines {
            Some(lines) => &lines.data,
            None => return Ok(()),
        };

        for line in lines {
            let period_start = line
                .period
                .as_ref()
                .and_then(|p| p.start)
                .and_then(|ts| OffsetDateTime::from_unix_timestamp(ts).ok());
            let period_end = line
                .period
                .as_ref()
                .and_then(|p| p.end)
                .and_then(|ts| OffsetDateTime::from_unix_timestamp(ts).ok());

            let line_type = format!("{:?}", line.type_);

            let (price_id, product_id, product_name, unit_amount) = match &line.price {
                Some(price) => {
                    let pid = price.id.to_string();
                    let prod_id = match &price.product {
                        Some(stripe::Expandable::Id(id)) => Some(id.to_string()),
                        Some(stripe::Expandable::Object(p)) => Some(p.id.to_string()),
                        None => None,
                    };
                    let prod_name = price.nickname.clone();
                    let unit_amt = price.unit_amount.unwrap_or(0);
                    (Some(pid), prod_id, prod_name, unit_amt)
                }
                None => {
                    // Calculate unit amount from total and quantity
                    let qty = line.quantity.unwrap_or(1) as i64;
                    let unit_amt = if qty > 0 {
                        line.amount / qty
                    } else {
                        line.amount
                    };
                    (None, None, None, unit_amt)
                }
            };

            // Insert line item, skip if already exists (idempotent)
            sqlx::query(
                r#"
                INSERT INTO invoice_line_items (
                    id, invoice_id, stripe_line_item_id, description, quantity,
                    unit_amount_cents, amount_cents, currency, type,
                    period_start, period_end, proration,
                    stripe_price_id, stripe_product_id, product_name
                ) VALUES (
                    $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15
                )
                ON CONFLICT DO NOTHING
                "#,
            )
            .bind(Uuid::new_v4())
            .bind(invoice_id)
            .bind(Some(line.id.to_string()))
            .bind(line.description.as_deref().unwrap_or("Invoice item"))
            .bind(line.quantity.unwrap_or(1) as i32)
            .bind(unit_amount as i32)
            .bind(line.amount as i32)
            .bind(line.currency.to_string())
            .bind(&line_type)
            .bind(period_start)
            .bind(period_end)
            .bind(line.proration)
            .bind(&price_id)
            .bind(&product_id)
            .bind(&product_name)
            .execute(&self.pool)
            .await?;
        }

        Ok(())
    }

    /// Handle charge.failed - payment attempt failed
    async fn handle_charge_failed(&self, event: Event) -> BillingResult<()> {
        let event_id = event.id.to_string();
        let charge = match event.data.object {
            EventObject::Charge(charge) => charge,
            _ => {
                return Err(BillingError::WebhookEventNotSupported(
                    "Expected Charge".to_string(),
                ))
            }
        };

        let customer_id = match &charge.customer {
            Some(stripe::Expandable::Id(id)) => id.to_string(),
            Some(stripe::Expandable::Object(c)) => c.id.to_string(),
            None => {
                tracing::warn!(charge_id = %charge.id, "Charge failed without customer ID");
                return Ok(());
            }
        };

        // Find org by customer ID
        let org_id: Option<(Uuid,)> =
            sqlx::query_as("SELECT id FROM organizations WHERE stripe_customer_id = $1")
                .bind(&customer_id)
                .fetch_optional(&self.pool)
                .await?;

        let org_id = match org_id {
            Some((id,)) => id,
            None => {
                tracing::warn!(customer_id = %customer_id, "Charge failed for unknown customer");
                return Ok(());
            }
        };

        let amount_cents = charge.amount as i32;
        let failure_message = charge
            .failure_message
            .clone()
            .unwrap_or_else(|| "Unknown failure".to_string());
        let failure_code = charge.failure_code.clone().map(|c| c.to_string());

        tracing::error!(
            org_id = %org_id,
            charge_id = %charge.id,
            amount_cents = amount_cents,
            failure_message = %failure_message,
            failure_code = ?failure_code,
            "Payment charge failed"
        );

        // Log billing event
        if let Err(e) = self
            .event_logger
            .log_event(
                BillingEventBuilder::new(org_id, BillingEventType::PaymentFailed)
                    .data(serde_json::json!({
                        "amount_cents": amount_cents,
                        "failure_message": failure_message,
                        "failure_code": failure_code,
                        "charge_id": charge.id.to_string(),
                    }))
                    .stripe_event(&event_id)
                    .actor_type(ActorType::Stripe),
            )
            .await
        {
            tracing::warn!(error = %e, "Failed to log charge failed event");
        }

        // Update instant_charge if this was for an instant charge
        if let Some(invoice_id) = charge.invoice.as_ref().map(|i| match i {
            stripe::Expandable::Id(id) => id.to_string(),
            stripe::Expandable::Object(inv) => inv.id.to_string(),
        }) {
            let instant_charge_service = crate::InstantChargeService::new(
                self.stripe.clone(),
                self.pool.clone(),
                self.email.clone(),
            );
            if let Err(e) = instant_charge_service
                .mark_failed(&invoice_id, &failure_message)
                .await
            {
                tracing::error!(
                    invoice_id = %invoice_id,
                    failure_message = %failure_message,
                    error = %e,
                    "Failed to mark instant charge as failed in database. \
                     Instant charge record may show incorrect status."
                );
            }
        }

        // Send notification email to org owner
        if let Ok(Some((email, org_name))) = self.get_org_owner_email(org_id).await {
            if let Err(e) = self
                .email
                .send_payment_failed(&email, &org_name, amount_cents, &failure_message)
                .await
            {
                tracing::error!(error = %e, "Failed to send payment failed email");
            }
        }

        Ok(())
    }

    /// Handle charge.refunded - charge was refunded (partially or fully)
    async fn handle_charge_refunded(&self, event: Event) -> BillingResult<()> {
        let event_id = event.id.to_string();
        let charge = match event.data.object {
            EventObject::Charge(charge) => charge,
            _ => {
                return Err(BillingError::WebhookEventNotSupported(
                    "Expected Charge".to_string(),
                ))
            }
        };

        let customer_id = match &charge.customer {
            Some(stripe::Expandable::Id(id)) => id.to_string(),
            Some(stripe::Expandable::Object(c)) => c.id.to_string(),
            None => {
                tracing::warn!(charge_id = %charge.id, "Charge refunded without customer ID");
                return Ok(());
            }
        };

        // Find org by customer ID
        let org_id: Option<(Uuid,)> =
            sqlx::query_as("SELECT id FROM organizations WHERE stripe_customer_id = $1")
                .bind(&customer_id)
                .fetch_optional(&self.pool)
                .await?;

        let org_id = match org_id {
            Some((id,)) => id,
            None => {
                tracing::warn!(customer_id = %customer_id, "Charge refunded for unknown customer");
                return Ok(());
            }
        };

        let amount_refunded = charge.amount_refunded as i32;
        let total_amount = charge.amount as i32;
        let is_full_refund = amount_refunded >= total_amount;

        tracing::info!(
            org_id = %org_id,
            charge_id = %charge.id,
            amount_refunded = amount_refunded,
            total_amount = total_amount,
            is_full_refund = is_full_refund,
            "Charge refunded"
        );

        // Log billing event
        if let Err(e) = self
            .event_logger
            .log_event(
                BillingEventBuilder::new(org_id, BillingEventType::RefundIssued)
                    .data(serde_json::json!({
                        "amount_refunded_cents": amount_refunded,
                        "total_amount_cents": total_amount,
                        "is_full_refund": is_full_refund,
                        "charge_id": charge.id.to_string(),
                    }))
                    .stripe_event(&event_id)
                    .actor_type(ActorType::Stripe),
            )
            .await
        {
            tracing::warn!(error = %e, "Failed to log charge refunded event");
        }

        // Record the refund in refund_audit table
        sqlx::query(
            r#"
            INSERT INTO refund_audit (org_id, stripe_charge_id, amount_cents, refund_type, source, created_at)
            VALUES ($1, $2, $3, $4, 'stripe_webhook', NOW())
            ON CONFLICT DO NOTHING
            "#
        )
        .bind(org_id)
        .bind(charge.id.to_string())
        .bind(amount_refunded)
        .bind(if is_full_refund { "full" } else { "partial" })
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Handle charge.dispute.created - customer initiated a chargeback
    /// CRITICAL: Disputes can result in significant financial penalties
    async fn handle_charge_dispute_created(&self, event: Event) -> BillingResult<()> {
        let event_id = event.id.to_string();
        let dispute = match event.data.object {
            EventObject::Dispute(dispute) => dispute,
            _ => {
                return Err(BillingError::WebhookEventNotSupported(
                    "Expected Dispute".to_string(),
                ))
            }
        };

        let charge_id = match &dispute.charge {
            stripe::Expandable::Id(id) => Some(id.to_string()),
            stripe::Expandable::Object(ch) => Some(ch.id.to_string()),
        };

        let amount = dispute.amount as i32;
        let reason = if dispute.reason.is_empty() {
            "unknown".to_string()
        } else {
            dispute.reason.clone()
        };

        // This is a CRITICAL alert - disputes are serious
        tracing::error!(
            dispute_id = %dispute.id,
            charge_id = ?charge_id,
            amount_cents = amount,
            reason = %reason,
            status = ?dispute.status,
            "CRITICAL: Charge dispute (chargeback) created!"
        );

        // Try to find the customer and org
        if let Some(charge_id_str) = &charge_id {
            // Look up the charge to find the customer
            if let Ok(charge) = stripe::Charge::retrieve(
                self.stripe.inner(),
                &charge_id_str.parse().unwrap_or_default(),
                &[],
            )
            .await
            {
                if let Some(customer) = &charge.customer {
                    let customer_id = match customer {
                        stripe::Expandable::Id(id) => id.to_string(),
                        stripe::Expandable::Object(c) => c.id.to_string(),
                    };

                    if let Ok(Some((id,))) = sqlx::query_as::<_, (Uuid,)>(
                        "SELECT id FROM organizations WHERE stripe_customer_id = $1",
                    )
                    .bind(&customer_id)
                    .fetch_optional(&self.pool)
                    .await
                    {
                        // Record the dispute
                        sqlx::query(
                            r#"
                            INSERT INTO billing_disputes (
                                org_id, stripe_dispute_id, stripe_charge_id,
                                amount_cents, reason, status, created_at
                            )
                            VALUES ($1, $2, $3, $4, $5, $6, NOW())
                            ON CONFLICT (stripe_dispute_id) DO UPDATE SET
                                status = EXCLUDED.status,
                                updated_at = NOW()
                            "#,
                        )
                        .bind(id)
                        .bind(dispute.id.to_string())
                        .bind(charge_id_str)
                        .bind(amount)
                        .bind(&reason)
                        .bind(format!("{:?}", dispute.status))
                        .execute(&self.pool)
                        .await
                        .ok(); // Best effort - don't fail webhook for audit log failure

                        // Notify org owner about the dispute
                        if let Ok(Some((email, org_name))) = self.get_org_owner_email(id).await {
                            if let Err(e) = self
                                .email
                                .send_dispute_alert(&email, &org_name, amount, &reason)
                                .await
                            {
                                tracing::error!(error = %e, "Failed to send dispute alert email");
                            }
                        }

                        // Log billing event
                        if let Err(e) = self
                            .event_logger
                            .log_event(
                                BillingEventBuilder::new(id, BillingEventType::DisputeCreated)
                                    .data(serde_json::json!({
                                        "amount_cents": amount,
                                        "reason": reason,
                                        "status": format!("{:?}", dispute.status),
                                        "dispute_id": dispute.id.to_string(),
                                        "charge_id": charge_id_str,
                                    }))
                                    .stripe_event(&event_id)
                                    .actor_type(ActorType::Stripe),
                            )
                            .await
                        {
                            tracing::warn!(error = %e, "Failed to log dispute created event");
                        }

                        tracing::warn!(
                            org_id = %id,
                            dispute_id = %dispute.id,
                            "Dispute recorded for organization"
                        );
                    }
                }
            }
        }

        // TODO: Consider implementing automatic response or admin notification system

        Ok(())
    }

    /// Handle payment_intent.payment_failed - payment intent level failure
    async fn handle_payment_intent_failed(&self, event: Event) -> BillingResult<()> {
        let event_id = event.id.to_string();
        let payment_intent = match event.data.object {
            EventObject::PaymentIntent(pi) => pi,
            _ => {
                return Err(BillingError::WebhookEventNotSupported(
                    "Expected PaymentIntent".to_string(),
                ))
            }
        };

        let customer_id = match &payment_intent.customer {
            Some(stripe::Expandable::Id(id)) => id.to_string(),
            Some(stripe::Expandable::Object(c)) => c.id.to_string(),
            None => {
                tracing::warn!(payment_intent_id = %payment_intent.id, "PaymentIntent failed without customer");
                return Ok(());
            }
        };

        // Find org by customer ID
        let org_id: Option<(Uuid,)> =
            sqlx::query_as("SELECT id FROM organizations WHERE stripe_customer_id = $1")
                .bind(&customer_id)
                .fetch_optional(&self.pool)
                .await?;

        let org_id = match org_id {
            Some((id,)) => id,
            None => {
                tracing::warn!(customer_id = %customer_id, "PaymentIntent failed for unknown customer");
                return Ok(());
            }
        };

        let amount = payment_intent.amount as i32;
        let error_message = payment_intent
            .last_payment_error
            .as_ref()
            .and_then(|e| e.message.clone())
            .unwrap_or_else(|| "Payment failed".to_string());
        let error_code = payment_intent
            .last_payment_error
            .as_ref()
            .and_then(|e| e.code)
            .map(|c| c.to_string());

        tracing::error!(
            org_id = %org_id,
            payment_intent_id = %payment_intent.id,
            amount_cents = amount,
            error_message = %error_message,
            error_code = ?error_code,
            "Payment intent failed"
        );

        // Log billing event
        if let Err(e) = self
            .event_logger
            .log_event(
                BillingEventBuilder::new(org_id, BillingEventType::PaymentFailed)
                    .data(serde_json::json!({
                        "amount_cents": amount,
                        "error_message": error_message,
                        "error_code": error_code,
                        "payment_intent_id": payment_intent.id.to_string(),
                    }))
                    .stripe_event(&event_id)
                    .actor_type(ActorType::Stripe),
            )
            .await
        {
            tracing::warn!(error = %e, "Failed to log payment intent failed event");
        }

        // Send notification to org owner
        if let Ok(Some((email, org_name))) = self.get_org_owner_email(org_id).await {
            if let Err(e) = self
                .email
                .send_payment_failed(&email, &org_name, amount, &error_message)
                .await
            {
                tracing::error!(error = %e, "Failed to send payment intent failed email");
            }
        }

        Ok(())
    }

    // ============ CUSTOMER EVENT HANDLERS ============

    async fn handle_customer_created(&self, event: Event) -> BillingResult<()> {
        let event_id = event.id.to_string();
        let customer = self.extract_customer(event)?;

        // Try to link customer to org via metadata
        let org_id = customer
            .metadata
            .as_ref()
            .and_then(|m| m.get("org_id"))
            .and_then(|id| Uuid::parse_str(id).ok());

        if let Some(org_id) = org_id {
            // Update organization with Stripe customer ID
            let result = sqlx::query(
                "UPDATE organizations SET stripe_customer_id = $1, updated_at = NOW() WHERE id = $2 AND stripe_customer_id IS NULL"
            )
            .bind(customer.id.to_string())
            .bind(org_id)
            .execute(&self.pool)
            .await;

            if let Err(e) = result {
                tracing::warn!(
                    org_id = %org_id,
                    customer_id = %customer.id,
                    error = %e,
                    "Failed to link Stripe customer to organization"
                );
            }

            // Log billing event
            if let Err(e) = self
                .event_logger
                .log_event(
                    BillingEventBuilder::new(org_id, BillingEventType::CustomerCreated)
                        .data(serde_json::json!({
                            "customer_id": customer.id.to_string(),
                            "email": customer.email,
                        }))
                        .stripe_event(&event_id)
                        .actor_type(ActorType::Stripe),
                )
                .await
            {
                tracing::warn!(error = %e, "Failed to log customer created event");
            }

            tracing::info!(
                org_id = %org_id,
                customer_id = %customer.id,
                "Linked Stripe customer to organization"
            );
        } else {
            tracing::info!(
                customer_id = %customer.id,
                email = ?customer.email,
                "Customer created without org_id metadata"
            );
        }

        Ok(())
    }

    async fn handle_customer_updated(&self, event: Event) -> BillingResult<()> {
        let event_id = event.id.to_string();
        let customer = self.extract_customer(event)?;

        // Find org by customer ID
        let org_id: Option<(Uuid,)> =
            sqlx::query_as("SELECT id FROM organizations WHERE stripe_customer_id = $1")
                .bind(customer.id.to_string())
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| BillingError::Database(e.to_string()))?;

        if let Some((org_id,)) = org_id {
            // Log billing event for customer updates
            if let Err(e) = self
                .event_logger
                .log_event(
                    BillingEventBuilder::new(org_id, BillingEventType::CustomerUpdated)
                        .data(serde_json::json!({
                            "customer_id": customer.id.to_string(),
                            "email": customer.email,
                            "delinquent": customer.delinquent,
                        }))
                        .stripe_event(&event_id)
                        .actor_type(ActorType::Stripe),
                )
                .await
            {
                tracing::warn!(error = %e, "Failed to log customer updated event");
            }

            // Check if customer is marked delinquent
            if customer.delinquent.unwrap_or(false) {
                tracing::warn!(
                    org_id = %org_id,
                    customer_id = %customer.id,
                    "Customer marked as delinquent - payment issues detected"
                );
            }

            tracing::info!(
                org_id = %org_id,
                customer_id = %customer.id,
                "Customer updated"
            );
        }

        Ok(())
    }

    async fn handle_customer_deleted(&self, event: Event) -> BillingResult<()> {
        let event_id = event.id.to_string();
        let customer = self.extract_customer(event)?;

        // Find org by customer ID
        let org_id: Option<(Uuid,)> =
            sqlx::query_as("SELECT id FROM organizations WHERE stripe_customer_id = $1")
                .bind(customer.id.to_string())
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| BillingError::Database(e.to_string()))?;

        if let Some((org_id,)) = org_id {
            // Clear the Stripe customer ID from org (customer was deleted in Stripe)
            sqlx::query(
                "UPDATE organizations SET stripe_customer_id = NULL, updated_at = NOW() WHERE id = $1"
            )
            .bind(org_id)
            .execute(&self.pool)
            .await
            .map_err(|e| BillingError::Database(e.to_string()))?;

            // Log billing event
            if let Err(e) = self
                .event_logger
                .log_event(
                    BillingEventBuilder::new(org_id, BillingEventType::CustomerDeleted)
                        .data(serde_json::json!({
                            "customer_id": customer.id.to_string(),
                        }))
                        .stripe_event(&event_id)
                        .actor_type(ActorType::Stripe),
                )
                .await
            {
                tracing::warn!(error = %e, "Failed to log customer deleted event");
            }

            tracing::warn!(
                org_id = %org_id,
                customer_id = %customer.id,
                "Stripe customer deleted - cleared customer ID from organization"
            );
        }

        Ok(())
    }

    fn extract_customer(&self, event: Event) -> BillingResult<stripe::Customer> {
        match event.data.object {
            EventObject::Customer(customer) => Ok(customer),
            _ => Err(BillingError::WebhookEventNotSupported(
                "Expected Customer".to_string(),
            )),
        }
    }

    // ============ WEBHOOK REPLAY FUNCTIONALITY ============

    /// List failed webhook events that can be replayed
    pub async fn list_failed_webhooks(
        &self,
        limit: i64,
        offset: i64,
    ) -> BillingResult<Vec<WebhookEventRecord>> {
        let records: Vec<WebhookEventRecord> = sqlx::query_as(
            r#"
            SELECT id, stripe_event_id, event_type, event_timestamp,
                   processing_result, processing_started_at, error_message,
                   created_at
            FROM stripe_webhook_events
            WHERE processing_result IN ('error', 'processing')
            ORDER BY created_at DESC
            LIMIT $1 OFFSET $2
            "#,
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| BillingError::Database(e.to_string()))?;

        Ok(records)
    }

    /// List all webhook events with optional status filter
    pub async fn list_webhooks(
        &self,
        status_filter: Option<&str>,
        limit: i64,
        offset: i64,
    ) -> BillingResult<Vec<WebhookEventRecord>> {
        let records: Vec<WebhookEventRecord> = match status_filter {
            Some(status) => sqlx::query_as(
                r#"
                    SELECT id, stripe_event_id, event_type, event_timestamp,
                           processing_result, processing_started_at, error_message,
                           created_at
                    FROM stripe_webhook_events
                    WHERE processing_result = $1
                    ORDER BY created_at DESC
                    LIMIT $2 OFFSET $3
                    "#,
            )
            .bind(status)
            .bind(limit)
            .bind(offset)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| BillingError::Database(e.to_string()))?,
            None => sqlx::query_as(
                r#"
                    SELECT id, stripe_event_id, event_type, event_timestamp,
                           processing_result, processing_started_at, error_message,
                           created_at
                    FROM stripe_webhook_events
                    ORDER BY created_at DESC
                    LIMIT $1 OFFSET $2
                    "#,
            )
            .bind(limit)
            .bind(offset)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| BillingError::Database(e.to_string()))?,
        };

        Ok(records)
    }

    /// Replay a webhook event by fetching it from Stripe and re-processing
    ///
    /// This is useful for:
    /// - Recovering from transient errors
    /// - Testing after fixing bugs
    /// - Manual intervention after failed processing
    pub async fn replay_webhook(
        &self,
        stripe_event_id: &str,
    ) -> BillingResult<WebhookReplayResult> {
        tracing::info!(
            stripe_event_id = %stripe_event_id,
            "Attempting to replay webhook event"
        );

        // First check if we have a record of this event
        let existing: Option<(Uuid, String, Option<String>)> = sqlx::query_as(
            r#"
            SELECT id, processing_result, error_message
            FROM stripe_webhook_events
            WHERE stripe_event_id = $1
            "#,
        )
        .bind(stripe_event_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| BillingError::Database(e.to_string()))?;

        let (record_id, previous_status, previous_error) = existing.ok_or_else(|| {
            BillingError::NotFound(format!(
                "Webhook event {} not found in database",
                stripe_event_id
            ))
        })?;

        // Fetch the event from Stripe
        let event_id = stripe_event_id
            .parse::<stripe::EventId>()
            .map_err(|e| BillingError::InvalidInput(format!("Invalid event ID: {}", e)))?;

        let event = stripe::Event::retrieve(self.stripe.inner(), &event_id, &[])
            .await
            .map_err(|e| {
                BillingError::StripeApi(format!("Failed to fetch event from Stripe: {}", e))
            })?;

        // Mark as replaying
        sqlx::query(
            r#"
            UPDATE stripe_webhook_events
            SET processing_result = 'replaying',
                processing_started_at = NOW(),
                error_message = CONCAT('Replay initiated. Previous status: ', $2, '. Previous error: ', COALESCE($3, 'none'))
            WHERE stripe_event_id = $1
            "#
        )
        .bind(stripe_event_id)
        .bind(&previous_status)
        .bind(&previous_error)
        .execute(&self.pool)
        .await
        .map_err(|e| BillingError::Database(e.to_string()))?;

        // Process the event
        let process_result = self.process_event_internal(&event).await;

        // Update with result
        let (new_status, new_error) = match &process_result {
            Ok(()) => ("success".to_string(), None),
            Err(e) => ("error".to_string(), Some(e.to_string())),
        };

        sqlx::query(
            r#"
            UPDATE stripe_webhook_events
            SET processing_result = $1,
                error_message = $2
            WHERE stripe_event_id = $3
            "#,
        )
        .bind(&new_status)
        .bind(&new_error)
        .bind(stripe_event_id)
        .execute(&self.pool)
        .await
        .map_err(|e| BillingError::Database(e.to_string()))?;

        tracing::info!(
            stripe_event_id = %stripe_event_id,
            previous_status = %previous_status,
            new_status = %new_status,
            success = process_result.is_ok(),
            "Webhook replay completed"
        );

        Ok(WebhookReplayResult {
            record_id,
            stripe_event_id: stripe_event_id.to_string(),
            event_type: format!("{:?}", event.type_),
            previous_status,
            previous_error,
            new_status,
            new_error,
            success: process_result.is_ok(),
        })
    }

    /// Replay all failed webhooks (with optional limit)
    pub async fn replay_all_failed(
        &self,
        max_events: Option<i64>,
    ) -> BillingResult<Vec<WebhookReplayResult>> {
        let limit = max_events.unwrap_or(100);

        let failed_events: Vec<(String,)> = sqlx::query_as(
            r#"
            SELECT stripe_event_id
            FROM stripe_webhook_events
            WHERE processing_result = 'error'
            ORDER BY created_at ASC
            LIMIT $1
            "#,
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| BillingError::Database(e.to_string()))?;

        let mut results = Vec::with_capacity(failed_events.len());

        for (event_id,) in failed_events {
            match self.replay_webhook(&event_id).await {
                Ok(result) => results.push(result),
                Err(e) => {
                    tracing::error!(
                        stripe_event_id = %event_id,
                        error = %e,
                        "Failed to replay webhook"
                    );
                    results.push(WebhookReplayResult {
                        record_id: Uuid::nil(),
                        stripe_event_id: event_id,
                        event_type: "unknown".to_string(),
                        previous_status: "error".to_string(),
                        previous_error: None,
                        new_status: "error".to_string(),
                        new_error: Some(e.to_string()),
                        success: false,
                    });
                }
            }
        }

        Ok(results)
    }

    /// Reset a stuck webhook to allow re-processing
    pub async fn reset_stuck_webhook(&self, stripe_event_id: &str) -> BillingResult<()> {
        let result = sqlx::query(
            r#"
            UPDATE stripe_webhook_events
            SET processing_result = 'pending_replay',
                error_message = CONCAT('Reset for replay at ', NOW()::TEXT)
            WHERE stripe_event_id = $1
              AND processing_result IN ('processing', 'error')
            "#,
        )
        .bind(stripe_event_id)
        .execute(&self.pool)
        .await
        .map_err(|e| BillingError::Database(e.to_string()))?;

        if result.rows_affected() == 0 {
            return Err(BillingError::NotFound(format!(
                "Webhook {} not found or not in resettable state",
                stripe_event_id
            )));
        }

        tracing::info!(
            stripe_event_id = %stripe_event_id,
            "Webhook reset for replay"
        );

        Ok(())
    }
}

/// Stored webhook event record
#[derive(Debug, Clone, serde::Serialize, sqlx::FromRow)]
pub struct WebhookEventRecord {
    pub id: Uuid,
    pub stripe_event_id: String,
    pub event_type: String,
    pub event_timestamp: OffsetDateTime,
    pub processing_result: String,
    pub processing_started_at: Option<OffsetDateTime>,
    pub error_message: Option<String>,
    pub created_at: OffsetDateTime,
}

/// Result of a webhook replay operation
#[derive(Debug, Clone, serde::Serialize)]
pub struct WebhookReplayResult {
    pub record_id: Uuid,
    pub stripe_event_id: String,
    pub event_type: String,
    pub previous_status: String,
    pub previous_error: Option<String>,
    pub new_status: String,
    pub new_error: Option<String>,
    pub success: bool,
}
