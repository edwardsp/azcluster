use crate::arm::client::ArmClient;
use crate::cluster_state::{state_path, ClusterState};
use crate::keyvault::{vault_uri, KeyVaultClient};
use anyhow::{anyhow, bail, Context, Result};
use std::path::Path;
use std::time::{Duration, SystemTime};

pub const TAG_MANAGED: &str = "azcluster:managed";
pub const TAG_NAME: &str = "azcluster:name";
pub const TAG_KV: &str = "azcluster:kv";
pub const TAG_VERSION: &str = "azcluster:version";
pub const TAG_DEPLOYED_AT: &str = "azcluster:deployed-at";

pub const MANIFEST_SECRET: &str = "cluster-manifest";
pub const SECRETS_BUNDLE: &str = "secrets-bundle";

const CACHE_STALE_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolveSource {
    Cache,
    KeyVault,
}

pub struct ResolvedCluster {
    pub state: ClusterState,
    pub source: ResolveSource,
}

pub struct Resolver<'a> {
    arm: &'a ArmClient,
    vault_token: String,
    no_cache: bool,
}

impl<'a> Resolver<'a> {
    pub fn new(arm: &'a ArmClient, vault_token: String, no_cache: bool) -> Self {
        Self {
            arm,
            vault_token,
            no_cache,
        }
    }

    pub fn resolve(&self, name: &str) -> Result<ResolvedCluster> {
        if !self.no_cache {
            if let Some(state) = load_fresh_cache(name)? {
                return Ok(ResolvedCluster {
                    state,
                    source: ResolveSource::Cache,
                });
            }
        }

        let rg = self.find_rg_for_cluster(name)?;
        let kv_name = rg
            .tags
            .get(TAG_KV)
            .ok_or_else(|| anyhow!("RG {} missing tag {TAG_KV}", rg.name))?
            .clone();
        let manifest = self.fetch_manifest(&kv_name)?;
        let state: ClusterState =
            serde_json::from_str(&manifest).context("Failed to parse cluster-manifest JSON")?;

        if state.name != name {
            bail!(
                "manifest name mismatch: tag points to '{name}' but KV manifest says '{}'",
                state.name
            );
        }

        write_cache(&state)?;
        Ok(ResolvedCluster {
            state,
            source: ResolveSource::KeyVault,
        })
    }

    fn find_rg_for_cluster(&self, name: &str) -> Result<TaggedRg> {
        let rgs = self
            .arm
            .list_resource_groups_by_tag(TAG_NAME, Some(name))
            .context("Failed to list resource groups by tag")?;
        match rgs.len() {
            0 => bail!(
                "no resource group found with tag {TAG_NAME}={name} in subscription {}",
                self.arm.subscription_id()
            ),
            1 => {
                let rg = &rgs[0];
                let rg_name = rg
                    .get("name")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow!("RG response missing 'name'"))?
                    .to_string();
                let tags = self.arm.get_resource_group_tags(&rg_name)?;
                Ok(TaggedRg { name: rg_name, tags })
            }
            n => bail!(
                "ambiguous: {n} resource groups in sub {} have tag {TAG_NAME}={name}; expected exactly 1",
                self.arm.subscription_id()
            ),
        }
    }

    fn fetch_manifest(&self, kv_name: &str) -> Result<String> {
        let kv = KeyVaultClient::new(vault_uri(kv_name), self.vault_token.clone())?;
        let bundle = kv
            .get_secret(MANIFEST_SECRET)
            .with_context(|| format!("fetching {MANIFEST_SECRET} from vault {kv_name}"))?;
        Ok(bundle.value)
    }
}

struct TaggedRg {
    name: String,
    tags: std::collections::HashMap<String, String>,
}

fn load_fresh_cache(name: &str) -> Result<Option<ClusterState>> {
    let path = state_path(name)?;
    if !path.exists() {
        return Ok(None);
    }
    if is_cache_stale(&path)? {
        return Ok(None);
    }
    let state = ClusterState::load(name)?;
    Ok(Some(state))
}

fn is_cache_stale(path: &Path) -> Result<bool> {
    let meta = std::fs::metadata(path)?;
    let mtime = meta.modified()?;
    let age = SystemTime::now()
        .duration_since(mtime)
        .unwrap_or(Duration::ZERO);
    Ok(is_stale_age(age))
}

fn is_stale_age(age: Duration) -> bool {
    age > CACHE_STALE_AFTER
}

fn write_cache(state: &ClusterState) -> Result<()> {
    state.save()?;
    Ok(())
}

pub fn purge_cache(name: Option<&str>) -> Result<usize> {
    let dir = state_path("_dummy")?
        .parent()
        .ok_or_else(|| anyhow!("no cache dir"))?
        .to_path_buf();
    if !dir.exists() {
        return Ok(0);
    }
    let mut removed = 0;
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let p = entry.path();
        let Some(stem) = p.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let Some(ext) = p.extension().and_then(|e| e.to_str()) else {
            continue;
        };
        if ext != "toml" {
            continue;
        }
        if stem.ends_with("-pending") || stem.ends_with("-secrets") {
            continue;
        }
        if let Some(n) = name {
            if stem != n {
                continue;
            }
        }
        std::fs::remove_file(&p)?;
        removed += 1;
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_at_25h() {
        assert!(is_stale_age(Duration::from_secs(25 * 60 * 60)));
    }

    #[test]
    fn fresh_at_23h() {
        assert!(!is_stale_age(Duration::from_secs(23 * 60 * 60)));
    }

    #[test]
    fn boundary_at_exactly_24h_is_fresh() {
        assert!(!is_stale_age(CACHE_STALE_AFTER));
    }

    #[test]
    fn fresh_at_zero() {
        assert!(!is_stale_age(Duration::ZERO));
    }
}
