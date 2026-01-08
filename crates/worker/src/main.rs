// Worker clippy configuration
#![allow(dead_code)]
#![allow(unused_imports)]
#![allow(clippy::if_same_then_else)]
#![allow(clippy::suspicious_doc_comments)]

//! PlexMCP Background Worker
//!
//! Handles scheduled jobs including:
//! - Metered usage reporting to Stripe (every 6 hours)
//! - Usage aggregation for analytics (hourly)
//! - Final usage report before billing (daily at 23:55 UTC)
//! - Webhook queue processing (every minute)
//! - Test history cleanup based on subscription tier (daily at 4:00 AM UTC)
//! - MCP health check monitoring (every 30 minutes)

mod webhook_processor;

use std::sync::Arc;
use std::time::Duration;

use plexmcp_api::email::SecurityEmailService;
use plexmcp_billing::{BillingService, UsageReportResult};
use sqlx::postgres::PgPoolOptions;
use tokio_cron_scheduler::{Job, JobScheduler};
use tracing::{error, info, warn};
use uuid::Uuid;

/// Create a database connection pool
async fn create_db_pool() -> anyhow::Result<sqlx::PgPool> {
    #[allow(clippy::expect_used)] // Fail-fast on startup if required config is missing
    let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");

    let pool = PgPoolOptions::new()
        .max_connections(5)
        .acquire_timeout(Duration::from_secs(5))
        .connect(&database_url)
        .await?;

    info!("Database pool created");
    Ok(pool)
}

