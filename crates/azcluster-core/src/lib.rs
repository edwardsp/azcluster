use serde::{Deserialize, Serialize};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cluster {
    pub name: String,
    pub region: String,
    pub scheduler: NodeSpec,
    pub login: Vec<NodeSpec>,
    /// v1 always synthesises exactly one element (the GPU pool from
    /// `--gpu-sku/--gpu-count`). Bicep templates and slurm.conf rendering
    /// iterate this list from day 1 so future multi-pool support (CPU pool,
    /// interactive pool, etc.) is not a breaking change. Do not collapse to
    /// a single `NodePool` field.
    pub pools: Vec<NodePool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeSpec {
    pub sku: String,
    pub image: ImageSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodePool {
    /// Slurm partition name.
    pub name: String,
    pub sku: String,
    pub count: u32,
    pub role: PoolRole,
    pub image: ImageSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PoolRole {
    Compute { gpus_per_node: Option<u32> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageSpec {
    pub publisher: String,
    pub offer: String,
    pub sku: String,
    pub version: String,
}

impl ImageSpec {
    /// Default base image for v1: Ubuntu HPC 24.04 marketplace image.
    pub fn ubuntu_hpc_2404() -> Self {
        Self {
            publisher: "microsoft-dsvm".to_string(),
            offer: "ubuntu-hpc".to_string(),
            sku: "2404".to_string(),
            version: "latest".to_string(),
        }
    }

    pub fn ubuntu_hpc_2204() -> Self {
        Self {
            publisher: "microsoft-dsvm".to_string(),
            offer: "ubuntu-hpc".to_string(),
            sku: "2204".to_string(),
            version: "latest".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_populated() {
        assert!(!VERSION.is_empty());
    }

    #[test]
    fn ubuntu_hpc_defaults_are_consistent() {
        let img = ImageSpec::ubuntu_hpc_2404();
        assert_eq!(img.publisher, "microsoft-dsvm");
        assert_eq!(img.offer, "ubuntu-hpc");
        assert_eq!(img.sku, "2404");
    }
}
