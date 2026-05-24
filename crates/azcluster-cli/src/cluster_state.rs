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
}
