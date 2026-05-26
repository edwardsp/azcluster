use anyhow::{bail, Context, Result};
use chrono::{Duration, Utc};
use serde::Deserialize;
use std::thread;
use std::time::Duration as StdDuration;

use super::cache::CachedAccount;
use super::{
    device_code_endpoint, token_endpoint, OAuthErrorResponse, OAuthTokenResponse,
    AZURE_CLI_CLIENT_ID, COMMON_TENANT, MANAGEMENT_SCOPE,
};

#[derive(Debug, Deserialize)]
struct DeviceCodeResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    expires_in: i64,
    interval: Option<u64>,
    message: Option<String>,
}

pub fn login(tenant: Option<&str>) -> Result<CachedAccount> {
    let tenant = tenant.unwrap_or(COMMON_TENANT);
    let client = reqwest::blocking::Client::new();

    let resp = client
        .post(device_code_endpoint(tenant))
        .form(&[
            ("client_id", AZURE_CLI_CLIENT_ID),
            ("scope", MANAGEMENT_SCOPE),
        ])
        .send()
        .context("Device code request failed")?;

    if !resp.status().is_success() {
        let body = resp.text().unwrap_or_default();
        bail!("Device code request failed: {body}");
    }

    let dc: DeviceCodeResponse = resp
        .json()
        .context("Failed to parse device code response")?;

    if let Some(ref msg) = dc.message {
        eprintln!("{msg}");
    } else {
        eprintln!(
            "To sign in, use a web browser to open {} and enter the code {}",
            dc.verification_uri, dc.user_code
        );
    }

    let poll_interval = StdDuration::from_secs(dc.interval.unwrap_or(5));
    let deadline = Utc::now() + Duration::seconds(dc.expires_in);

    loop {
        if Utc::now() >= deadline {
            bail!("Device code expired. Please try again.");
        }

        thread::sleep(poll_interval);

        let resp = client
            .post(token_endpoint(tenant))
            .form(&[
                ("client_id", AZURE_CLI_CLIENT_ID),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("device_code", &dc.device_code),
            ])
            .send()
            .context("Token poll request failed")?;

        let status = resp.status();
        let body = resp.text().unwrap_or_default();

        if status.is_success() {
            let token_resp: OAuthTokenResponse =
                serde_json::from_str(&body).context("Failed to parse token response")?;

            let expires_in = token_resp.expires_in.unwrap_or(3600);
            let expires_at = Utc::now() + Duration::seconds(expires_in);

            let username = super::token_provider::extract_username(&token_resp.access_token)
                .unwrap_or_else(|_| "unknown".to_string());

            return Ok(CachedAccount {
                subscription_id: String::new(),
                tenant_id: tenant.to_string(),
                access_token: token_resp.access_token,
                refresh_token: token_resp.refresh_token,
                expires_at,
                username,
                auth_method: "device_code".to_string(),
                vault_access_token: None,
                vault_expires_at: None,
            });
        }

        if let Ok(err) = serde_json::from_str::<OAuthErrorResponse>(&body) {
            match err.error.as_str() {
                "authorization_pending" => continue,
                "slow_down" => {
                    thread::sleep(StdDuration::from_secs(5));
                    continue;
                }
                "expired_token" => bail!("Device code expired. Please try again."),
                _ => bail!(
                    "Device code auth failed: {}: {}",
                    err.error,
                    err.error_description.unwrap_or_default()
                ),
            }
        }

        bail!("Unexpected token response ({status}): {body}");
    }
}
