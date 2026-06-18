use anyhow::{anyhow, bail, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use serde::Deserialize;
use std::io::Cursor;
use std::sync::Arc;
use url::Url;

use crate::cluster_state::ClusterState;

#[derive(Clone)]
pub(crate) struct K8sClient {
    pub(crate) server: String,
    pub(crate) http: reqwest::blocking::Client,
    pub(crate) tls: Arc<rustls::ClientConfig>,
    pub(crate) server_host: String,
    pub(crate) server_port: u16,
}

struct KubeIdentity {
    server: String,
    ca_pem: Vec<u8>,
    client_cert_pem: Vec<u8>,
    client_key_pem: Vec<u8>,
}

#[derive(Deserialize)]
struct KubeConfig {
    clusters: Vec<NamedCluster>,
    users: Vec<NamedUser>,
}

#[derive(Deserialize)]
struct NamedCluster {
    cluster: ClusterEntry,
}

#[derive(Deserialize)]
struct ClusterEntry {
    server: String,
    #[serde(rename = "certificate-authority-data")]
    certificate_authority_data: String,
}

#[derive(Deserialize)]
struct NamedUser {
    user: UserEntry,
}

#[derive(Deserialize)]
struct UserEntry {
    #[serde(rename = "client-certificate-data")]
    client_certificate_data: String,
    #[serde(rename = "client-key-data")]
    client_key_data: String,
}

impl K8sClient {
    pub(crate) fn from_state(state: &ClusterState) -> Result<Self> {
        let aks = state
            .aks
            .as_ref()
            .ok_or_else(|| anyhow!("cluster '{}' is not an AKS cluster", state.name))?;
        let kubeconfig = crate::arm_client()?
            .list_cluster_admin_credential(&state.resource_group, &aks.aks_cluster_name)?;
        Self::from_kubeconfig(&kubeconfig)
    }

    fn from_kubeconfig(kubeconfig: &str) -> Result<Self> {
        let ident = parse_kubeconfig(kubeconfig)?;
        let mut identity_pem = ident.client_cert_pem.clone();
        if !identity_pem.ends_with(b"\n") {
            identity_pem.push(b'\n');
        }
        identity_pem.extend_from_slice(&ident.client_key_pem);

        let http = reqwest::blocking::Client::builder()
            .use_rustls_tls()
            .add_root_certificate(reqwest::Certificate::from_pem(&ident.ca_pem)?)
            .identity(reqwest::Identity::from_pem(&identity_pem)?)
            .build()
            .context("build Kubernetes HTTP client")?;

        let tls = Arc::new(build_rustls_config(
            &ident.ca_pem,
            &ident.client_cert_pem,
            &ident.client_key_pem,
        )?);
        let (server_host, server_port) = server_host_port(&ident.server)?;
        Ok(Self {
            server: ident.server,
            http,
            tls,
            server_host,
            server_port,
        })
    }

    pub(crate) fn url(&self, path: &str) -> Result<Url> {
        let base = self.server.trim_end_matches('/');
        Url::parse(&format!("{base}{path}")).with_context(|| format!("parse Kubernetes URL {path}"))
    }

    pub(crate) fn wss_url(&self, path: &str) -> Result<Url> {
        let mut u = self.url(path)?;
        u.set_scheme("wss")
            .map_err(|_| anyhow!("invalid Kubernetes server URL scheme"))?;
        Ok(u)
    }

    pub(crate) fn api_get_json(&self, path: &str) -> Result<serde_json::Value> {
        let url = self.url(path)?;
        let resp = self
            .http
            .get(url)
            .send()
            .with_context(|| format!("GET {path}"))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().unwrap_or_default();
            bail!("Kubernetes GET {path} failed ({status}): {body}");
        }
        resp.json()
            .with_context(|| format!("parse GET {path} response"))
    }

    pub(crate) fn api_post_json(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<serde_json::Value> {
        let url = self.url(path)?;
        let resp = self
            .http
            .post(url)
            .json(body)
            .send()
            .with_context(|| format!("POST {path}"))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().unwrap_or_default();
            bail!("Kubernetes POST {path} failed ({status}): {body}");
        }
        resp.json()
            .with_context(|| format!("parse POST {path} response"))
    }

    pub(crate) fn api_delete(&self, path: &str) -> Result<()> {
        let url = self.url(path)?;
        let resp = self
            .http
            .delete(url)
            .send()
            .with_context(|| format!("DELETE {path}"))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().unwrap_or_default();
            bail!("Kubernetes DELETE {path} failed ({status}): {body}");
        }
        Ok(())
    }
}

