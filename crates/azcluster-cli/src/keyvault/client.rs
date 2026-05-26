use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::thread;
use std::time::Duration;

const API_VERSION: &str = "7.4";

const RBAC_RETRY_MAX_ATTEMPTS: u32 = 10;
const RBAC_RETRY_DELAY_SECS: u64 = 30;

pub struct KeyVaultClient {
    client: reqwest::blocking::Client,
    vault_uri: String,
    access_token: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SecretBundle {
    pub value: String,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default, rename = "contentType")]
    pub content_type: Option<String>,
}

#[derive(Debug, Serialize)]
struct SetSecretBody<'a> {
    value: &'a str,
    #[serde(skip_serializing_if = "Option::is_none", rename = "contentType")]
    content_type: Option<&'a str>,
}

impl KeyVaultClient {
    pub fn new(vault_uri: impl Into<String>, access_token: impl Into<String>) -> Result<Self> {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .context("Failed to build Key Vault HTTP client")?;
        Ok(Self {
            client,
            vault_uri: vault_uri.into().trim_end_matches('/').to_string(),
            access_token: access_token.into(),
        })
    }

    pub fn get_secret(&self, name: &str) -> Result<SecretBundle> {
        let url = format!(
            "{}/secrets/{}?api-version={}",
            self.vault_uri, name, API_VERSION
        );
        self.with_rbac_retry(|| self.do_get(&url))
    }

    pub fn try_get_secret(&self, name: &str) -> Result<Option<SecretBundle>> {
        let url = format!(
            "{}/secrets/{}?api-version={}",
            self.vault_uri, name, API_VERSION
        );
        let resp = self
            .client
            .get(&url)
            .bearer_auth(&self.access_token)
            .send()
            .context("Failed to send Key Vault GET")?;
        let status = resp.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !status.is_success() {
            let body = resp.text().unwrap_or_default();
            bail!("Key Vault GET {name} failed ({status}): {body}");
        }
        let bundle: SecretBundle = resp.json().context("Failed to parse Key Vault response")?;
        Ok(Some(bundle))
    }

    pub fn set_secret(&self, name: &str, value: &str, content_type: Option<&str>) -> Result<()> {
        let url = format!(
            "{}/secrets/{}?api-version={}",
            self.vault_uri, name, API_VERSION
        );
        let body = SetSecretBody {
            value,
            content_type,
        };
        self.with_rbac_retry(|| self.do_put(&url, &json!(body)))?;
        Ok(())
    }

    pub fn delete_secret(&self, name: &str) -> Result<()> {
        let url = format!(
            "{}/secrets/{}?api-version={}",
            self.vault_uri, name, API_VERSION
        );
        let resp = self
            .client
            .delete(&url)
            .bearer_auth(&self.access_token)
            .send()
            .context("Failed to send Key Vault DELETE")?;
        let status = resp.status();
        if !status.is_success() && status != reqwest::StatusCode::NOT_FOUND {
            let body = resp.text().unwrap_or_default();
            bail!("Key Vault DELETE {name} failed ({status}): {body}");
        }
        Ok(())
    }

    fn do_get(&self, url: &str) -> Result<SecretBundle> {
        let resp = self
            .client
            .get(url)
            .bearer_auth(&self.access_token)
            .send()
            .context("Failed to send Key Vault GET")?;
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        if !status.is_success() {
            bail!("KV {status}: {body}");
        }
        serde_json::from_str(&body).context("Failed to parse Key Vault response")
    }

    fn do_put(&self, url: &str, body: &serde_json::Value) -> Result<()> {
        let resp = self
            .client
            .put(url)
            .bearer_auth(&self.access_token)
            .json(body)
            .send()
            .context("Failed to send Key Vault PUT")?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().unwrap_or_default();
            bail!("KV {status}: {text}");
        }
        Ok(())
    }

    fn with_rbac_retry<T, F>(&self, mut op: F) -> Result<T>
    where
        F: FnMut() -> Result<T>,
    {
        let mut last_err: Option<anyhow::Error> = None;
        for attempt in 1..=RBAC_RETRY_MAX_ATTEMPTS {
            match op() {
                Ok(v) => return Ok(v),
                Err(e) => {
                    let msg = format!("{e:#}");
                    if is_rbac_propagation_error(&msg) && attempt < RBAC_RETRY_MAX_ATTEMPTS {
                        eprintln!(
                            "Key Vault RBAC not yet propagated (attempt {attempt}/{RBAC_RETRY_MAX_ATTEMPTS}); retrying in {RBAC_RETRY_DELAY_SECS}s..."
                        );
                        thread::sleep(Duration::from_secs(RBAC_RETRY_DELAY_SECS));
                        last_err = Some(e);
                    } else {
                        return Err(e);
                    }
                }
            }
        }
        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("Key Vault retry loop exhausted")))
    }
}

fn is_rbac_propagation_error(msg: &str) -> bool {
    let lower = msg.to_lowercase();
    (lower.contains("403") || lower.contains("401"))
        && (lower.contains("forbidden")
            || lower.contains("does not have secrets")
            || lower.contains("does not have permission")
            || lower.contains("accessdenied")
            || lower.contains("unauthorized"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rbac_propagation_classifier() {
        assert!(is_rbac_propagation_error(
            "KV 403 Forbidden: caller does not have secrets get permission"
        ));
        assert!(is_rbac_propagation_error("KV 401 Unauthorized: ..."));
        assert!(is_rbac_propagation_error("KV 403: AccessDenied"));
        assert!(!is_rbac_propagation_error("KV 404 Not Found"));
        assert!(!is_rbac_propagation_error("KV 500 server explosion"));
        assert!(!is_rbac_propagation_error("connection refused"));
    }

    #[test]
    fn vault_uri_trims_trailing_slash() {
        let kv = KeyVaultClient::new("https://kv-x.vault.azure.net/", "tok").unwrap();
        assert_eq!(kv.vault_uri, "https://kv-x.vault.azure.net");
    }

    #[test]
    fn set_secret_body_omits_content_type_when_none() {
        let body = SetSecretBody {
            value: "v",
            content_type: None,
        };
        let s = serde_json::to_string(&body).unwrap();
        assert_eq!(s, r#"{"value":"v"}"#);
    }

    #[test]
    fn set_secret_body_includes_content_type() {
        let body = SetSecretBody {
            value: "v",
            content_type: Some("application/json"),
        };
        let s = serde_json::to_string(&body).unwrap();
        assert!(s.contains(r#""contentType":"application/json""#));
    }
}
