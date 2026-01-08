//! Refund service for admin-initiated refunds
//!
//! Handles issuing actual refunds to payment methods (vs credits which use prorations).
//! Used for immediate downgrades when admin selects "refund" instead of "credit".

use serde::Serialize;
use sqlx::PgPool;
use stripe::{CreateRefund, Invoice, Refund, RefundReasonFilter};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::client::StripeClient;
use crate::error::{BillingError, BillingResult};

/// Result of a refund operation
#[derive(Debug, Clone, Serialize)]
pub struct RefundResult {
    /// Stripe refund ID
    pub stripe_refund_id: String,
    /// Stripe charge ID that was refunded
    pub stripe_charge_id: String,
    /// Amount refunded in cents
    pub amount_cents: i64,
    /// Refund type: "refund" (actual money back) or "credit" (Stripe account credit)
    pub refund_type: String,
}

/// Information about a refundable charge
#[derive(Debug, Clone)]
pub struct RefundableCharge {
    /// Stripe charge ID
    pub charge_id: String,
    /// Stripe invoice ID
    pub invoice_id: String,
    /// Amount paid in cents
    pub amount_cents: i64,
    /// When the billing period started
    pub period_start: OffsetDateTime,
    /// When the billing period ends
    pub period_end: OffsetDateTime,
    /// When the charge was created
    pub created_at: OffsetDateTime,
}

/// Admin refund record for audit trail
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct AdminRefund {
    pub id: Uuid,
    pub org_id: Uuid,
    pub admin_user_id: Uuid,
    pub stripe_refund_id: Option<String>,
    pub stripe_charge_id: Option<String>,
    pub stripe_invoice_id: Option<String>,
    pub amount_cents: i32,
    pub refund_type: String,
    pub reason: String,
    pub old_tier: String,
    pub new_tier: String,
    pub status: String,
    pub error_message: Option<String>,
    pub created_at: OffsetDateTime,
    pub completed_at: Option<OffsetDateTime>,
}

/// Refund service for handling Stripe refunds
pub struct RefundService {
    stripe: StripeClient,
    pool: PgPool,
}

impl RefundService {
    pub fn new(stripe: StripeClient, pool: PgPool) -> Self {
        Self { stripe, pool }
    }

    /// Issue a refund to the customer's payment method
    ///
    /// This creates an actual Stripe refund (money back to card/bank),
    /// not a credit on the Stripe account.
    pub async fn issue_refund(
        &self,
        org_id: Uuid,
        admin_user_id: Uuid,
        charge_id: &str,
        invoice_id: &str,
        amount_cents: i64,
        reason: &str,
        old_tier: &str,
        new_tier: &str,
    ) -> BillingResult<RefundResult> {
        // Create audit record first (pending status)
        let refund_record_id = self
            .create_refund_record(
                org_id,
                admin_user_id,
                Some(charge_id),
                Some(invoice_id),
                amount_cents,
                "refund",
                reason,
                old_tier,
                new_tier,
            )
            .await?;

        // Create the Stripe refund
        let mut params = CreateRefund::new();
        params.charge = Some(
            charge_id
                .parse()
                .map_err(|e| BillingError::RefundFailed(format!("Invalid charge ID: {}", e)))?,
        );
        params.amount = Some(amount_cents);
        params.reason = Some(RefundReasonFilter::RequestedByCustomer);

        // Add metadata for audit purposes
        let mut metadata = std::collections::HashMap::new();
        metadata.insert("admin_refund".to_string(), "true".to_string());
        metadata.insert("admin_user_id".to_string(), admin_user_id.to_string());
        metadata.insert("org_id".to_string(), org_id.to_string());
        metadata.insert("old_tier".to_string(), old_tier.to_string());
        metadata.insert("new_tier".to_string(), new_tier.to_string());
        metadata.insert("reason".to_string(), reason.to_string());
        params.metadata = Some(metadata);

        match Refund::create(self.stripe.inner(), params).await {
            Ok(refund) => {
                // Update audit record with success
                self.complete_refund_record(
                    refund_record_id,
                    Some(refund.id.as_str()),
                    "completed",
                    None,
                )
                .await?;

                tracing::info!(
                    org_id = %org_id,
                    admin_user_id = %admin_user_id,
                    refund_id = %refund.id,
                    charge_id = %charge_id,
                    amount_cents = %amount_cents,
                    "Issued refund"
                );

                Ok(RefundResult {
                    stripe_refund_id: refund.id.to_string(),
                    stripe_charge_id: charge_id.to_string(),
                    amount_cents,
                    refund_type: "refund".to_string(),
                })
            }
            Err(e) => {
                // Update audit record with failure
                let error_msg = e.to_string();
                self.complete_refund_record(refund_record_id, None, "failed", Some(&error_msg))
                    .await?;

                tracing::error!(
                    org_id = %org_id,
                    charge_id = %charge_id,
                    error = %error_msg,
                    "Failed to issue refund"
                );

                Err(BillingError::RefundFailed(error_msg))
            }
        }
    }

