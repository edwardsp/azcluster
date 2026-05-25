//! Azure Bastion native client module.
//!
//! Provides WebSocket-based tunneling to Azure resources via Bastion.
//! Supports Standard, Premium, Developer, and QuickConnect SKUs.

pub mod client;

pub use client::{BastionClient, BastionHost, BastionSku, BastionTokenResponse};
