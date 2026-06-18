use anyhow::{anyhow, Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Deployment backend a cluster was provisioned with. Added in the AKS-target work;
/// pre-existing Slurm manifests have no `target` field and deserialize as `Slurm`
/// via `#[serde(default)]`, keeping their on-disk representation byte-identical.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Target {
    #[default]
    Slurm,
    Aks,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AksState {
    pub aks_cluster_name: String,
    /// Azure-managed node resource group (`MC_<rg>_<cluster>_<region>`).
    pub node_resource_group: String,
    #[serde(default)]
    pub fqdn: Option<String>,
    /// AKS agentpool name: lowercase alnum, <= 12 chars.
    pub gpu_pool_name: String,
    pub gpu_sku: String,
    pub gpu_node_count: u32,
    #[serde(default)]
    pub kubelet_identity_object_id: Option<String>,
    #[serde(default)]
    pub kubelet_identity_client_id: Option<String>,
    #[serde(default)]
    pub oidc_issuer_url: Option<String>,
    #[serde(default)]
    pub monitoring_enabled: bool,
    /// Operator stages already applied, for idempotent `resume`.
    /// Ids include `cert-manager`, `network-operator`, `gpu-operator`,
    /// `prometheus-servicemonitor`, `kueue`, `mpi-operator`, `acstor`.
    #[serde(default)]
    pub stages_completed: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterState {
    pub name: String,
    pub subscription_id: String,
    pub resource_group: String,
    pub location: String,
    #[serde(default)]
    pub target: Target,
    pub admin_username: String,
    pub login_public_ip: Option<String>,
    pub scheduler_private_ip: String,
    pub anf_mount_ip: Option<String>,
    #[serde(default)]
    pub compute_vmss_names: Vec<String>,
    #[serde(default)]
    pub extra_packages: Vec<String>,
    #[serde(default)]
    pub accounting_enabled: bool,
    #[serde(default)]
    pub bastion_enabled: bool,
    #[serde(default)]
    pub bastion_name: Option<String>,
    #[serde(default)]
    pub bastion_dns_name: Option<String>,
    #[serde(default)]
    pub bastion_resource_id: Option<String>,
    #[serde(default)]
    pub storage_enabled: bool,
    #[serde(default)]
    pub storage_account_name: Option<String>,
    #[serde(default)]
    pub storage_blob_endpoint: Option<String>,
    #[serde(default)]
    pub storage_dfs_endpoint: Option<String>,
    #[serde(default)]
    pub storage_data_container_url: Option<String>,
    #[serde(default)]
    pub storage_hns: bool,
    #[serde(default)]
    pub storage_public_access: bool,
    #[serde(default)]
    pub azcp_version: Option<String>,
    #[serde(default)]
    pub aks: Option<AksState>,
}

fn project_dirs() -> Result<ProjectDirs> {
    ProjectDirs::from("", "", "azcluster")
        .ok_or_else(|| anyhow!("cannot resolve XDG config directory"))
}

pub fn state_path(name: &str) -> Result<PathBuf> {
    Ok(project_dirs()?
        .config_dir()
        .join("clusters")
        .join(format!("{name}.toml")))
}

pub fn secrets_path(name: &str) -> Result<PathBuf> {
    Ok(project_dirs()?
        .config_dir()
        .join("clusters")
        .join(format!("{name}-secrets.toml")))
}

pub fn pending_path(name: &str) -> Result<PathBuf> {
    Ok(project_dirs()?
        .config_dir()
        .join("clusters")
        .join(format!("{name}-pending.toml")))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingDeploy {
    pub cluster: String,
    pub deployment_name: String,
    pub resource_group: String,
    pub started_at: String,
    pub monitoring_enabled: bool,
    pub accounting_enabled: bool,
    pub shared_storage: String,
    #[serde(default)]
    pub grafana_location: Option<String>,
    #[serde(default)]
    pub extra_packages: Vec<String>,
    #[serde(default)]
    pub bastion_enabled: bool,
    #[serde(default)]
    pub storage_enabled: bool,
    #[serde(default)]
    pub storage_account_name: Option<String>,
    #[serde(default)]
    pub storage_hns: bool,
    #[serde(default)]
    pub storage_public_access: bool,
    #[serde(default)]
    pub azcp_version: Option<String>,
}

impl PendingDeploy {
    pub fn save(&self) -> Result<PathBuf> {
        let path = pending_path(&self.cluster)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, toml::to_string_pretty(self)?)?;
        Ok(path)
    }

    pub fn load_optional(name: &str) -> Result<Option<Self>> {
        let path = pending_path(name)?;
        if !path.exists() {
            return Ok(None);
        }
        let body =
            std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        let s: Self = toml::from_str(&body).with_context(|| format!("parse {}", path.display()))?;
        Ok(Some(s))
    }

    pub fn delete(name: &str) -> Result<()> {
        let path = pending_path(name)?;
        if path.exists() {
            std::fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterSecrets {
    pub ldap_admin_password: String,
    #[serde(default)]
    pub mysql_admin_password: Option<String>,
    #[serde(default)]
    pub admin_ssh_public_key: String,
    #[serde(default)]
    pub admin_ssh_private_key: String,
}

impl ClusterSecrets {
    pub fn load(name: &str) -> Result<Self> {
        let path = secrets_path(name)?;
        let body = std::fs::read_to_string(&path)
            .with_context(|| format!("no secrets for cluster '{name}' at {}", path.display()))?;
        toml::from_str(&body).with_context(|| format!("parse {}", path.display()))
    }

    pub fn load_optional(name: &str) -> Result<Option<Self>> {
        let path = secrets_path(name)?;
        if !path.exists() {
            return Ok(None);
        }
        let body =
            std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        let s: Self = toml::from_str(&body).with_context(|| format!("parse {}", path.display()))?;
        Ok(Some(s))
    }

    pub fn save(&self, name: &str) -> Result<PathBuf> {
        let path = secrets_path(name)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, toml::to_string_pretty(self)?)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(path)
    }
}

impl ClusterState {
    pub fn load(name: &str) -> Result<Self> {
        let path = state_path(name)?;
        let body = std::fs::read_to_string(&path)
            .with_context(|| format!("no state for cluster '{name}' at {}", path.display()))?;
        toml::from_str(&body).with_context(|| format!("parse {}", path.display()))
    }

    pub fn save(&self) -> Result<PathBuf> {
        let path = state_path(&self.name)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, toml::to_string_pretty(self)?)?;
        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cluster_state_without_target_defaults_to_slurm() {
        let body = r#"
name = "demo"
subscription_id = "00000000-0000-0000-0000-000000000000"
resource_group = "rg-azcluster-demo"
location = "eastus"
admin_username = "azureuser"
scheduler_private_ip = "10.42.1.4"
"#;
        let s: ClusterState = toml::from_str(body).unwrap();
        assert_eq!(s.target, Target::Slurm);
        assert!(s.aks.is_none());
        assert_eq!(s.name, "demo");
    }

    #[test]
    fn cluster_state_aks_target_round_trips() {
        let body = r#"
name = "demo"
subscription_id = "00000000-0000-0000-0000-000000000000"
resource_group = "rg-azcluster-demo"
location = "mexicocentral"
admin_username = "azureuser"
scheduler_private_ip = ""
"#;
        let mut s: ClusterState = toml::from_str(body).unwrap();
        s.target = Target::Aks;
        s.aks = Some(AksState {
            aks_cluster_name: "demo".into(),
            node_resource_group: "MC_rg-azcluster-demo_demo_mexicocentral".into(),
            fqdn: Some("demo-xyz.hcp.mexicocentral.azmk8s.io".into()),
            gpu_pool_name: "gpu".into(),
            gpu_sku: "Standard_ND96isr_H200_v5".into(),
            gpu_node_count: 2,
            kubelet_identity_object_id: None,
            kubelet_identity_client_id: None,
            oidc_issuer_url: None,
            monitoring_enabled: true,
            stages_completed: vec!["cert-manager".into(), "gpu-operator".into()],
        });
        let ser = toml::to_string(&s).unwrap();
        assert!(ser.contains("target = \"aks\""));
        let de: ClusterState = toml::from_str(&ser).unwrap();
        assert_eq!(de.target, Target::Aks);
        let aks = de.aks.expect("aks state present");
        assert_eq!(aks.gpu_sku, "Standard_ND96isr_H200_v5");
        assert_eq!(aks.gpu_node_count, 2);
        assert_eq!(aks.stages_completed, vec!["cert-manager", "gpu-operator"]);
    }

    #[test]
    fn secrets_backward_compat_without_mysql_field() {
        let body = "ldap_admin_password = \"hunter2\"\n";
        let s: ClusterSecrets = toml::from_str(body).unwrap();
        assert_eq!(s.ldap_admin_password, "hunter2");
        assert!(s.mysql_admin_password.is_none());
    }

    #[test]
    fn secrets_round_trip_with_both_fields() {
        let s = ClusterSecrets {
            ldap_admin_password: "ldap-pw".into(),
            mysql_admin_password: Some("mysql-pw".into()),
            admin_ssh_public_key: "ssh-ed25519 AAAA test".into(),
            admin_ssh_private_key:
                "-----BEGIN OPENSSH PRIVATE KEY-----\n...\n-----END OPENSSH PRIVATE KEY-----\n"
                    .into(),
        };
        let ser = toml::to_string(&s).unwrap();
        let de: ClusterSecrets = toml::from_str(&ser).unwrap();
        assert_eq!(de.ldap_admin_password, "ldap-pw");
        assert_eq!(de.mysql_admin_password.as_deref(), Some("mysql-pw"));
        assert!(de.admin_ssh_public_key.starts_with("ssh-ed25519 "));
        assert!(de.admin_ssh_private_key.contains("OPENSSH PRIVATE KEY"));
    }

    #[test]
    fn pending_deploy_round_trip() {
        let p = PendingDeploy {
            cluster: "demo".into(),
            deployment_name: "azcluster-demo-20260524-072035".into(),
            resource_group: "rg-azcluster-demo".into(),
            started_at: "2026-05-24T07:20:35Z".into(),
            monitoring_enabled: true,
            accounting_enabled: false,
            shared_storage: "anf".into(),
            grafana_location: Some("uksouth".into()),
            extra_packages: vec!["git-lfs".into(), "python3.12-venv".into()],
            bastion_enabled: true,
            storage_enabled: true,
            storage_account_name: Some("stazc89a3f12c".into()),
            storage_hns: false,
            storage_public_access: false,
            azcp_version: Some("v0.4.5".into()),
        };
        let ser = toml::to_string(&p).unwrap();
        let de: PendingDeploy = toml::from_str(&ser).unwrap();
        assert_eq!(de.deployment_name, p.deployment_name);
        assert_eq!(de.resource_group, p.resource_group);
        assert_eq!(de.grafana_location.as_deref(), Some("uksouth"));
        assert!(!de.accounting_enabled);
        assert_eq!(de.extra_packages, vec!["git-lfs", "python3.12-venv"]);
        assert!(de.bastion_enabled);
        assert!(de.storage_enabled);
        assert_eq!(de.storage_account_name.as_deref(), Some("stazc89a3f12c"));
        assert_eq!(de.azcp_version.as_deref(), Some("v0.4.5"));
    }

    #[test]
    fn pending_deploy_backward_compat_without_extra_packages() {
        // v0.19.0 / v0.19.1 markers had no extra_packages field; must still parse.
        let body = r#"cluster = "demo"
deployment_name = "azcluster-demo-x"
resource_group = "rg-azcluster-demo"
started_at = "2026-05-24T07:20:35Z"
monitoring_enabled = true
accounting_enabled = false
shared_storage = "anf"
"#;
        let de: PendingDeploy = toml::from_str(body).unwrap();
        assert_eq!(de.cluster, "demo");
        assert!(de.extra_packages.is_empty());
    }
}
