//! Azure Bastion native client.
//!
//! Provides WebSocket-based tunneling to Azure resources via Bastion.
//! Replaces the need for SSH ProxyCommand through Bastion.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Bastion SKU types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BastionSku {
    Basic,
    Standard,
    Premium,
    Developer,
    QuickConnect,
}

impl BastionSku {
    /// Parse SKU from string.
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "basic" => Some(BastionSku::Basic),
            "standard" => Some(BastionSku::Standard),
            "premium" => Some(BastionSku::Premium),
            "developer" => Some(BastionSku::Developer),
            "quickconnect" => Some(BastionSku::QuickConnect),
            _ => None,
        }
    }

    /// Check if this SKU supports native client.
    pub fn supports_native_client(&self) -> bool {
        matches!(
            self,
            BastionSku::Standard
                | BastionSku::Premium
                | BastionSku::Developer
                | BastionSku::QuickConnect
        )
    }
}

/// Token response from Bastion API.
#[derive(Debug, Deserialize, Serialize)]
pub struct BastionTokenResponse {
    pub auth_token: String,
    pub websocket_token: String,
    pub node_id: String,
}

/// Bastion host information.
#[derive(Debug, Deserialize, Serialize)]
pub struct BastionHost {
    pub id: String,
    pub name: String,
    pub location: String,
    pub sku: Option<BastionHostSku>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct BastionHostSku {
    pub name: String,
}

/// Bastion client for native WebSocket tunneling.
pub struct BastionClient {
    subscription_id: String,
    access_token: String,
}

impl BastionClient {
    /// Create a new Bastion client.
    pub fn new(access_token: String, subscription_id: String) -> Self {
        Self {
            subscription_id,
            access_token,
        }
    }

    /// Get a Bastion host.
    pub fn get_bastion_host(
        &self,
        resource_group: &str,
        bastion_name: &str,
    ) -> Result<BastionHost> {
        let url = format!(
            "https://management.azure.com/subscriptions/{}/resourceGroups/{}/providers/Microsoft.Network/bastionHosts/{}?api-version=2024-01-01",
            self.subscription_id, resource_group, bastion_name
        );

        let client = reqwest::blocking::Client::new();
        let response = client
            .get(&url)
            .bearer_auth(&self.access_token)
            .send()
            .context("Failed to get Bastion host")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            bail!("Get Bastion host failed ({status}): {body}");
        }

        response
            .json()
            .context("Failed to parse Bastion host response")
    }

    /// Get a tunnel token for a resource.
    /// This is a placeholder; full implementation requires async WebSocket handling.
    pub fn get_tunnel_token(
        &self,
        bastion_endpoint: &str,
        resource_id: &str,
        resource_port: u16,
    ) -> Result<BastionTokenResponse> {
        let url = format!("https://{}/api/tokens", bastion_endpoint);

        let body = json!({
            "resourceId": resource_id,
            "resourcePort": resource_port,
        });

        let client = reqwest::blocking::Client::new();
        let response = client
            .post(&url)
            .bearer_auth(&self.access_token)
            .json(&body)
            .send()
            .context("Failed to get tunnel token")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            bail!("Get tunnel token failed ({status}): {body}");
        }

        response
            .json()
            .context("Failed to parse tunnel token response")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bastion_sku_from_str() {
        assert_eq!(BastionSku::from_str("standard"), Some(BastionSku::Standard));
        assert_eq!(BastionSku::from_str("PREMIUM"), Some(BastionSku::Premium));
        assert_eq!(BastionSku::from_str("invalid"), None);
    }

    #[test]
    fn test_bastion_sku_supports_native_client() {
        assert!(BastionSku::Standard.supports_native_client());
        assert!(BastionSku::Premium.supports_native_client());
        assert!(BastionSku::Developer.supports_native_client());
        assert!(BastionSku::QuickConnect.supports_native_client());
        assert!(!BastionSku::Basic.supports_native_client());
    }

    #[test]
    fn test_bastion_client_new() {
        let client = BastionClient::new("token".to_string(), "sub-123".to_string());
        assert_eq!(client.subscription_id, "sub-123");
    }
}