pub(crate) fn block_on<F, T>(future: F) -> Result<T>
where
    F: std::future::Future<Output = Result<T>>,
{
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("build tokio runtime for Kubernetes WebSocket")?
        .block_on(future)
}

fn parse_kubeconfig(kubeconfig: &str) -> Result<KubeIdentity> {
    let kc: KubeConfig = serde_yaml::from_str(kubeconfig).context("parse kubeconfig YAML")?;
    let cluster = kc
        .clusters
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("kubeconfig has no clusters"))?
        .cluster;
    let user = kc
        .users
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("kubeconfig has no users"))?
        .user;
    Ok(KubeIdentity {
        server: cluster.server,
        ca_pem: STANDARD
            .decode(cluster.certificate_authority_data)
            .context("decode certificate-authority-data")?,
        client_cert_pem: STANDARD
            .decode(user.client_certificate_data)
            .context("decode client-certificate-data")?,
        client_key_pem: STANDARD
            .decode(user.client_key_data)
            .context("decode client-key-data")?,
    })
}

fn build_rustls_config(
    ca_pem: &[u8],
    client_cert_pem: &[u8],
    client_key_pem: &[u8],
) -> Result<rustls::ClientConfig> {
    let mut roots = rustls::RootCertStore::empty();
    for cert in rustls_pemfile::certs(&mut Cursor::new(ca_pem)) {
        roots
            .add(cert.context("parse CA certificate PEM")?)
            .context("add Kubernetes CA certificate")?;
    }
    if roots.is_empty() {
        bail!("kubeconfig CA data contained no certificates");
    }
    let cert_chain: Vec<CertificateDer<'static>> =
        rustls_pemfile::certs(&mut Cursor::new(client_cert_pem))
            .collect::<std::result::Result<Vec<_>, _>>()
            .context("parse client certificate PEM")?;
    if cert_chain.is_empty() {
        bail!("kubeconfig client certificate data contained no certificates");
    }
    let key: PrivateKeyDer<'static> = rustls_pemfile::private_key(&mut Cursor::new(client_key_pem))
        .context("parse client private key PEM")?
        .ok_or_else(|| anyhow!("kubeconfig client key data contained no private key"))?;
    rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_client_auth_cert(cert_chain, key)
        .context("build Kubernetes WebSocket TLS config")
}

fn server_host_port(server: &str) -> Result<(String, u16)> {
    let u = Url::parse(server).with_context(|| format!("parse Kubernetes server URL {server}"))?;
    let host = u
        .host_str()
        .ok_or_else(|| anyhow!("Kubernetes server URL has no host"))?
        .to_string();
    Ok((host, u.port_or_known_default().unwrap_or(443)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kubeconfig_yaml_parsing_extracts_embedded_pems() {
        let ca = STANDARD.encode(b"-----BEGIN CERTIFICATE-----\nCA\n-----END CERTIFICATE-----\n");
        let cert =
            STANDARD.encode(b"-----BEGIN CERTIFICATE-----\nCERT\n-----END CERTIFICATE-----\n");
        let key = STANDARD.encode(b"-----BEGIN PRIVATE KEY-----\nKEY\n-----END PRIVATE KEY-----\n");
        let body = format!(
            r#"
apiVersion: v1
clusters:
- name: c
  cluster:
    server: https://demo.azmk8s.io:443
    certificate-authority-data: {ca}
users:
- name: u
  user:
    client-certificate-data: {cert}
    client-key-data: {key}
"#
        );
        let parsed = parse_kubeconfig(&body).unwrap();
        assert_eq!(parsed.server, "https://demo.azmk8s.io:443");
        assert_eq!(
            parsed.ca_pem,
            b"-----BEGIN CERTIFICATE-----\nCA\n-----END CERTIFICATE-----\n"
        );
        assert_eq!(
            parsed.client_cert_pem,
            b"-----BEGIN CERTIFICATE-----\nCERT\n-----END CERTIFICATE-----\n"
        );
        assert_eq!(
            parsed.client_key_pem,
            b"-----BEGIN PRIVATE KEY-----\nKEY\n-----END PRIVATE KEY-----\n"
        );
    }

    #[test]
    fn server_url_defaults_https_port() {
        assert_eq!(
            server_host_port("https://demo.azmk8s.io").unwrap(),
            ("demo.azmk8s.io".into(), 443)
        );
        assert_eq!(
            server_host_port("https://demo.azmk8s.io:8443").unwrap(),
            ("demo.azmk8s.io".into(), 8443)
        );
    }
}
