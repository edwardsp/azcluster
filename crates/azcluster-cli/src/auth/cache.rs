//! Token cache: persistent storage for Azure OAuth2 tokens.
//!
//! Stores tokens in JSON format at `~/.azure/azcli_tokens.json` (mode 0o600).
//! NOT compatible with Python `az` CLI's MSAL binary cache.

#![allow(dead_code)]

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

/// Token cache entry for a single account.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedAccount {
    /// Azure subscription ID (GUID).
    pub subscription_id: String,
    /// Tenant ID (GUID).
    pub tenant_id: String,
    /// Access token (JWT).
    pub access_token: String,
    /// Refresh token (opaque string).
    pub refresh_token: Option<String>,
    /// Token expiry time (UTC ISO8601).
    pub expires_at: DateTime<Utc>,
    /// Username or service principal ID.
    pub username: String,
    /// Auth method used (interactive, device_code, managed_identity, etc).
    pub auth_method: String,
    /// Vault-scope access token. Cached separately from `access_token` because
    /// Key Vault rejects management-scope tokens (different `aud` claim).
    #[serde(default)]
    pub vault_access_token: Option<String>,
    #[serde(default)]
    pub vault_expires_at: Option<DateTime<Utc>>,
}

impl CachedAccount {
    pub fn is_vault_token_valid(&self) -> bool {
        match (&self.vault_access_token, self.vault_expires_at) {
            (Some(_), Some(exp)) => Utc::now() + chrono::Duration::minutes(5) < exp,
            _ => false,
        }
    }
}

impl CachedAccount {
    /// Check if the access token is still valid (with 5-minute buffer).
    pub fn is_valid(&self) -> bool {
        let now = Utc::now();
        let buffer = chrono::Duration::minutes(5);
        now + buffer < self.expires_at
    }

    /// Check if the token is expired.
    pub fn is_expired(&self) -> bool {
        Utc::now() > self.expires_at
    }
}

/// In-memory token cache (keyed by subscription_id).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TokenCache {
    pub accounts: HashMap<String, CachedAccount>,
}

impl TokenCache {
    /// Load cache from disk (`~/.azure/azcli_tokens.json`).
    pub fn load() -> Result<Self> {
        let path = Self::cache_path()?;
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = fs::read_to_string(&path).context("Failed to read token cache")?;
        let cache: TokenCache =
            serde_json::from_str(&content).context("Failed to parse token cache JSON")?;
        Ok(cache)
    }

    /// Save cache to disk with mode 0o600 (owner read/write only).
    pub fn save(&self) -> Result<()> {
        let path = Self::cache_path()?;
        let dir = path.parent().ok_or_else(|| anyhow!("Invalid cache path"))?;
        fs::create_dir_all(dir).context("Failed to create cache directory")?;

        let json = serde_json::to_string_pretty(self).context("Failed to serialize token cache")?;
        fs::write(&path, json).context("Failed to write token cache")?;

        // Set permissions to 0o600 (owner read/write only).
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = fs::Permissions::from_mode(0o600);
            fs::set_permissions(&path, perms).context("Failed to set cache file permissions")?;
        }

        Ok(())
    }

    /// Get the cache file path: `~/.azure/azcli_tokens.json`.
    fn cache_path() -> Result<PathBuf> {
        let home = dirs::home_dir().ok_or_else(|| anyhow!("Could not determine home directory"))?;
        Ok(home.join(".azure").join("azcli_tokens.json"))
    }

    /// Get a cached account by subscription ID.
    pub fn get(&self, subscription_id: &str) -> Option<&CachedAccount> {
        self.accounts.get(subscription_id)
    }

    /// Get a mutable cached account by subscription ID.
    pub fn get_mut(&mut self, subscription_id: &str) -> Option<&mut CachedAccount> {
        self.accounts.get_mut(subscription_id)
    }

    /// Insert or update a cached account.
    pub fn insert(&mut self, subscription_id: String, account: CachedAccount) {
        self.accounts.insert(subscription_id, account);
    }

    /// Remove a cached account.
    pub fn remove(&mut self, subscription_id: &str) -> Option<CachedAccount> {
        self.accounts.remove(subscription_id)
    }

    /// Clear all cached accounts.
    pub fn clear(&mut self) {
        self.accounts.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cached_account_is_valid() {
        let future = Utc::now() + chrono::Duration::hours(1);
        let account = CachedAccount {
            subscription_id: "sub-123".to_string(),
            tenant_id: "tenant-456".to_string(),
            access_token: "token".to_string(),
            refresh_token: None,
            expires_at: future,
            username: "user@example.com".to_string(),
            auth_method: "interactive".to_string(),
            vault_access_token: None,
            vault_expires_at: None,
        };
        assert!(account.is_valid());
        assert!(!account.is_expired());
    }

    #[test]
    fn test_cached_account_is_expired() {
        let past = Utc::now() - chrono::Duration::hours(1);
        let account = CachedAccount {
            subscription_id: "sub-123".to_string(),
            tenant_id: "tenant-456".to_string(),
            access_token: "token".to_string(),
            refresh_token: None,
            expires_at: past,
            username: "user@example.com".to_string(),
            auth_method: "interactive".to_string(),
            vault_access_token: None,
            vault_expires_at: None,
        };
        assert!(!account.is_valid());
        assert!(account.is_expired());
    }

    #[test]
    fn test_token_cache_insert_get() {
        let mut cache = TokenCache::default();
        let account = CachedAccount {
            subscription_id: "sub-123".to_string(),
            tenant_id: "tenant-456".to_string(),
            access_token: "token".to_string(),
            refresh_token: None,
            expires_at: Utc::now() + chrono::Duration::hours(1),
            username: "user@example.com".to_string(),
            auth_method: "interactive".to_string(),
            vault_access_token: None,
            vault_expires_at: None,
        };
        cache.insert("sub-123".to_string(), account.clone());
        assert_eq!(cache.get("sub-123").unwrap().username, "user@example.com");
    }
}
