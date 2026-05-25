//! ARM client configuration with support for custom API versions.
//!
//! Allows loading API versions from configuration files or environment variables.

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// ARM API version configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiVersionConfig {
    /// Resource group API version (default: 2024-03-01)
    #[serde(default = "default_resource_group_version")]
    pub resource_group: String,
    /// Deployment API version (default: 2024-03-01)
    #[serde(default = "default_deployment_version")]
    pub deployment: String,
    /// Compute API version (default: 2024-07-01)
    #[serde(default = "default_compute_version")]
    pub compute: String,
    /// Network API version (default: 2023-11-01)
    #[serde(default = "default_network_version")]
    pub network: String,
    /// Storage API version (default: 2023-05-01)
    #[serde(default = "default_storage_version")]
    pub storage: String,
}

fn default_resource_group_version() -> String {
    "2024-03-01".to_string()
}

fn default_deployment_version() -> String {
    "2024-03-01".to_string()
}

fn default_compute_version() -> String {
    "2024-07-01".to_string()
}

fn default_network_version() -> String {
    "2023-11-01".to_string()
}

fn default_storage_version() -> String {
    "2023-05-01".to_string()
}

impl Default for ApiVersionConfig {
    fn default() -> Self {
        Self {
            resource_group: default_resource_group_version(),
            deployment: default_deployment_version(),
            compute: default_compute_version(),
            network: default_network_version(),
            storage: default_storage_version(),
        }
    }
}

impl ApiVersionConfig {
    /// Load configuration from a TOML file.
    pub fn from_file(path: &PathBuf) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file: {}", path.display()))?;
        toml::from_str(&content).context("Failed to parse TOML config")
    }

    /// Load configuration from environment variables.
    /// Looks for: ARM_API_VERSION_RESOURCE_GROUP, ARM_API_VERSION_DEPLOYMENT, etc.
    pub fn from_env() -> Self {
        let mut config = Self::default();

        if let Ok(v) = std::env::var("ARM_API_VERSION_RESOURCE_GROUP") {
            config.resource_group = v;
        }
        if let Ok(v) = std::env::var("ARM_API_VERSION_DEPLOYMENT") {
            config.deployment = v;
        }
        if let Ok(v) = std::env::var("ARM_API_VERSION_COMPUTE") {
            config.compute = v;
        }
        if let Ok(v) = std::env::var("ARM_API_VERSION_NETWORK") {
            config.network = v;
        }
        if let Ok(v) = std::env::var("ARM_API_VERSION_STORAGE") {
            config.storage = v;
        }

        config
    }

    /// Load configuration from file if it exists, otherwise use environment variables.
    pub fn load(config_path: Option<&PathBuf>) -> Result<Self> {
        if let Some(path) = config_path {
            if path.exists() {
                return Self::from_file(path);
            }
        }
        Ok(Self::from_env())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_api_version_config_default() {
        let config = ApiVersionConfig::default();
        assert_eq!(config.resource_group, "2024-03-01");
        assert_eq!(config.deployment, "2024-03-01");
        assert_eq!(config.compute, "2024-07-01");
        assert_eq!(config.network, "2023-11-01");
        assert_eq!(config.storage, "2023-05-01");
    }

    #[test]
    fn test_api_version_config_from_env() {
        // Set environment variables
        std::env::set_var("ARM_API_VERSION_RESOURCE_GROUP", "2025-01-01");
        std::env::set_var("ARM_API_VERSION_DEPLOYMENT", "2025-02-01");

        let config = ApiVersionConfig::from_env();
        assert_eq!(config.resource_group, "2025-01-01");
        assert_eq!(config.deployment, "2025-02-01");
        // Others should use defaults
        assert_eq!(config.compute, "2024-07-01");

        // Clean up
        std::env::remove_var("ARM_API_VERSION_RESOURCE_GROUP");
        std::env::remove_var("ARM_API_VERSION_DEPLOYMENT");
    }

    #[test]
    fn test_api_version_config_partial_env() {
        // Only set one variable
        std::env::set_var("ARM_API_VERSION_COMPUTE", "2025-03-01");

        let config = ApiVersionConfig::from_env();
        assert_eq!(config.compute, "2025-03-01");
        // Others should use defaults
        assert_eq!(config.resource_group, "2024-03-01");
        assert_eq!(config.deployment, "2024-03-01");

        // Clean up
        std::env::remove_var("ARM_API_VERSION_COMPUTE");
    }

    #[test]
    fn test_api_version_config_toml_serialization() {
        let config = ApiVersionConfig {
            resource_group: "2025-01-01".to_string(),
            deployment: "2025-02-01".to_string(),
            compute: "2025-03-01".to_string(),
            network: "2025-04-01".to_string(),
            storage: "2025-05-01".to_string(),
        };

        let toml_str = toml::to_string(&config).unwrap();
        assert!(toml_str.contains("2025-01-01"));
        assert!(toml_str.contains("2025-02-01"));

        let parsed: ApiVersionConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.resource_group, "2025-01-01");
        assert_eq!(parsed.deployment, "2025-02-01");
    }
}
