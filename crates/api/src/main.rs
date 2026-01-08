// API server clippy configuration
#![allow(dead_code)] // Contains methods for future use
#![allow(unused_imports)] // Contains imports for conditional/future use
#![allow(clippy::useless_vec)]
#![allow(clippy::single_match)]
#![allow(clippy::needless_borrows_for_generic_args)]
#![allow(clippy::format_in_format_args)]
#![allow(clippy::inconsistent_digit_grouping)]
#![allow(clippy::expect_fun_call)]
#![allow(clippy::type_complexity)]
#![allow(clippy::if_same_then_else)]
#![allow(clippy::suspicious_doc_comments)]
#![cfg_attr(test, allow(clippy::expect_used))]
#![cfg_attr(test, allow(clippy::unwrap_used))]

//! PlexMCP API Server
//!
//! The main API server for PlexMCP, providing authentication,
//! organization management, MCP routing, and billing endpoints.

mod alerting;
mod audit_constants;
mod auth;
mod config;
mod email;
mod error;
mod flyio;
mod mcp;
mod routes;
mod routing;
mod security;
mod state;
mod websocket;

use std::net::SocketAddr;

use axum::http::{header, Method};
use axum::middleware;
use plexmcp_shared::{create_migration_pool, create_pool};
use tower_http::{
    cors::{AllowOrigin, CorsLayer},
    trace::TraceLayer,
};

