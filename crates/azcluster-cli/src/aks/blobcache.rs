use crate::cluster_state::ClusterState;
use anyhow::{anyhow, Result};

const PROBE_MANIFEST: &str = include_str!("manifests/blobcache-probe.yaml");

pub(crate) fn render_probe(state: &ClusterState, prefix: &str, cache_gib: u32) -> Result<String> {
    let aks = state
        .aks
        .as_ref()
        .ok_or_else(|| anyhow!("cluster '{}' is not an AKS cluster", state.name))?;
    let account = state.storage_account_name.as_deref().ok_or_else(|| {
        anyhow!(
            "cluster '{}' has no storage account; deploy with --target aks (storage on by default)",
            state.name
        )
    })?;
    let client_id = aks.kubelet_identity_client_id.as_deref().ok_or_else(|| {
        anyhow!(
            "cluster '{}' did not record a kubelet identity client id; redeploy on a build that emits kubeletIdentityClientId",
            state.name
        )
    })?;
    Ok(render_probe_str(account, client_id, prefix, cache_gib))
}

fn render_probe_str(account: &str, client_id: &str, prefix: &str, cache_gib: u32) -> String {
    let cache_max_bytes = (cache_gib as u64)
        .saturating_mul(1024 * 1024 * 1024)
        .saturating_mul(85)
        / 100;
    PROBE_MANIFEST
        .replace("{{STORAGE_ACCOUNT}}", account)
        .replace("{{CONTAINER}}", "data")
        .replace("{{PREFIX}}", prefix)
        .replace("{{MI_CLIENT_ID}}", client_id)
        .replace("{{CACHE_GIB}}", &cache_gib.to_string())
        .replace("{{CACHE_MAX_BYTES}}", &cache_max_bytes.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_manifest_uses_acstor_not_hostpath() {
        assert!(!PROBE_MANIFEST.contains("hostPath"));
        assert!(PROBE_MANIFEST.contains("storageClassName: local-csi"));
        assert!(PROBE_MANIFEST.contains("localdisk.csi.acstor.io/accept-ephemeral-storage"));
        assert!(PROBE_MANIFEST.contains("mountPropagation: Bidirectional"));
        assert!(PROBE_MANIFEST.contains("mountPropagation: HostToContainer"));
        assert!(PROBE_MANIFEST.contains("privileged: true"));
        assert!(PROBE_MANIFEST.contains("ghcr.io/edwardsp/blobcache:v2.9.1"));
        assert!(PROBE_MANIFEST.contains("/hydrate"));
    }

    #[test]
    fn render_fills_every_token() {
        let out = render_probe_str(
            "stazcabc12345",
            "11111111-2222-3333-4444-555555555555",
            "m",
            64,
        );
        assert!(!out.contains("{{"));
        assert!(out.contains("account = \"stazcabc12345\""));
        assert!(out.contains("11111111-2222-3333-4444-555555555555"));
        assert!(out.contains("storage: 64Gi"));
        assert!(out.contains(&(64u64 * 1024 * 1024 * 1024 * 85 / 100).to_string()));
    }
}
