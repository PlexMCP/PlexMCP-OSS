//! Authentication module for PlexMCP

pub mod api_key;
#[cfg(test)]
mod edge_case_tests;
pub mod jwt;
pub mod middleware;
#[cfg(test)]
mod middleware_tests;
pub mod password;
pub mod sessions;
pub mod tokens;
pub mod totp;

pub use api_key::ApiKeyManager;
pub use jwt::{Claims, JwtManager, TokenType};
pub(crate) use middleware::{InFlightRequests, TokenCache};
pub use middleware::{
    optional_auth, require_active_member, require_auth, require_auth_with_billing,
    require_billing_active, require_full_access, AuthMethod, AuthState, AuthUser,
};
pub use password::{
    generate_impossible_hash, hash_password, validate_password_strength, verify_password,
};
pub use tokens::{TokenError, TokenManager, TokenType as VerificationTokenType};
pub use totp::TotpError;