use crate::security::security_headers_middleware;
use time::OffsetDateTime;
use tokio::time::{interval, Duration};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use crate::{config::Config, routes::create_router, state::AppState};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load .env file if present
    dotenvy::dotenv().ok();

    // Initialize tracing
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,plexmcp_api=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    tracing::info!("Starting PlexMCP API Server v{}", env!("CARGO_PKG_VERSION"));

    // Load configuration
    let config = Config::from_env()?;
    tracing::info!("Configuration loaded");

    // NOTE: Database validation removed for self-hosted deployments.
    // Self-hosters can use any PostgreSQL-compatible database.

    // Create database pool (using pooler URL for regular queries)
    tracing::info!("Connecting to database...");
    let pool = create_pool(&config.database_url).await?;
    tracing::info!("Database connection established");

    // DATABASE FINGERPRINT AT STARTUP - Verify which database we connected to
    tracing::info!("Fetching database fingerprint...");
    let db_fingerprint: Result<(String, String, String, String, i32, i32), sqlx::Error> =
        sqlx::query_as(
            r#"SELECT
            current_database()::TEXT,
            current_schema()::TEXT,
            current_setting('search_path')::TEXT,
            inet_server_addr()::TEXT,
            inet_server_port(),
            (SELECT COUNT(*)::INT FROM public.users) as user_count
        "#,
        )
        .fetch_one(&pool)
        .await;

    match db_fingerprint {
        Ok((db_name, schema, search_path, server_ip, server_port, user_count)) => {
            tracing::info!(
                startup_db_fingerprint = "v435",
                db_name = %db_name,
                schema = %schema,
                search_path = %search_path,
                server_ip = %server_ip,
                server_port = %server_port,
                user_count = %user_count,
                "Database fingerprint captured at startup"
            );
        }
        Err(e) => {
            tracing::error!(startup_db_fingerprint = "v435", error = ?e, "Failed to fetch database fingerprint at startup");
        }
    }

    // Run migrations using direct URL (bypasses PgBouncer which doesn't support prepared statements)
    // Uses migration-specific pool with longer timeouts
    // NOTE: Migrations disabled - contact fields migration applied
    // tracing::info!("Running database migrations...");
    // let migration_url = config.database_direct_url.as_ref().unwrap_or(&config.database_url);
    // let migration_pool = create_migration_pool(migration_url).await?;
    // plexmcp_shared::run_migrations(&migration_pool).await?;
    // migration_pool.close().await;
    tracing::info!("Database migrations skipped (already applied)");

    // V436: Query and log ALL RLS policies on staff_email_assignments table
    tracing::info!("Querying RLS policies on staff_email_assignments table...");
    let rls_policies: Result<Vec<(String, String, Option<String>, Option<String>)>, sqlx::Error> =
        sqlx::query_as(
            r#"SELECT
            polname::TEXT,
            CASE polcmd
                WHEN 'r' THEN 'SELECT'
                WHEN 'a' THEN 'INSERT'
                WHEN 'w' THEN 'UPDATE'
                WHEN 'd' THEN 'DELETE'
                WHEN '*' THEN 'ALL'
                ELSE polcmd::TEXT
            END as command,
            pg_get_expr(polqual, polrelid) as using_clause,
            pg_get_expr(polwithcheck, polrelid) as with_check_clause
        FROM pg_policy
        WHERE polrelid = 'staff_email_assignments'::regclass
        ORDER BY polname"#,
        )
        .fetch_all(&pool)
        .await;

    match rls_policies {
        Ok(policies) => {
            tracing::info!(
                v436_rls_policies = "staff_email_assignments",
                policy_count = policies.len(),
                "RLS policies found"
            );
            for (policy_name, command, using_clause, with_check) in policies {
                tracing::info!(
                    v436_rls_policy = %policy_name,
                    command = %command,
                    using_clause = ?using_clause,
                    with_check_clause = ?with_check,
                    "RLS policy details"
                );
            }
        }
        Err(e) => {
            tracing::error!(v436_rls_policies = "staff_email_assignments", error = ?e, "Failed to query RLS policies");
        }
    }

    // Ensure GeoIP database exists before creating state
    // This allows geoip_reader to be loaded immediately instead of being None until first background update
    tracing::info!("Checking for GeoIP database...");
    use std::path::Path;
    let db_path = "data/GeoLite2-City.mmdb";
    if !Path::new(db_path).exists() {
        tracing::info!("GeoIP database not found, downloading before startup...");
        if let Err(e) = update_geoip_database(&config.maxmind_license_key).await {
            tracing::error!(error = ?e, "Failed to download GeoIP database on startup - location tracking will be disabled");
        }
    } else {
        tracing::info!("GeoIP database file found");
    }

    // Create application state
    let state = AppState::new(pool.clone(), config.clone());

    // Start background alert checker task for website analytics
    tokio::spawn(routes::analytics_tracking::alert_checker_task(pool.clone()));
    tracing::info!("Analytics alert checker task started");

    // Start background GeoIP database update task (weekly)
    let state_for_geoip = state.clone();
    tokio::spawn(async move {
        geoip_update_task(state_for_geoip).await;
    });
    tracing::info!("GeoIP database update task started");

    // Start background usage aggregation task (hourly) - only when billing feature is enabled
    // SOC 2: Populates usage_aggregates for analytics; usage_records remains immutable
    #[cfg(feature = "billing")]
    {
        if let Some(billing) = &state.billing {
            let usage_meter = billing.usage.clone();
            tokio::spawn(async move {
                // Run immediately on startup to populate recent data
                match usage_meter.aggregate_all_recent().await {
                    Ok(count) => {
                        tracing::info!(count = count, "Initial usage aggregation complete")
                    }
                    Err(e) => tracing::error!(error = ?e, "Initial usage aggregation failed"),
                }

                // Then run every hour
                let mut interval = interval(Duration::from_secs(3600));
                interval.tick().await; // Skip first tick since we just ran

                loop {
                    interval.tick().await;
                    if let Err(e) = usage_meter.aggregate_all_recent().await {
                        tracing::error!(error = ?e, "Hourly usage aggregation failed");
                    }
                }
            });
            tracing::info!("Usage aggregation background task started");
        } else {
            tracing::warn!("Billing not configured - usage aggregation task not started");
        }
    }

    #[cfg(not(feature = "billing"))]
    tracing::info!("Usage aggregation skipped (billing feature not enabled)");

    // Build CORS layer - restrict to allowed origins only
    // SOC 2 CC6.1: Explicit origin allowlist prevents cross-origin attacks
    // Default to localhost for development/self-hosted; production should set ALLOWED_ORIGINS
    let allowed_origins: Vec<axum::http::HeaderValue> = std::env::var("ALLOWED_ORIGINS")
        .unwrap_or_else(|_| "http://localhost:3000,http://127.0.0.1:3000".to_string())
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect();

    tracing::info!(
        allowed_origins = ?allowed_origins,
        "CORS configured with {} allowed origins",
        allowed_origins.len()
    );

    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::list(allowed_origins))
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::DELETE,
            Method::PATCH,
            Method::OPTIONS,
        ])
        .allow_headers([
            header::AUTHORIZATION,
            header::CONTENT_TYPE,
            header::ACCEPT,
            header::ORIGIN,
            axum::http::HeaderName::from_static("x-csrf-token"),
        ])
        .expose_headers([header::CONTENT_TYPE])
        .allow_credentials(true);

    // Build the router
    // SOC 2 CC6.1: Security headers middleware adds X-Frame-Options, X-Content-Type-Options, etc.
    let app = create_router(state)
        .layer(middleware::from_fn(security_headers_middleware))
        .layer(cors)
        .layer(TraceLayer::new_for_http());

    // Parse bind address
    let addr: SocketAddr = config.bind_address.parse()?;
    tracing::info!("Starting server on {}", addr);

    // Start the server
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

