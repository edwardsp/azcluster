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

/// Try to reuse an existing cached account (valid or refreshable) and rebind
/// it under `target_sub_id` without re-running the OAuth flow. The Azure
/// management-scope access token is principal-scoped, not subscription-scoped,
/// so the same token works against any subscription the user has access to.
///
/// Returns `Ok(Some(account))` on success, `Ok(None)` if no usable cached
/// account was found (caller should fall through to the full login flow),
/// or `Err` on a non-recoverable failure (e.g. token refresh rejected).
///
/// When `tenant_filter` is provided, only cached accounts matching that tenant
/// are considered.
pub fn try_rebind_cached(
    target_sub_id: &str,
    tenant_filter: Option<&str>,
) -> Result<Option<CachedAccount>> {
    let mut cache = TokenCache::load()?;

    let candidate_key = cache
        .accounts
        .iter()
        .filter(|(_, acc)| match tenant_filter {
            None => true,
            Some(t) => acc.tenant_id == t || t == "common" || t == "organizations",
        })
        .filter(|(_, acc)| acc.is_valid() || acc.refresh_token.is_some())
        .max_by_key(|(_, acc)| acc.expires_at)
        .map(|(k, _)| k.clone());

    let Some(key) = candidate_key else {
        return Ok(None);
    };

    let mut account = cache
        .accounts
        .get(&key)
        .cloned()
        .ok_or_else(|| anyhow!("cache race: candidate {key} vanished"))?;

    if !account.is_valid() {
        let refresh_token = account
            .refresh_token
            .clone()
            .ok_or_else(|| anyhow!("cached account {key} has no refresh token"))?;
        let mut provider = TokenProvider::new(key.clone(), account.tenant_id.clone())?;
        let new_access = provider.refresh_with_token(&refresh_token)?;
        let refreshed = TokenCache::load()?;
        if let Some(acc) = refreshed.get(&key) {
            account = acc.clone();
        } else {
            account.access_token = new_access;
        }
    }

    if key != target_sub_id {
        cache = TokenCache::load()?;
        cache.remove(&key);
        account.subscription_id = target_sub_id.to_string();
        account.auth_method = format!("{} (rebound)", account.auth_method);
        cache.insert(target_sub_id.to_string(), account.clone());
        cache.save()?;
    } else if account.subscription_id != target_sub_id {
        account.subscription_id = target_sub_id.to_string();
        cache.insert(target_sub_id.to_string(), account.clone());
        cache.save()?;
    }

    Ok(Some(account))
}

/// Decode the `upn` or `preferred_username` claim from an Azure JWT for display.
pub fn extract_username(token: &str) -> Result<String> {
    let claims = decode_jwt_claims(token)?;
    for key in ["upn", "preferred_username", "unique_name", "email"] {
        if let Some(v) = claims.get(key).and_then(|x| x.as_str()) {
            return Ok(v.to_string());
        }
    }
    bail!("no username claim found in JWT")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrincipalType {
    User,
    ServicePrincipal,
}

impl PrincipalType {
    /// Display string matching ARM role-assignment principalType values.
    pub fn as_arm_str(&self) -> &'static str {
        match self {
            PrincipalType::User => "User",
            PrincipalType::ServicePrincipal => "ServicePrincipal",
        }
    }
}

/// Extract `(object_id, principal_type)` from an Azure access token.
///
/// Reads the `oid` claim for the object ID and infers principal type from
/// the `idtyp` claim (Microsoft-specific: `user` / `app`). Falls back to the
/// presence of `upn` / `appid` claims if `idtyp` is absent. This replaces
/// the `az ad signed-in-user show` + `az ad sp show` round-trip — no
/// network call needed; the access token already carries the object ID.
pub fn extract_principal(token: &str) -> Result<(String, PrincipalType)> {
    let claims = decode_jwt_claims(token)?;
    let oid = claims
        .get("oid")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("no 'oid' claim in JWT - token may be malformed"))?
        .to_string();

    let principal_type = match claims.get("idtyp").and_then(|v| v.as_str()) {
        Some("user") => PrincipalType::User,
        Some("app") => PrincipalType::ServicePrincipal,
        _ => {
            if claims.get("upn").is_some() {
                PrincipalType::User
            } else if claims.get("appid").is_some() {
                PrincipalType::ServicePrincipal
            } else {
                PrincipalType::User
            }
        }
    };
    Ok((oid, principal_type))
}

fn decode_jwt_claims(token: &str) -> Result<serde_json::Value> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        bail!("invalid JWT: expected 3 segments, got {}", parts.len());
    }
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(parts[1])
        .context("failed to base64-decode JWT payload")?;
    serde_json::from_slice(&decoded).context("failed to parse JWT payload as JSON")
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

    fn make_token(payload_json: &[u8]) -> String {
        let header = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b"{}");
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload_json);
        format!("{header}.{payload}.sig")
    }

    #[test]
    fn extract_principal_user_via_idtyp() {
        let token = make_token(br#"{"oid":"11111111-1111-1111-1111-111111111111","idtyp":"user"}"#);
        let (oid, ptype) = extract_principal(&token).unwrap();
        assert_eq!(oid, "11111111-1111-1111-1111-111111111111");
        assert_eq!(ptype, PrincipalType::User);
        assert_eq!(ptype.as_arm_str(), "User");
    }

    #[test]
    fn extract_principal_sp_via_idtyp() {
        let token = make_token(br#"{"oid":"22222222-2222-2222-2222-222222222222","idtyp":"app"}"#);
        let (oid, ptype) = extract_principal(&token).unwrap();
        assert_eq!(oid, "22222222-2222-2222-2222-222222222222");
        assert_eq!(ptype, PrincipalType::ServicePrincipal);
        assert_eq!(ptype.as_arm_str(), "ServicePrincipal");
    }

    #[test]
    fn extract_principal_user_via_upn_fallback() {
        let token = make_token(
            br#"{"oid":"33333333-3333-3333-3333-333333333333","upn":"alice@example.com"}"#,
        );
        let (_, ptype) = extract_principal(&token).unwrap();
        assert_eq!(ptype, PrincipalType::User);
    }

    #[test]
    fn extract_principal_sp_via_appid_fallback() {
        let token =
            make_token(br#"{"oid":"44444444-4444-4444-4444-444444444444","appid":"deadbeef"}"#);
        let (_, ptype) = extract_principal(&token).unwrap();
        assert_eq!(ptype, PrincipalType::ServicePrincipal);
    }

    #[test]
    fn extract_principal_missing_oid_errors() {
        let token = make_token(br#"{"idtyp":"user"}"#);
        assert!(extract_principal(&token).is_err());
    }
}
