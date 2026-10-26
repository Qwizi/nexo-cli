//! Nexo CMS client used by the `nexo` CLI: profiles, device sign-in, tokens
//! kept in the OS keychain, API calls with refresh under a lock, and AI images.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod auth;
pub mod client;
pub mod config;
pub mod error;
pub mod images;
pub mod tokens;

pub use client::{Body, Client, FilePart};
pub use config::{Config, Paths, Profile};
pub use error::{Error, Result};
pub use tokens::{TokenStore, Tokens};

/// Version of the API contract the CLI speaks (`Cms-Version`).
pub const CMS_VERSION: &str = "2026-10-03";

/// `nexo-cli/0.1.0 (macos aarch64)`, sent with every request.
pub fn user_agent() -> String {
    format!(
        "nexo-cli/{} ({} {})",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    )
}
