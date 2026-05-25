use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;

use anyhow::{bail, Context, Result};
use base64::Engine;
use chrono::{Duration, Utc};
use rand::RngCore;
use sha2::{Digest, Sha256};

use super::cache::CachedAccount;
use super::{
    authorize_endpoint, token_endpoint, OAuthErrorResponse, OAuthTokenResponse,
    AZURE_CLI_CLIENT_ID, COMMON_TENANT, MANAGEMENT_SCOPE,
};

pub fn login(tenant: Option<&str>) -> Result<CachedAccount> {
    let tenant = tenant.unwrap_or(COMMON_TENANT);
    let (code_verifier, code_challenge) = generate_pkce();

    let listener =
        TcpListener::bind("127.0.0.1:0").context("Failed to bind localhost for redirect")?;
    let port = listener.local_addr()?.port();
    let redirect_uri = format!("http://localhost:{port}");

    let state = uuid::Uuid::new_v4().to_string();

    let mut auth_url = url::Url::parse(&authorize_endpoint(tenant))
        .context("Failed to parse authorize endpoint URL")?;
    auth_url
        .query_pairs_mut()
        .append_pair("client_id", AZURE_CLI_CLIENT_ID)
        .append_pair("response_type", "code")
        .append_pair("redirect_uri", &redirect_uri)
        .append_pair("scope", MANAGEMENT_SCOPE)
        .append_pair("state", &state)
        .append_pair("code_challenge", &code_challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("prompt", "select_account");
    let auth_url = auth_url.to_string();

    eprintln!("Opening browser for Azure login...");
    eprintln!("If the browser does not open, visit:\n{auth_url}");
    let _ = webbrowser::open(&auth_url);

    let (mut stream, _) = listener
        .accept()
        .context("Failed to accept redirect connection")?;
    let mut request_line = String::new();
    {
        let mut reader = BufReader::new(&stream);
        reader.read_line(&mut request_line)?;
    }

    let path = request_line
        .split_whitespace()
        .nth(1)
        .context("Invalid HTTP request from redirect")?;

    let full_url = format!("http://localhost:{port}{path}");
    let parsed = url::Url::parse(&full_url).context("Failed to parse redirect URL")?;

    let mut code = None;
    let mut returned_state = None;
    let mut error = None;
    let mut error_description = None;

    for (key, value) in parsed.query_pairs() {
        match key.as_ref() {
            "code" => code = Some(value.to_string()),
            "state" => returned_state = Some(value.to_string()),
            "error" => error = Some(value.to_string()),
            "error_description" => error_description = Some(value.to_string()),
            _ => {}
        }
    }

    let response_html = if error.is_some() {
        "<html><body><h2>azcluster login failed.</h2><p>You can close this window.</p></body></html>"
    } else {
        "<html><body><h2>azcluster login successful.</h2><p>You can close this window.</p></body></html>"
    };
    let http_response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        response_html.len(),
        response_html
    );
    let _ = stream.write_all(http_response.as_bytes());
    let _ = stream.flush();

    if let Some(err) = error {
        let desc = error_description.unwrap_or_default();
        bail!("Authentication failed: {err}: {desc}");
    }

    let code = code.context("No authorization code received")?;

    if returned_state.as_deref() != Some(&state) {
        bail!("OAuth state mismatch - possible CSRF attack");
    }

    let client = reqwest::blocking::Client::new();
    let resp = client
        .post(token_endpoint(tenant))
        .form(&[
            ("client_id", AZURE_CLI_CLIENT_ID),
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("redirect_uri", &redirect_uri),
            ("code_verifier", &code_verifier),
            ("scope", MANAGEMENT_SCOPE),
        ])
        .send()
        .context("Token exchange request failed")?;

    if !resp.status().is_success() {
        let body = resp.text().unwrap_or_default();
        if let Ok(err) = serde_json::from_str::<OAuthErrorResponse>(&body) {
            bail!(
                "Token exchange failed: {}: {}",
                err.error,
                err.error_description.unwrap_or_default()
            );
        }
        bail!("Token exchange failed: {body}");
    }

    let token_resp: OAuthTokenResponse = resp.json().context("Failed to parse token response")?;

    let expires_in = token_resp.expires_in.unwrap_or(3600);
    let expires_at = Utc::now() + Duration::seconds(expires_in);

    let username = super::token_provider::extract_username(&token_resp.access_token)
        .unwrap_or_else(|_| "unknown".to_string());

    Ok(CachedAccount {
        subscription_id: String::new(),
        tenant_id: tenant.to_string(),
        access_token: token_resp.access_token,
        refresh_token: token_resp.refresh_token,
        expires_at,
        username,
        auth_method: "interactive".to_string(),
    })
}

fn generate_pkce() -> (String, String) {
    let mut verifier_bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut verifier_bytes);
    let verifier = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(verifier_bytes);

    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hasher.finalize());

    (verifier, challenge)
}

#[cfg(test)]
mod tests {
    use super::generate_pkce;
    use base64::Engine;

    #[test]
    fn pkce_verifier_and_challenge_are_url_safe_no_pad() {
        let (v, c) = generate_pkce();
        assert!(!v.is_empty());
        assert!(!c.is_empty());
        assert!(!v.contains('='));
        assert!(!c.contains('='));
        assert!(!v.contains('+'));
        assert!(!c.contains('/'));
        let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(c.as_bytes())
            .unwrap();
        assert_eq!(decoded.len(), 32, "SHA-256 digest must be 32 bytes");
    }

    #[test]
    fn pkce_verifier_changes_per_call() {
        let (v1, _) = generate_pkce();
        let (v2, _) = generate_pkce();
        assert_ne!(v1, v2);
    }
}
