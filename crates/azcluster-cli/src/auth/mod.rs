//! Azure authentication module.
//!
//! Native OAuth2 token acquisition + refresh. Replaces `az` CLI dependency for
//! operator-side ARM calls. Token cache at `~/.azure/azcli_tokens.json` (mode 0o600),
//! NOT compatible with Python `az` CLI's MSAL binary cache.

pub mod cache;
pub mod device_code;
pub mod interactive;
pub mod token_provider;

pub use cache::TokenCache;
pub use token_provider::TokenProvider;

use serde::Deserialize;

/// Well-known Azure CLI public client ID. Allows the public-client OAuth2 flows
/// (PKCE auth-code + device-code) without registering our own app.
pub const AZURE_CLI_CLIENT_ID: &str = "04b07795-8ddb-461a-bbee-02f9e1bf7b46";

/// ARM scope. `offline_access` is REQUIRED to get a refresh_token back.
pub const MANAGEMENT_SCOPE: &str = "https://management.azure.com/.default offline_access";

pub const VAULT_SCOPE: &str = "https://vault.azure.net/.default offline_access";

pub const GRAFANA_SCOPE: &str = "ce34865e-cb55-4dbc-8d7c-12f1cfcd1c01/.default offline_access";

/// Default tenant for first-time login. `organizations` = any Entra ID tenant.
pub const COMMON_TENANT: &str = "organizations";

pub fn token_endpoint(tenant: &str) -> String {
    format!("https://login.microsoftonline.com/{tenant}/oauth2/v2.0/token")
}

pub fn authorize_endpoint(tenant: &str) -> String {
    format!("https://login.microsoftonline.com/{tenant}/oauth2/v2.0/authorize")
}

pub fn device_code_endpoint(tenant: &str) -> String {
    format!("https://login.microsoftonline.com/{tenant}/oauth2/v2.0/devicecode")
}

#[derive(Debug, Deserialize)]
pub struct OAuthTokenResponse {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: Option<i64>,
    #[allow(dead_code)]
    pub token_type: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct OAuthErrorResponse {
    pub error: String,
    pub error_description: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SubscriptionInfo {
    #[serde(rename = "subscriptionId")]
    pub subscription_id: String,
    #[serde(rename = "displayName")]
    pub display_name: Option<String>,
    #[serde(rename = "tenantId")]
    #[allow(dead_code)]
    pub tenant_id: Option<String>,
    #[allow(dead_code)]
    pub state: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SubscriptionListResponse {
    value: Vec<SubscriptionInfo>,
}

pub fn list_subscriptions(access_token: &str) -> anyhow::Result<Vec<SubscriptionInfo>> {
    use anyhow::{bail, Context};
    let client = reqwest::blocking::Client::new();
    let resp = client
        .get("https://management.azure.com/subscriptions?api-version=2022-12-01")
        .bearer_auth(access_token)
        .send()
        .context("Failed to list subscriptions")?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        bail!("List subscriptions failed ({status}): {body}");
    }
    let list: SubscriptionListResponse =
        resp.json().context("Failed to parse subscription list")?;
    Ok(list.value)
}
