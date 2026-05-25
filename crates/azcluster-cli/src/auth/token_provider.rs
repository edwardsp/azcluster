//! Token provider: OAuth2 token acquisition, refresh, and JWT introspection.
//!
//! Public flows: `interactive_login` (browser + PKCE), `device_code_login`
//! (headless / ssh). Refresh is automatic when a valid refresh_token is cached.
//! There is NO `az` CLI fallback - operators must run `azcluster login` once.

#![allow(dead_code)]

use super::cache::{CachedAccount, TokenCache};
use super::{
    device_code, interactive, token_endpoint, OAuthErrorResponse, OAuthTokenResponse,
    AZURE_CLI_CLIENT_ID, MANAGEMENT_SCOPE,
};
use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use chrono::{Duration, Utc};

pub struct TokenProvider {
    cache: TokenCache,
    subscription_id: String,
    tenant_id: String,
}

impl TokenProvider {
    pub fn new(subscription_id: String, tenant_id: String) -> Result<Self> {
        let cache = TokenCache::load()?;
        Ok(Self {
            cache,
            subscription_id,
            tenant_id,
        })
    }

    /// Get a valid access token. Uses cache if fresh, refreshes if expired with
    /// a valid refresh_token, otherwise bails with instructions to log in.
    pub fn get_token(&mut self) -> Result<String> {
        if let Some(acc) = self.cache.get(&self.subscription_id) {
            if acc.is_valid() {
                return Ok(acc.access_token.clone());
            }
        }

        let refresh_token = self
            .cache
            .get(&self.subscription_id)
            .and_then(|acc| acc.refresh_token.clone());

        if let Some(rt) = refresh_token {
            return self.refresh_with_token(&rt);
        }

        bail!(
            "No cached Azure credentials for subscription {}. Run: azcluster login",
            self.subscription_id
        );
    }

    fn refresh_with_token(&mut self, refresh_token: &str) -> Result<String> {
        let client = reqwest::blocking::Client::new();
        let url = token_endpoint(&self.tenant_id);

        let resp = client
            .post(&url)
            .form(&[
                ("grant_type", "refresh_token"),
                ("client_id", AZURE_CLI_CLIENT_ID),
                ("refresh_token", refresh_token),
                ("scope", MANAGEMENT_SCOPE),
            ])
            .send()
            .context("Failed to refresh Azure token")?;

        let status = resp.status();
        let body = resp.text().unwrap_or_default();

        if !status.is_success() {
            if let Ok(err) = serde_json::from_str::<OAuthErrorResponse>(&body) {
                bail!(
                    "Token refresh failed: {}: {}. Run: azcluster login",
                    err.error,
                    err.error_description.unwrap_or_default()
                );
            }
            bail!("Token refresh failed ({status}): {body}");
        }

        let token_resp: OAuthTokenResponse =
            serde_json::from_str(&body).context("Failed to parse refresh response")?;

        let expires_in = token_resp.expires_in.unwrap_or(3600);
        let expires_at = Utc::now() + Duration::seconds(expires_in);

        let username =
            extract_username(&token_resp.access_token).unwrap_or_else(|_| "unknown".to_string());

        let account = CachedAccount {
            subscription_id: self.subscription_id.clone(),
            tenant_id: self.tenant_id.clone(),
            access_token: token_resp.access_token.clone(),
            refresh_token: token_resp
                .refresh_token
                .or_else(|| Some(refresh_token.to_string())),
            expires_at,
            username,
            auth_method: "refresh".to_string(),
        };

        self.cache.insert(self.subscription_id.clone(), account);
        self.cache.save()?;

        Ok(token_resp.access_token)
    }
}

/// Run interactive browser login and persist the resulting account to the cache.
/// Subscription id is left blank; caller binds it via `bind_subscription` after
/// listing subscriptions with the new token.
pub fn run_interactive_login(tenant: Option<&str>) -> Result<CachedAccount> {
    let account = interactive::login(tenant)?;
    persist_unbound(account)
}

pub fn run_device_code_login(tenant: Option<&str>) -> Result<CachedAccount> {
    let account = device_code::login(tenant)?;
    persist_unbound(account)
}

fn persist_unbound(account: CachedAccount) -> Result<CachedAccount> {
    let mut cache = TokenCache::load()?;
    let key = if account.subscription_id.is_empty() {
        format!("_pending:{}", account.tenant_id)
    } else {
        account.subscription_id.clone()
    };
    cache.insert(key, account.clone());
    cache.save()?;
    Ok(account)
}

/// Bind a freshly-logged-in account to a specific subscription id and re-key it
/// under that subscription in the cache. Removes the pending placeholder.
pub fn bind_subscription(tenant_id: &str, subscription_id: &str) -> Result<CachedAccount> {
    let mut cache = TokenCache::load()?;
    let pending_key = format!("_pending:{tenant_id}");
    let mut account = cache
        .remove(&pending_key)
        .ok_or_else(|| anyhow!("no pending login for tenant {tenant_id}"))?;
    account.subscription_id = subscription_id.to_string();
    cache.insert(subscription_id.to_string(), account.clone());
    cache.save()?;
    Ok(account)
}

/// Decode the `upn` or `preferred_username` claim from an Azure JWT for display.
pub fn extract_username(token: &str) -> Result<String> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        bail!("invalid JWT: expected 3 segments, got {}", parts.len());
    }
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(parts[1])
        .context("failed to base64-decode JWT payload")?;
    let claims: serde_json::Value =
        serde_json::from_slice(&decoded).context("failed to parse JWT payload as JSON")?;

    for key in ["upn", "preferred_username", "unique_name", "email"] {
        if let Some(v) = claims.get(key).and_then(|x| x.as_str()) {
            return Ok(v.to_string());
        }
    }
    bail!("no username claim found in JWT")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_endpoint_formatting() {
        assert_eq!(
            token_endpoint("common"),
            "https://login.microsoftonline.com/common/oauth2/v2.0/token"
        );
    }

    #[test]
    fn extract_username_rejects_malformed() {
        assert!(extract_username("notajwt").is_err());
        assert!(extract_username("a.b").is_err());
    }

    #[test]
    fn extract_username_parses_upn() {
        let header = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b"{}");
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(br#"{"upn":"alice@example.com"}"#);
        let token = format!("{header}.{payload}.signature");
        assert_eq!(extract_username(&token).unwrap(), "alice@example.com");
    }

    #[test]
    fn extract_username_falls_back_to_preferred_username() {
        let header = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b"{}");
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(br#"{"preferred_username":"bob@example.com"}"#);
        let token = format!("{header}.{payload}.sig");
        assert_eq!(extract_username(&token).unwrap(), "bob@example.com");
    }
}
