use anyhow::{anyhow, Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterState {
    pub name: String,
    pub subscription_id: String,
    pub resource_group: String,
    pub location: String,
    pub admin_username: String,
    pub login_public_ip: Option<String>,
    pub scheduler_private_ip: String,
    pub anf_mount_ip: Option<String>,
    #[serde(default)]
    pub compute_vmss_names: Vec<String>,
    #[serde(default)]
    pub extra_packages: Vec<String>,
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
        };
        let ser = toml::to_string(&s).unwrap();
        let de: ClusterSecrets = toml::from_str(&ser).unwrap();
        assert_eq!(de.ldap_admin_password, "ldap-pw");
        assert_eq!(de.mysql_admin_password.as_deref(), Some("mysql-pw"));
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
        };
        let ser = toml::to_string(&p).unwrap();
        let de: PendingDeploy = toml::from_str(&ser).unwrap();
        assert_eq!(de.deployment_name, p.deployment_name);
        assert_eq!(de.resource_group, p.resource_group);
        assert_eq!(de.grafana_location.as_deref(), Some("uksouth"));
        assert!(!de.accounting_enabled);
        assert_eq!(de.extra_packages, vec!["git-lfs", "python3.12-venv"]);
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