    /// Record a credit operation (for audit trail when using prorations instead of refund)
    pub async fn record_credit(
        &self,
        org_id: Uuid,
        admin_user_id: Uuid,
        amount_cents: i64,
        reason: &str,
        old_tier: &str,
        new_tier: &str,
    ) -> BillingResult<Uuid> {
        let record_id = self
            .create_refund_record(
                org_id,
                admin_user_id,
                None,
                None,
                amount_cents,
                "credit",
                reason,
                old_tier,
                new_tier,
            )
            .await?;

        // Credits are completed immediately (handled by Stripe proration)
        self.complete_refund_record(record_id, None, "completed", None)
            .await?;

        tracing::info!(
            org_id = %org_id,
            admin_user_id = %admin_user_id,
            amount_cents = %amount_cents,
            "Recorded credit for tier change"
        );

        Ok(record_id)
    }

    /// Get the latest refundable charge for a subscription
    ///
    /// Stripe allows refunds within ~90 days of charge. This finds the most recent
    /// paid invoice and returns its charge information.
    pub async fn get_refundable_charge(
        &self,
        subscription_id: &str,
    ) -> BillingResult<RefundableCharge> {
        // List invoices for this subscription, sorted by date descending
        let mut params = stripe::ListInvoices::new();
        params.subscription =
            Some(subscription_id.parse().map_err(|e| {
                BillingError::RefundFailed(format!("Invalid subscription ID: {}", e))
            })?);
        params.status = Some(stripe::InvoiceStatus::Paid);
        params.limit = Some(1);

        let invoices = Invoice::list(self.stripe.inner(), &params).await?;

        let invoice = invoices
            .data
            .into_iter()
            .next()
            .ok_or(BillingError::NoRefundableCharge)?;

        // Get the charge ID from the invoice
        let charge_id = invoice
            .charge
            .as_ref()
            .map(|c| match c {
                stripe::Expandable::Id(id) => id.to_string(),
                stripe::Expandable::Object(charge) => charge.id.to_string(),
            })
            .ok_or(BillingError::NoRefundableCharge)?;

        // Check if charge is too old (Stripe 90-day limit)
        let charge_created = invoice.created.unwrap_or(0);
        let charge_time = OffsetDateTime::from_unix_timestamp(charge_created)
            .map_err(|_| BillingError::RefundFailed("Invalid charge timestamp".to_string()))?;

        let now = OffsetDateTime::now_utc();
        let days_old = (now - charge_time).whole_days();

        if days_old > 90 {
            return Err(BillingError::ChargeExpiredForRefund);
        }

        // Get period information from the invoice lines
        let period_start = invoice
            .period_start
            .map(|ts| {
                OffsetDateTime::from_unix_timestamp(ts).unwrap_or_else(|e| {
                    tracing::warn!(
                        invoice_id = %invoice.id,
                        timestamp = ts,
                        error = %e,
                        "Failed to parse period_start timestamp, using now"
                    );
                    OffsetDateTime::now_utc()
                })
            })
            .unwrap_or_else(|| {
                tracing::warn!(invoice_id = %invoice.id, "Missing period_start, using now");
                OffsetDateTime::now_utc()
            });

        let period_end = invoice
            .period_end
            .map(|ts| {
                OffsetDateTime::from_unix_timestamp(ts).unwrap_or_else(|e| {
                    tracing::warn!(
                        invoice_id = %invoice.id,
                        timestamp = ts,
                        error = %e,
                        "Failed to parse period_end timestamp, using now"
                    );
                    OffsetDateTime::now_utc()
                })
            })
            .unwrap_or_else(|| {
                tracing::warn!(invoice_id = %invoice.id, "Missing period_end, using now");
                OffsetDateTime::now_utc()
            });

        let amount_cents = invoice.amount_paid.unwrap_or(0);

        Ok(RefundableCharge {
            charge_id,
            invoice_id: invoice.id.to_string(),
            amount_cents,
            period_start,
            period_end,
            created_at: charge_time,
        })
    }

