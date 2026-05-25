//! Azure authentication module.
//!
//! Provides token caching, refresh, and acquisition without depending on the `az` CLI.
//! Supports multiple auth methods: interactive, device code, managed identity, and fallback to `az` CLI.

pub mod cache;
pub mod token_provider;

pub use cache::{CachedAccount, TokenCache};
pub use token_provider::TokenProvider;