/// Background task to update GeoIP database weekly
async fn geoip_update_task(state: AppState) {
    // Check every 24 hours
    let mut interval = interval(Duration::from_secs(24 * 60 * 60));
    let mut last_update: Option<OffsetDateTime> = None;

    loop {
        interval.tick().await;

        let now = OffsetDateTime::now_utc();

        // Update if 7+ days since last update
        let should_update = match last_update {
            None => true, // First run
            Some(last) => (now - last).whole_days() >= 7,
        };

        if !should_update {
            continue;
        }

        tracing::info!("Starting weekly GeoIP database update...");

        match update_geoip_database(&state.config.maxmind_license_key).await {
            Ok(_) => {
                last_update = Some(now);
                tracing::info!("GeoIP database updated successfully");
            }
            Err(e) => {
                tracing::error!(error = ?e, "Failed to update GeoIP database");
            }
        }
    }
}

/// Download and replace GeoIP database file
async fn update_geoip_database(license_key: &str) -> anyhow::Result<()> {
    use flate2::read::GzDecoder;
    use std::io::Read;
    use tar::Archive;
    use tokio::fs;

    if license_key.is_empty() {
        anyhow::bail!("MAXMIND_LICENSE_KEY not set");
    }

    let db_path = "data/GeoLite2-City.mmdb";
    let temp_path = "data/GeoLite2-City.mmdb.tmp";

    let download_url = format!(
        "https://download.maxmind.com/app/geoip_download?edition_id=GeoLite2-City&license_key={}&suffix=tar.gz",
        license_key
    );

    tracing::info!("Downloading GeoIP database...");

    // Download .tar.gz archive
    let client = reqwest::Client::new();
    let response = client.get(&download_url).send().await?.error_for_status()?;
    let bytes = response.bytes().await?;

    tracing::info!(
        size_mb = bytes.len() / 1024 / 1024,
        "Download complete, extracting..."
    );

    // Extract tar.gz in blocking task (tar operations are synchronous)
    let contents = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<u8>> {
        let decoder = GzDecoder::new(&bytes[..]);
        let mut archive = Archive::new(decoder);

        for entry in archive.entries()? {
            let mut entry = entry?;
            let path = entry.path()?;
            if path.extension().and_then(|s| s.to_str()) == Some("mmdb") {
                let mut contents = Vec::new();
                entry.read_to_end(&mut contents)?;
                return Ok(contents);
            }
        }
        anyhow::bail!("No .mmdb file found in downloaded archive")
    })
    .await??;

    // Write to temp file
    fs::write(temp_path, &contents).await?;

    // Validate before replacing
    maxminddb::Reader::open_readfile(temp_path)?;

    // Atomically replace old database
    fs::rename(temp_path, db_path).await?;

    tracing::info!("GeoIP database replaced successfully");
    Ok(())
}