    /// Calculate prorated refund amount based on remaining days in billing period
    ///
    /// Always floors the result to avoid over-refunding.
    pub fn calculate_prorated_amount(
        invoice_amount_cents: i64,
        period_start: OffsetDateTime,
        period_end: OffsetDateTime,
    ) -> i64 {
        let now = OffsetDateTime::now_utc();

        // If period has already ended, no refund
        if now >= period_end {
            return 0;
        }

        let total_days = (period_end - period_start).whole_days() as f64;
        if total_days <= 0.0 {
            return 0;
        }

        let remaining_days = (period_end - now).whole_days().max(0) as f64;
        let prorated = (invoice_amount_cents as f64) * (remaining_days / total_days);

        prorated.floor() as i64
    }

    /// Create an audit record for a refund/credit operation
    async fn create_refund_record(
        &self,
        org_id: Uuid,
        admin_user_id: Uuid,
        charge_id: Option<&str>,
        invoice_id: Option<&str>,
        amount_cents: i64,
        refund_type: &str,
        reason: &str,
        old_tier: &str,
        new_tier: &str,
    ) -> BillingResult<Uuid> {
        let record: (Uuid,) = sqlx::query_as(
            r#"
            INSERT INTO admin_refunds (
                org_id,
                admin_user_id,
                stripe_charge_id,
                stripe_invoice_id,
                amount_cents,
                refund_type,
                reason,
                old_tier,
                new_tier,
                status
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'pending')
            RETURNING id
            "#,
        )
        .bind(org_id)
        .bind(admin_user_id)
        .bind(charge_id)
        .bind(invoice_id)
        .bind(amount_cents as i32)
        .bind(refund_type)
        .bind(reason)
        .bind(old_tier)
        .bind(new_tier)
        .fetch_one(&self.pool)
        .await?;

        Ok(record.0)
    }

    /// Complete a refund record (success or failure)
    async fn complete_refund_record(
        &self,
        record_id: Uuid,
        stripe_refund_id: Option<&str>,
        status: &str,
        error_message: Option<&str>,
    ) -> BillingResult<()> {
        sqlx::query(
            r#"
            UPDATE admin_refunds
            SET stripe_refund_id = $2,
                status = $3,
                error_message = $4,
                completed_at = NOW()
            WHERE id = $1
            "#,
        )
        .bind(record_id)
        .bind(stripe_refund_id)
        .bind(status)
        .bind(error_message)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Get refund history for an organization
    pub async fn get_refund_history(&self, org_id: Uuid) -> BillingResult<Vec<AdminRefund>> {
        let records = sqlx::query_as::<_, AdminRefund>(
            r#"
            SELECT
                id,
                org_id,
                admin_user_id,
                stripe_refund_id,
                stripe_charge_id,
                stripe_invoice_id,
                amount_cents,
                refund_type,
                reason,
                old_tier,
                new_tier,
                status,
                error_message,
                created_at,
                completed_at
            FROM admin_refunds
            WHERE org_id = $1
            ORDER BY created_at DESC
            "#,
        )
        .bind(org_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(records)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_prorated_amount() {
        // 30-day month, 10+ days remaining = ~1/3 refund
        // Use 11 days to account for timing variance (whole_days() truncation)
        let start = OffsetDateTime::now_utc() - time::Duration::days(20);
        let end = OffsetDateTime::now_utc() + time::Duration::days(11);

        let refund = RefundService::calculate_prorated_amount(9000, start, end);
        // With 31-day total and 10-11 days remaining: 9000 * (10/31) to (11/31) ≈ 2903-3193
        // Should be close to 3000 (within timing variance)
        assert!(
            (2900..=3300).contains(&refund),
            "Expected refund between 2900-3300, got {}",
            refund
        );
    }

    #[test]
    fn test_calculate_prorated_amount_zero_remaining() {
        // Period already ended
        let start = OffsetDateTime::now_utc() - time::Duration::days(30);
        let end = OffsetDateTime::now_utc() - time::Duration::days(1);

        let refund = RefundService::calculate_prorated_amount(9000, start, end);
        assert_eq!(refund, 0);
    }

    #[test]
    fn test_calculate_prorated_amount_full_period() {
        // Full period remaining (just started)
        // Use -1 second from now to ensure now() inside function is still within the period
        let start = OffsetDateTime::now_utc() - time::Duration::seconds(1);
        let end = start + time::Duration::days(30);

        let refund = RefundService::calculate_prorated_amount(9000, start, end);
        // Should be close to full amount (29-30/30 days)
        // With the -1 second buffer, remaining should be at least 29 days: 9000 * 29/30 = 8700
        assert!(refund >= 8700, "Expected refund >= 8700, got {}", refund);
    }
}
