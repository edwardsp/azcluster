use crate::cluster_state::ClusterState;
use anyhow::{anyhow, Result};

const UPLOAD_JOB: &str = include_str!("manifests/azcp-upload-job.yaml");

pub(crate) fn render_upload(
    state: &ClusterState,
    dest_prefix: &str,
    scratch_gib: u32,
) -> Result<String> {
    let aks = state
        .aks
        .as_ref()
        .ok_or_else(|| anyhow!("cluster '{}' is not an AKS cluster", state.name))?;
    let client_id = aks.kubelet_identity_client_id.as_deref().ok_or_else(|| {
        anyhow!(
            "cluster '{}' did not record a kubelet identity client id; redeploy on a build that emits kubeletIdentityClientId",
            state.name
        )
    })?;
    let container_url = state.storage_data_container_url.as_deref().ok_or_else(|| {
        anyhow!(
            "cluster '{}' has no storage data container; deploy with --target aks (storage on by default)",
            state.name
        )
    })?;
    let dest = format!(
        "{}/{}",
        container_url.trim_end_matches('/'),
        dest_prefix.trim_start_matches('/')
    );
    let prep = "head -c 104857600 /dev/urandom > /scratch/upload/azcp-test.bin";
    Ok(render_upload_str(client_id, &dest, prep, scratch_gib))
}

fn render_upload_str(client_id: &str, dest_url: &str, prep_cmd: &str, scratch_gib: u32) -> String {
    UPLOAD_JOB
        .replace("{{MI_CLIENT_ID}}", client_id)
        .replace("{{BLOB_DEST_URL}}", dest_url)
        .replace("{{PREP_CMD}}", prep_cmd)
        .replace("{{CACHE_GIB}}", &scratch_gib.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upload_job_uses_acstor_and_imds_auth() {
        assert!(!UPLOAD_JOB.contains("hostPath"));
        assert!(UPLOAD_JOB.contains("storageClassName: local-csi"));
        assert!(UPLOAD_JOB.contains("localdisk.csi.acstor.io/accept-ephemeral-storage"));
        assert!(UPLOAD_JOB.contains("azcp copy"));
        assert!(UPLOAD_JOB.contains("AZURE_CLIENT_ID"));
        assert!(UPLOAD_JOB.contains("ghcr.io/edwardsp/azcp/azcp-cluster:v0.4.3"));
    }

    #[test]
    fn render_fills_every_token() {
        let out = render_upload_str(
            "11111111-2222-3333-4444-555555555555",
            "https://stazcabc.blob.core.windows.net/data/checkpoints/run1",
            "true",
            128,
        );
        assert!(!out.contains("{{"));
        assert!(out.contains("11111111-2222-3333-4444-555555555555"));
        assert!(out.contains("https://stazcabc.blob.core.windows.net/data/checkpoints/run1"));
        assert!(out.contains("storage: 128Gi"));
    }
}
