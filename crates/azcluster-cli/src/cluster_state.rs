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
