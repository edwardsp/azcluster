use anyhow::{bail, Context, Result};
use serde::Deserialize;

#[derive(Debug, Deserialize, Clone)]
pub struct BastionTokenResponse {
    #[serde(rename = "authToken")]
    pub auth_token: String,
    #[serde(rename = "nodeId", default)]
    pub node_id: Option<String>,
}

pub struct BastionClient {
    aad_access_token: String,
    http: reqwest::Client,
}

impl BastionClient {
    pub fn new(aad_access_token: String) -> Self {
        Self {
            aad_access_token,
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .expect("build reqwest client"),
        }
    }

    pub async fn get_tunnel_token(
        &self,
        bastion_dns: &str,
        target_resource_id: &str,
        target_port: u16,
        last_token: Option<&str>,
        node_id: Option<&str>,
    ) -> Result<BastionTokenResponse> {
        let url = format!("https://{}/api/tokens", bastion_dns);
        let mut form: Vec<(String, String)> = vec![
            ("resourceId".into(), target_resource_id.into()),
            ("protocol".into(), "tcptunnel".into()),
            ("workloadHostPort".into(), target_port.to_string()),
            ("aztoken".into(), self.aad_access_token.clone()),
        ];
        if let Some(t) = last_token {
            form.push(("token".into(), t.into()));
        }
        let mut req = self.http.post(&url).form(&form);
        if let Some(nid) = node_id {
            req = req.header("X-Node-Id", nid);
        }
        let resp = req.send().await.context("POST /api/tokens")?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("Bastion get_tunnel_token failed ({status}): {body}");
        }
        resp.json()
            .await
            .context("parse Bastion token response JSON")
    }

    pub async fn delete_tunnel_token(
        &self,
        bastion_dns: &str,
        token: &str,
        node_id: Option<&str>,
    ) -> Result<()> {
        let url = format!("https://{}/api/tokens/{}", bastion_dns, token);
        let mut req = self.http.delete(&url);
        if let Some(nid) = node_id {
            req = req.header("X-Node-Id", nid);
        }
        let resp = req.send().await.context("DELETE /api/tokens")?;
        match resp.status().as_u16() {
            200 | 204 | 404 => Ok(()),
            code => {
                let body = resp.text().await.unwrap_or_default();
                bail!("Bastion delete_tunnel_token failed ({code}): {body}");
            }
        }
    }
}
