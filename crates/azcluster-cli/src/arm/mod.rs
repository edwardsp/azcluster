//! Azure Resource Manager (ARM) REST client module.
//!
//! Provides direct REST API access to Azure resources, replacing shell-out calls to `az` CLI.

pub mod client;

pub use client::{ArmClient, ApiVersions};
