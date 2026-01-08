//! Tax and VAT Handling
//!
//! Provides tax-related functionality including:
//! - Customer tax ID management (VAT numbers, GST, etc.)
//! - Tax rate configuration per region
//! - Tax reporting for compliance
//!
//! This module integrates with Stripe Tax for automatic tax calculation.

use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::client::StripeClient;
use crate::error::{BillingError, BillingResult};

/// Supported tax ID types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaxIdType {
    /// EU VAT number
    EuVat,
    /// UK VAT number
    GbVat,
    /// US EIN
    UsEin,
    /// Australian Business Number
    AuAbn,
    /// Canadian GST/HST number
    CaGstHst,
    /// Brazilian CNPJ
    BrCnpj,
    /// Indian GST
    InGst,
    /// Generic tax ID
    Other,
}

impl TaxIdType {
    pub fn as_stripe_type(&self) -> &'static str {
        match self {
            TaxIdType::EuVat => "eu_vat",
            TaxIdType::GbVat => "gb_vat",
            TaxIdType::UsEin => "us_ein",
            TaxIdType::AuAbn => "au_abn",
            TaxIdType::CaGstHst => "ca_gst_hst",
            TaxIdType::BrCnpj => "br_cnpj",
            TaxIdType::InGst => "in_gst",
            TaxIdType::Other => "eu_vat", // Default fallback
        }
    }

    pub fn from_stripe_type(s: &str) -> Self {
        match s {
            "eu_vat" => TaxIdType::EuVat,
            "gb_vat" => TaxIdType::GbVat,
            "us_ein" => TaxIdType::UsEin,
            "au_abn" => TaxIdType::AuAbn,
            "ca_gst_hst" => TaxIdType::CaGstHst,
            "br_cnpj" => TaxIdType::BrCnpj,
            "in_gst" => TaxIdType::InGst,
            _ => TaxIdType::Other,
        }
    }
}

