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
    pub storage: StorageSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeSpec {
    pub sku: String,
    pub image: ImageSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodePool {
    pub slurm_partition: String,
    pub sku: String,
    pub desired_count: u32,
    pub max_count: u32,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageSpec {
    pub anf: Option<AnfSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnfSpec {
    pub size_tib: u32,
    pub service_level: AnfServiceLevel,
    pub mount_path: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub enum AnfServiceLevel {
    Standard,
    Premium,
    Ultra,
}

impl AnfServiceLevel {
    pub fn as_arm_str(&self) -> &'static str {
        match self {
            Self::Standard => "Standard",
            Self::Premium => "Premium",
            Self::Ultra => "Ultra",
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

    #[test]
    fn pool_max_count_invariant() {
        let pool = NodePool {
            slurm_partition: "gpu".into(),
            sku: "Standard_ND96isr_H200_v5".into(),
            desired_count: 0,
            max_count: 2,
            role: PoolRole::Compute { gpus_per_node: Some(8) },
            image: ImageSpec::ubuntu_hpc_2404(),
        };
        assert!(pool.desired_count <= pool.max_count);
    }

    #[test]
    fn anf_service_levels_round_trip() {
        for sl in [AnfServiceLevel::Standard, AnfServiceLevel::Premium, AnfServiceLevel::Ultra] {
            let s = serde_json::to_string(&sl).unwrap();
            let back: AnfServiceLevel = serde_json::from_str(&s).unwrap();
            assert_eq!(sl, back);
        }
    }
}