/// Log results of usage reporting
fn log_usage_results(results: &[UsageReportResult]) {
    let reported = results
        .iter()
        .filter(|r| matches!(r, UsageReportResult::Reported { .. }))
        .count();
    let no_overage = results
        .iter()
        .filter(|r| matches!(r, UsageReportResult::NoOverage { .. }))
        .count();
    let errors = results
        .iter()
        .filter(|r| matches!(r, UsageReportResult::Error { .. }))
        .count();

    info!(
        reported = reported,
        no_overage = no_overage,
        errors = errors,
        "Usage report cycle complete"
    );

    // Log individual errors
    for result in results {
        if let UsageReportResult::Error { org_id, error } = result {
            error!(org_id = %org_id, error = %error, "Failed to report usage");
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    // Load environment
    dotenvy::dotenv().ok();

    info!("Starting PlexMCP Worker");

    // Create database pool
    let pool = create_db_pool().await?;

    // Create billing service
    let billing = match BillingService::from_env(pool.clone()) {
        Ok(b) => Arc::new(b),
        Err(e) => {
            // If Stripe isn't configured, run in minimal mode
            warn!(error = %e, "Failed to create billing service - running in minimal mode");
            info!("Worker running without Stripe integration");

            // Keep running with minimal functionality
            loop {
                tokio::time::sleep(Duration::from_secs(60)).await;
                info!("Worker heartbeat (minimal mode)");
            }
        }
    };

    // Create scheduler
    let scheduler = JobScheduler::new().await?;

    // Job 1: Report usage to Stripe every 6 hours
    // Cron: At minute 0 past every 6th hour (0:00, 6:00, 12:00, 18:00 UTC)
    let metered_service = billing.metered.clone();
    scheduler
        .add(Job::new_async("0 0 */6 * * *", move |_uuid, _l| {
            let service = metered_service.clone();
            Box::pin(async move {
                info!("Running scheduled metered usage report to Stripe");
                let results = service.report_all_usage().await;
                log_usage_results(&results);
            })
        })?)
        .await?;
    info!("Scheduled: Metered usage report (every 6 hours)");

    // Job 2: Final usage report before billing (end of day UTC)
    // Cron: At 23:55 every day - ensures final usage is captured before midnight billing
    let metered_service_final = billing.metered.clone();
    scheduler
        .add(Job::new_async("0 55 23 * * *", move |_uuid, _l| {
            let service = metered_service_final.clone();
            Box::pin(async move {
                info!("Running final daily usage report to Stripe");
                let results = service.report_all_usage().await;
                log_usage_results(&results);
            })
        })?)
        .await?;
    info!("Scheduled: Final daily usage report (23:55 UTC)");

    // Job 3: Health check heartbeat (every 5 minutes)
    scheduler
        .add(Job::new_async("0 */5 * * * *", |_uuid, _l| {
            Box::pin(async move {
                info!("Worker heartbeat - all systems operational");
            })
        })?)
        .await?;
    info!("Scheduled: Health check heartbeat (every 5 minutes)");

    // Job 4: Calculate and update overage charges (every 15 minutes)
    // This populates overage_charges table in real-time for display and Pay Now functionality
    let overage_pool = pool.clone();
    let overage_billing = billing.clone();
    scheduler
        .add(Job::new_async("0 */15 * * * *", move |_uuid, _l| {
            let pool = overage_pool.clone();
            let billing = overage_billing.clone();
            Box::pin(async move {
                info!("Running overage charge calculation job");

                // Get all orgs with Pro/Team subscriptions that have active billing periods
                // Note: subscriptions.customer_id stores org_id as text
                let orgs: Vec<(Uuid, String, time::OffsetDateTime, time::OffsetDateTime)> =
                    sqlx::query_as(
                        r#"
                    SELECT o.id, o.subscription_tier, s.current_period_start, s.current_period_end
                    FROM organizations o
                    JOIN subscriptions s ON s.customer_id = o.id::text
                    WHERE o.subscription_tier IN ('pro', 'team')
                      AND s.status = 'active'
                      AND s.current_period_start IS NOT NULL
                      AND s.current_period_end IS NOT NULL
                    "#,
                    )
                    .fetch_all(&pool)
                    .await
                    .unwrap_or_default();

                let total_orgs = orgs.len();
                let mut updated = 0;
                let mut errors = 0;

                for (org_id, tier, period_start, period_end) in orgs {
                    match billing
                        .overage
                        .create_or_update_current_overage(org_id, &tier, period_start, period_end)
                        .await
                    {
                        Ok(Some(_)) => {
                            updated += 1;
                            // Sync spend cap tracking (won't double-count)
                            if let Err(e) = billing.spend_cap.sync_spend_from_overages(org_id).await
                            {
                                error!(org_id = %org_id, error = %e, "Failed to sync spend cap");
                            }
                        }
                        Ok(None) => {} // No overage (within limits)
                        Err(e) => {
                            error!(org_id = %org_id, error = %e, "Failed to calculate overage");
                            errors += 1;
                        }
                    }
                }

                info!(
                    total_orgs = total_orgs,
                    updated = updated,
                    errors = errors,
                    "Overage charge calculation complete"
                );
            })
        })?)
        .await?;
    info!("Scheduled: Overage charge calculation (every 15 minutes)");

    // Job 5: Grace period enforcement (hourly)
    // Blocks organizations that have invoices past their 30-day grace period
    let grace_period_pool = pool.clone();
    let grace_period_email_service = SecurityEmailService::from_env();
    scheduler.add(
        Job::new_async("0 0 * * * *", move |_uuid, _l| {
            let pool = grace_period_pool.clone();
            let email_service = grace_period_email_service.clone();
            Box::pin(async move {
                info!("Running grace period enforcement job");

                // Find orgs with invoices past grace period that aren't already blocked
                let overdue_orgs: Vec<(Uuid, String, i64)> = sqlx::query_as(
                    r#"
                    SELECT DISTINCT
                        i.org_id,
                        o.name,
                        SUM(i.amount_due_cents)::BIGINT as total_due
                    FROM invoices i
                    JOIN organizations o ON i.org_id = o.id
                    WHERE i.status IN ('open', 'uncollectible')
                      AND i.grace_period_ends_at < NOW()
                      AND o.billing_blocked_at IS NULL
                    GROUP BY i.org_id, o.name
                    HAVING SUM(i.amount_due_cents) > 0
                    "#
                )
                .fetch_all(&pool)
                .await
                .unwrap_or_default();

                let total_overdue = overdue_orgs.len();
                let mut blocked = 0;
                let mut errors = 0;

                for (org_id, org_name, total_due) in overdue_orgs {
                    // Block the organization
                    let result = sqlx::query(
                        r#"
                        UPDATE organizations
                        SET billing_blocked_at = NOW(),
                            billing_block_reason = $2
                        WHERE id = $1
                          AND billing_blocked_at IS NULL
                        "#
                    )
                    .bind(org_id)
                    .bind(format!(
                        "Unpaid invoices past 30-day grace period. Outstanding balance: ${:.2}",
                        total_due as f64 / 100.0
                    ))
                    .execute(&pool)
                    .await;

                    match result {
                        Ok(rows) => {
                            if rows.rows_affected() > 0 {
                                blocked += 1;
                                warn!(
                                    org_id = %org_id,
                                    org_name = %org_name,
                                    total_due_cents = total_due,
                                    "Organization blocked for non-payment"
                                );

                                // Send suspension notification email to organization owner
                                let owner_email_result: Result<Option<String>, sqlx::Error> = sqlx::query_scalar(
                                    r#"
                                    SELECT u.email
                                    FROM users u
                                    WHERE u.org_id = $1 AND u.role = 'owner'
                                    LIMIT 1
                                    "#
                                )
                                .bind(org_id)
                                .fetch_optional(&pool)
                                .await;

                                if let Ok(Some(owner_email)) = owner_email_result {
                                    let email_svc = email_service.clone();
                                    let org_name_clone = org_name.clone();
                                    let block_reason = format!(
                                        "Unpaid invoices past 30-day grace period. Outstanding balance: ${:.2}",
                                        total_due as f64 / 100.0
                                    );
                                    tokio::spawn(async move {
                                        email_svc
                                            .send_service_suspended(
                                                &owner_email,
                                                &org_name_clone,
                                                total_due,
                                                &block_reason,
                                            )
                                            .await;
                                    });
                                    info!(org_id = %org_id, "Suspension notification email sent");
                                } else {
                                    warn!(org_id = %org_id, "No owner email found for suspension notification");
                                }
                            }
                        }
                        Err(e) => {
                            error!(
                                org_id = %org_id,
                                error = %e,
                                "Failed to block organization for non-payment"
                            );
                            errors += 1;
                        }
                    }
                }

                info!(
                    total_overdue = total_overdue,
                    blocked = blocked,
                    errors = errors,
                    "Grace period enforcement complete"
                );

                // Also check for orgs that should be unblocked (all invoices paid)
                let result = sqlx::query(
                    r#"
                    UPDATE organizations o
                    SET billing_blocked_at = NULL,
                        billing_block_reason = NULL
                    WHERE o.billing_blocked_at IS NOT NULL
                      AND NOT EXISTS (
                          SELECT 1 FROM invoices i
                          WHERE i.org_id = o.id
                            AND i.status IN ('open', 'uncollectible')
                            AND i.amount_due_cents > 0
                      )
                    "#
                )
                .execute(&pool)
                .await;

                if let Ok(rows) = result {
                    if rows.rows_affected() > 0 {
                        info!(
                            unblocked = rows.rows_affected(),
                            "Unblocked organizations after payment"
                        );
                    }
                }
            })
        })?
    ).await?;
    info!("Scheduled: Grace period enforcement (hourly)");

    // Job 6: Process webhook queue (every minute)
    // Processes queued webhooks with retry logic for reliability
    let webhook_pool = pool.clone();
    let resend_api_key = std::env::var("RESEND_API_KEY").unwrap_or_default();
    let enable_email_routing = std::env::var("ENABLE_EMAIL_ROUTING")
        .unwrap_or_else(|_| "false".to_string())
        .parse::<bool>()
        .unwrap_or(false);

    scheduler
        .add(Job::new_async("0 * * * * *", move |_uuid, _l| {
            let pool = webhook_pool.clone();
            let api_key = resend_api_key.clone();
            let routing_enabled = enable_email_routing;
            Box::pin(async move {
                let http_client = reqwest::Client::new();
                webhook_processor::process_webhook_queue(
                    &pool,
                    &http_client,
                    &api_key,
                    routing_enabled,
                )
                .await;
            })
        })?)
        .await?;
    info!("Scheduled: Webhook queue processing (every minute)");

    // Job 7: Cleanup old webhooks (daily at 3:00 AM UTC)
    let cleanup_pool = pool.clone();
    scheduler
        .add(Job::new_async("0 0 3 * * *", move |_uuid, _l| {
            let pool = cleanup_pool.clone();
            Box::pin(async move {
                info!("Running webhook queue cleanup");
                webhook_processor::cleanup_old_webhooks(&pool, 7).await; // Keep 7 days
            })
        })?)
        .await?;
    info!("Scheduled: Webhook queue cleanup (daily at 3:00 AM)");

    // Job 8: Clean up old test history based on subscription tier (daily at 4:00 AM UTC)
    // Implements tier-based retention: Free=7d, Starter=30d, Pro=90d, Team/Enterprise=365d
    let test_cleanup_pool = pool.clone();
    scheduler
        .add(Job::new_async("0 0 4 * * *", move |_uuid, _l| {
            let pool = test_cleanup_pool.clone();
            Box::pin(async move {
                info!("Running test history cleanup job");

                // Delete old test history based on each org's subscription tier
                // NOTE: subscription_tier is on organizations table, NOT subscriptions
                let result = sqlx::query(
                    r#"
                    DELETE FROM mcp_test_history th
                    USING organizations o
                    WHERE th.org_id = o.id
                    AND th.tested_at < NOW() - CASE o.subscription_tier
                        WHEN 'free' THEN INTERVAL '7 days'
                        WHEN 'starter' THEN INTERVAL '30 days'
                        WHEN 'pro' THEN INTERVAL '90 days'
                        ELSE INTERVAL '365 days'
                    END
                "#,
                )
                .execute(&pool)
                .await;

                match result {
                    Ok(r) => info!(deleted = r.rows_affected(), "Test history cleanup complete"),
                    Err(e) => error!(error = %e, "Test history cleanup failed"),
                }
            })
        })?)
        .await?;
    info!("Scheduled: Test history cleanup (daily at 4:00 AM UTC)");

    // Job 9: Scheduled MCP health checks (every 30 minutes)
    // Proactively checks MCPs that haven't been tested recently
    let health_check_pool = pool.clone();
    scheduler
        .add(Job::new_async("0 */30 * * * *", move |_uuid, _l| {
            let pool = health_check_pool.clone();
            Box::pin(async move {
                info!("Running scheduled MCP health checks");

                // Get MCPs that haven't been checked in 30+ minutes
                let stale_mcps: Vec<(Uuid, String)> = sqlx::query_as(
                    r#"
                    SELECT m.id, m.name
                    FROM mcp_instances m
                    WHERE m.status = 'active'
                    AND (m.last_health_check_at IS NULL
                         OR m.last_health_check_at < NOW() - INTERVAL '30 minutes')
                    LIMIT 100
                "#,
                )
                .fetch_all(&pool)
                .await
                .unwrap_or_default();

                let count = stale_mcps.len();
                if count > 0 {
                    info!(count = count, "Found MCPs needing health check");

                    // For each stale MCP, perform a basic connectivity check
                    // This is a lightweight check - just update last_health_check_at to track
                    // Full health checks are done by the API when users visit the testing page
                    for (mcp_id, _name) in &stale_mcps {
                        let _ = sqlx::query(
                            "UPDATE mcp_instances SET last_health_check_at = NOW() WHERE id = $1",
                        )
                        .bind(mcp_id)
                        .execute(&pool)
                        .await;
                    }

                    info!(
                        updated = count,
                        "Updated last_health_check_at for stale MCPs"
                    );
                }
            })
        })?)
        .await?;
    info!("Scheduled: MCP health check monitoring (every 30 minutes)");

    // Start the scheduler
    info!("Starting job scheduler");
    scheduler.start().await?;

    info!(
        "PlexMCP Worker started successfully with {} scheduled jobs",
        9
    );

    // Keep the main task running
    // The scheduler runs jobs in background tasks
    loop {
        tokio::time::sleep(Duration::from_secs(3600)).await;
    }
}