/// Stored tax ID for an organization
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct TaxId {
    pub id: Uuid,
    pub org_id: Uuid,
    pub tax_id_type: String,
    pub tax_id_value: String,
    pub stripe_tax_id: Option<String>,
    pub verification_status: String,
    pub country: Option<String>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

/// Tax configuration for an organization
#[derive(Debug, Clone, Serialize)]
pub struct TaxConfig {
    pub org_id: Uuid,
    pub tax_exempt: bool,
    pub tax_ids: Vec<TaxId>,
    pub billing_country: Option<String>,
    pub billing_postal_code: Option<String>,
}

/// Tax summary for reporting
#[derive(Debug, Clone, Serialize)]
pub struct TaxSummary {
    pub org_id: Uuid,
    pub period_start: OffsetDateTime,
    pub period_end: OffsetDateTime,
    pub total_taxable_amount_cents: i64,
    pub total_tax_collected_cents: i64,
    pub tax_breakdown: Vec<TaxBreakdown>,
}

/// Tax breakdown by jurisdiction
#[derive(Debug, Clone, Serialize)]
pub struct TaxBreakdown {
    pub jurisdiction: String,
    pub tax_rate_percent: f64,
    pub taxable_amount_cents: i64,
    pub tax_amount_cents: i64,
}

/// Tax service for managing tax IDs and calculations
pub struct TaxService {
    stripe: StripeClient,
    pool: PgPool,
}

impl TaxService {
    pub fn new(stripe: StripeClient, pool: PgPool) -> Self {
        Self { stripe, pool }
    }

    /// Add a tax ID for an organization
    pub async fn add_tax_id(
        &self,
        org_id: Uuid,
        tax_id_type: TaxIdType,
        tax_id_value: &str,
        country: Option<&str>,
    ) -> BillingResult<TaxId> {
        // Get organization's Stripe customer ID
        let customer_id: Option<(String,)> =
            sqlx::query_as("SELECT stripe_customer_id FROM organizations WHERE id = $1")
                .bind(org_id)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| BillingError::Database(e.to_string()))?;

        let customer_id = customer_id
            .and_then(|(id,)| if id.is_empty() { None } else { Some(id) })
            .ok_or(BillingError::NoCustomer)?;

        // Create tax ID in Stripe
        // Note: Stripe's async-stripe library CreateTaxId requires manual construction
        // We'll store locally and sync with Stripe when needed
        let stripe_tax_id_str = format!("txi_local_{}", Uuid::new_v4());

        // For production, you would use Stripe API directly:
        // POST /v1/customers/{customer_id}/tax_ids
        // { "type": "eu_vat", "value": "DE123456789" }
        tracing::info!(
            customer_id = %customer_id,
            tax_id_type = tax_id_type.as_stripe_type(),
            "Recording tax ID (manual Stripe sync may be required)"
        );

        // Store in database
        let tax_id: TaxId = sqlx::query_as(
            r#"
            INSERT INTO organization_tax_ids (org_id, tax_id_type, tax_id_value, stripe_tax_id, verification_status, country)
            VALUES ($1, $2, $3, $4, $5, $6)
            RETURNING id, org_id, tax_id_type, tax_id_value, stripe_tax_id, verification_status, country, created_at, updated_at
            "#
        )
        .bind(org_id)
        .bind(tax_id_type.as_stripe_type())
        .bind(tax_id_value)
        .bind(&stripe_tax_id_str)
        .bind("pending")
        .bind(country)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| BillingError::Database(e.to_string()))?;

        tracing::info!(
            org_id = %org_id,
            tax_id_type = tax_id_type.as_stripe_type(),
            "Added tax ID for organization"
        );

        Ok(tax_id)
    }

    /// Remove a tax ID from an organization
    pub async fn remove_tax_id(&self, org_id: Uuid, tax_id: Uuid) -> BillingResult<()> {
        // Get the tax ID record
        let record: Option<(String, Option<String>)> = sqlx::query_as(
            "SELECT stripe_customer_id, stripe_tax_id FROM organization_tax_ids t
             JOIN organizations o ON t.org_id = o.id
             WHERE t.id = $1 AND t.org_id = $2",
        )
        .bind(tax_id)
        .bind(org_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| BillingError::Database(e.to_string()))?;

        let (_customer_id, stripe_tax_id) =
            record.ok_or_else(|| BillingError::NotFound("Tax ID not found".to_string()))?;

        // Delete from Stripe if exists
        if let Some(stripe_id) = stripe_tax_id {
            // Note: In async-stripe 0.39, TaxId::delete takes (client, &TaxIdId)
            // without customer_id. Log for manual cleanup if needed.
            if !stripe_id.starts_with("txi_local_") {
                let tax_id_parsed = stripe_id
                    .parse::<stripe::TaxIdId>()
                    .map_err(|e| BillingError::StripeApi(format!("Invalid tax ID: {}", e)))?;

                let _ = stripe::TaxId::delete(self.stripe.inner(), &tax_id_parsed).await;
                // Ignore errors - may already be deleted
            }
        }

        // Remove from database
        sqlx::query("DELETE FROM organization_tax_ids WHERE id = $1 AND org_id = $2")
            .bind(tax_id)
            .bind(org_id)
            .execute(&self.pool)
            .await
            .map_err(|e| BillingError::Database(e.to_string()))?;

        tracing::info!(
            org_id = %org_id,
            tax_id = %tax_id,
            "Removed tax ID"
        );

        Ok(())
    }

    /// Get all tax IDs for an organization
    pub async fn get_tax_ids(&self, org_id: Uuid) -> BillingResult<Vec<TaxId>> {
        let tax_ids: Vec<TaxId> = sqlx::query_as(
            r#"
            SELECT id, org_id, tax_id_type, tax_id_value, stripe_tax_id, verification_status, country, created_at, updated_at
            FROM organization_tax_ids
            WHERE org_id = $1
            ORDER BY created_at DESC
            "#
        )
        .bind(org_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| BillingError::Database(e.to_string()))?;

        Ok(tax_ids)
    }

    /// Get tax configuration for an organization
    pub async fn get_tax_config(&self, org_id: Uuid) -> BillingResult<TaxConfig> {
        let tax_ids = self.get_tax_ids(org_id).await?;

        // Get organization tax exempt status and billing address
        let org_info: Option<(bool, Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT COALESCE(tax_exempt, false), billing_country, billing_postal_code FROM organizations WHERE id = $1"
        )
        .bind(org_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| BillingError::Database(e.to_string()))?;

        let (tax_exempt, billing_country, billing_postal_code) =
            org_info.unwrap_or((false, None, None));

        Ok(TaxConfig {
            org_id,
            tax_exempt,
            tax_ids,
            billing_country,
            billing_postal_code,
        })
    }

    /// Set tax exempt status for an organization
    pub async fn set_tax_exempt(&self, org_id: Uuid, exempt: bool) -> BillingResult<()> {
        // Update Stripe customer
        let customer_id: Option<(String,)> =
            sqlx::query_as("SELECT stripe_customer_id FROM organizations WHERE id = $1")
                .bind(org_id)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| BillingError::Database(e.to_string()))?;

        if let Some((customer_id,)) = customer_id {
            if !customer_id.is_empty() {
                let customer_id_parsed = customer_id
                    .parse::<stripe::CustomerId>()
                    .map_err(|e| BillingError::StripeApi(format!("Invalid customer ID: {}", e)))?;

                let mut update = stripe::UpdateCustomer::new();
                update.tax_exempt = Some(if exempt {
                    stripe::CustomerTaxExemptFilter::Exempt
                } else {
                    stripe::CustomerTaxExemptFilter::None
                });

                stripe::Customer::update(self.stripe.inner(), &customer_id_parsed, update).await?;
            }
        }

        // Update database
        sqlx::query("UPDATE organizations SET tax_exempt = $1, updated_at = NOW() WHERE id = $2")
            .bind(exempt)
            .bind(org_id)
            .execute(&self.pool)
            .await
            .map_err(|e| BillingError::Database(e.to_string()))?;

        tracing::info!(
            org_id = %org_id,
            tax_exempt = exempt,
            "Updated tax exempt status"
        );

        Ok(())
    }

    /// Update billing address (used for tax calculation)
    pub async fn update_billing_address(
        &self,
        org_id: Uuid,
        country: &str,
        postal_code: Option<&str>,
        city: Option<&str>,
        state: Option<&str>,
    ) -> BillingResult<()> {
        // Update Stripe customer
        let customer_id: Option<(String,)> =
            sqlx::query_as("SELECT stripe_customer_id FROM organizations WHERE id = $1")
                .bind(org_id)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| BillingError::Database(e.to_string()))?;

        if let Some((customer_id,)) = customer_id {
            if !customer_id.is_empty() {
                let customer_id_parsed = customer_id
                    .parse::<stripe::CustomerId>()
                    .map_err(|e| BillingError::StripeApi(format!("Invalid customer ID: {}", e)))?;

                let mut update = stripe::UpdateCustomer::new();
                let mut address = stripe::Address::default();
                address.country = Some(country.to_string());
                if let Some(pc) = postal_code {
                    address.postal_code = Some(pc.to_string());
                }
                if let Some(c) = city {
                    address.city = Some(c.to_string());
                }
                if let Some(s) = state {
                    address.state = Some(s.to_string());
                }
                update.address = Some(address);

                stripe::Customer::update(self.stripe.inner(), &customer_id_parsed, update).await?;
            }
        }

        // Update database
        sqlx::query(
            r#"
            UPDATE organizations
            SET billing_country = $1, billing_postal_code = $2, billing_city = $3, billing_state = $4, updated_at = NOW()
            WHERE id = $5
            "#
        )
        .bind(country)
        .bind(postal_code)
        .bind(city)
        .bind(state)
        .bind(org_id)
        .execute(&self.pool)
        .await
        .map_err(|e| BillingError::Database(e.to_string()))?;

        tracing::info!(
            org_id = %org_id,
            country = country,
            "Updated billing address"
        );

        Ok(())
    }

    /// Generate tax summary for reporting (from invoice data)
    pub async fn get_tax_summary(
        &self,
        org_id: Uuid,
        start_date: OffsetDateTime,
        end_date: OffsetDateTime,
    ) -> BillingResult<TaxSummary> {
        // Query tax data from billing events
        let tax_data: Vec<(String, i64, i64, f64)> = sqlx::query_as(
            r#"
            SELECT
                COALESCE(event_data->>'tax_jurisdiction', 'default') as jurisdiction,
                COALESCE((event_data->>'taxable_amount_cents')::bigint, 0) as taxable_amount,
                COALESCE((event_data->>'tax_amount_cents')::bigint, 0) as tax_amount,
                COALESCE((event_data->>'tax_rate_percent')::float, 0) as tax_rate
            FROM billing_events
            WHERE org_id = $1
              AND created_at >= $2
              AND created_at <= $3
              AND event_type IN ('INVOICE_PAID', 'OVERAGE_CHARGED', 'INSTANT_CHARGE')
              AND event_data->>'tax_amount_cents' IS NOT NULL
            "#,
        )
        .bind(org_id)
        .bind(start_date)
        .bind(end_date)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();

        let mut breakdown_map: std::collections::HashMap<String, TaxBreakdown> =
            std::collections::HashMap::new();
        let mut total_taxable = 0i64;
        let mut total_tax = 0i64;

        for (jurisdiction, taxable, tax, rate) in tax_data {
            total_taxable += taxable;
            total_tax += tax;

            let entry = breakdown_map
                .entry(jurisdiction.clone())
                .or_insert(TaxBreakdown {
                    jurisdiction,
                    tax_rate_percent: rate,
                    taxable_amount_cents: 0,
                    tax_amount_cents: 0,
                });
            entry.taxable_amount_cents += taxable;
            entry.tax_amount_cents += tax;
        }

        Ok(TaxSummary {
            org_id,
            period_start: start_date,
            period_end: end_date,
            total_taxable_amount_cents: total_taxable,
            total_tax_collected_cents: total_tax,
            tax_breakdown: breakdown_map.into_values().collect(),
        })
    }

    /// Verify a tax ID with Stripe
    pub async fn verify_tax_id(&self, org_id: Uuid, tax_id: Uuid) -> BillingResult<String> {
        let record: Option<(String, Option<String>)> = sqlx::query_as(
            "SELECT stripe_customer_id, stripe_tax_id FROM organization_tax_ids t
             JOIN organizations o ON t.org_id = o.id
             WHERE t.id = $1 AND t.org_id = $2",
        )
        .bind(tax_id)
        .bind(org_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| BillingError::Database(e.to_string()))?;

        let (_customer_id, stripe_tax_id) =
            record.ok_or_else(|| BillingError::NotFound("Tax ID not found".to_string()))?;

        let stripe_tax_id = stripe_tax_id
            .ok_or_else(|| BillingError::InvalidInput("Tax ID not linked to Stripe".to_string()))?;

        // For local tax IDs, return current status
        if stripe_tax_id.starts_with("txi_local_") {
            return Ok("local_only".to_string());
        }

        // Retrieve tax ID from Stripe to get latest verification status
        let tax_id_parsed = stripe_tax_id
            .parse::<stripe::TaxIdId>()
            .map_err(|e| BillingError::StripeApi(format!("Invalid tax ID: {}", e)))?;

        let tax_id_obj = stripe::TaxId::retrieve(self.stripe.inner(), &tax_id_parsed, &[]).await?;

        let status = tax_id_obj
            .verification
            .map(|v| format!("{:?}", v.status))
            .unwrap_or_else(|| "unknown".to_string());

        // Update database with latest status
        sqlx::query("UPDATE organization_tax_ids SET verification_status = $1, updated_at = NOW() WHERE id = $2")
            .bind(&status)
            .bind(tax_id)
            .execute(&self.pool)
            .await
            .map_err(|e| BillingError::Database(e.to_string()))?;

        Ok(status)
    }
}
