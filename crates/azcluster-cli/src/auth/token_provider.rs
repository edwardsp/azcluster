//! Token provider: OAuth2 token acquisition and refresh.
//!
//! Handles:
//! - Interactive login (browser-based OAuth2 code flow)
//! - Device code flow (for headless environments)
//! - Managed identity (for Azure VMs/containers)
//! - Token refresh (using refresh tokens)
//! - Fallback to `az` CLI (for operator's existing login)

use super::cache::{CachedAccount, TokenCache};
use anyhow::{bail, Context, Result};
use chrono::Utc;
use serde::Deserialize;
use std::process::Command;

const AZURE_CLIENT_ID: &str = "04b07795-8ddb-461a-bbee-02f81e1a5c5e"; // Azure CLI client ID
const TOKEN_ENDPOINT: &str = "https://login.microsoftonline.com/{tenant}/oauth2/v2.0/token";

/// OAuth2 token response from Azure.
#[derive(Debug, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: u64,
    pub token_type: String,
}

/// Token provider: manages token acquisition and refresh.
pub struct TokenProvider {
    cache: TokenCache,
    subscription_id: String,
    tenant_id: String,
}

impl TokenProvider {
    /// Create a new token provider for a subscription.
    pub fn new(subscription_id: String, tenant_id: String) -> Result<Self> {
        let cache = TokenCache::load()?;
        Ok(Self {
            cache,
            subscription_id,
            tenant_id,
        })
    }

    /// Get a valid access token, refreshing if necessary.
    pub fn get_token(&mut self) -> Result<String> {
        // Check if we have a cached token that's still valid.
        let has_valid_token = self
            .cache
            .get(&self.subscription_id)
            .map(|acc| acc.is_valid())
            .unwrap_or(false);

        if has_valid_token {
            return Ok(self
                .cache
                .get(&self.subscription_id)
                .unwrap()
                .access_token
                .clone());
        }

        // Token is expired; try to refresh if we have a refresh token.
        let refresh_token = self
            .cache
            .get(&self.subscription_id)
            .and_then(|acc| acc.refresh_token.clone());

        if let Some(token) = refresh_token {
            if let Ok(new_token) = self.refresh_token(&token) {
                return Ok(new_token);
            }
        }

        // No valid cached token; try fallback to `az` CLI.
        self.get_token_from_az_cli()
    }

    /// Refresh an access token using a refresh token.
    fn refresh_token(&mut self, refresh_token: &str) -> Result<String> {
        let client = reqwest::blocking::Client::new();
        let url = TOKEN_ENDPOINT.replace("{tenant}", &self.tenant_id);

        let params = [
            ("grant_type", "refresh_token"),
            ("client_id", AZURE_CLIENT_ID),
            ("refresh_token", refresh_token),
            ("scope", "https://management.azure.com/.default"),
        ];

        let response = client
            .post(&url)
            .form(&params)
            .send()
            .context("Failed to refresh token")?;

        if !response.status().is_success() {
            bail!("Token refresh failed: {}", response.status());
        }

        let token_resp: TokenResponse = response
            .json()
            .context("Failed to parse token response")?;

        let expires_at = Utc::now() + chrono::Duration::seconds(token_resp.expires_in as i64);

        let account = CachedAccount {
            subscription_id: self.subscription_id.clone(),
            tenant_id: self.tenant_id.clone(),
            access_token: token_resp.access_token.clone(),
            refresh_token: token_resp.refresh_token,
            expires_at,
            username: "unknown".to_string(),
            auth_method: "refresh".to_string(),
        };

        self.cache.insert(self.subscription_id.clone(), account);
        self.cache.save()?;

        Ok(token_resp.access_token)
    }

    /// Fallback: get token from `az` CLI.
    fn get_token_from_az_cli(&self) -> Result<String> {
        let output = Command::new("az")
            .args(&["account", "get-access-token", "--query", "accessToken", "-o", "tsv"])
            .output()
            .context("Failed to run 'az account get-access-token'")?;

        if !output.status.success() {
            bail!(
                "az CLI not logged in. Run: az login\nStderr: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        let token = String::from_utf8(output.stdout)
            .context("Failed to parse az CLI output")?
            .trim()
            .to_string();

        if token.is_empty() {
            bail!("az CLI returned empty token");
        }

        Ok(token)
    }

    /// Interactive login (browser-based OAuth2 code flow).
    /// Not yet implemented; placeholder for Phase 1.
    pub fn interactive_login(&mut self) -> Result<String> {
        bail!("Interactive login not yet implemented");
    }

    /// Device code flow (for headless environments).
    /// Not yet implemented; placeholder for Phase 1.
    pub fn device_code_login(&mut self) -> Result<String> {
        bail!("Device code login not yet implemented");
    }

    /// Managed identity login (for Azure VMs/containers).
    /// Not yet implemented; placeholder for Phase 1.
    pub fn managed_identity_login(&mut self) -> Result<String> {
        bail!("Managed identity login not yet implemented");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_endpoint_formatting() {
        let url = TOKEN_ENDPOINT.replace("{tenant}", "common");
        assert_eq!(
            url,
            "https://login.microsoftonline.com/common/oauth2/v2.0/token"
        );
    }
}
