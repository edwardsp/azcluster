//! Azure Resource Manager (ARM) integration.
//!
//! Provides ARM REST client, LRO polling, and related utilities.

pub mod client;
pub mod lro;

pub use client::ArmClient;
pub use lro::{LroConfig, LroPoller};
