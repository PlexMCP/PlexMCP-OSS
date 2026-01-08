// API crate clippy configuration
#![allow(clippy::useless_vec)] // Vec preferred for API response patterns
#![allow(clippy::single_match)] // Clearer in some cases
#![allow(clippy::needless_borrows_for_generic_args)] // Sometimes needed for clarity
#![allow(clippy::format_in_format_args)] // Intentional in logging macros
#![allow(clippy::inconsistent_digit_grouping)] // Epoch timestamps don't use grouping
#![allow(clippy::expect_fun_call)] // Used for descriptive error messages
// Test code patterns:
#![cfg_attr(test, allow(clippy::expect_used))]
#![cfg_attr(test, allow(clippy::unwrap_used))]

//! PlexMCP API Library
//!
//! This crate contains the API server components for PlexMCP.

pub mod alerting;
pub mod audit_constants;
pub mod auth;
pub mod config;
pub mod email;
pub mod error;
pub mod flyio;
pub mod mcp;
pub mod routes;
pub mod routing;
pub mod security;
pub mod state;
pub mod websocket;

pub use config::Config;
pub use error::{ApiError, ApiResult};
pub use routing::{DomainCache, HostResolver, ResolvedOrg};
pub use state::AppState;
