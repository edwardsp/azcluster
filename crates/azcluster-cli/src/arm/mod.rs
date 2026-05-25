//! Azure Resource Manager (ARM) integration.
//!
//! Provides ARM REST client, LRO polling, configuration, and related utilities.

pub mod client;
pub mod config;
pub mod lro;

pub use client::ArmClient;
pub use config::ApiVersionConfig;
pub use lro::{LroConfig, LroPoller};
