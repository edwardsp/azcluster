use crate::cluster_state::ClusterState;
use anyhow::{anyhow, Result};

pub(crate) fn status_aks(state: &ClusterState) -> Result<()> {
    let aks = state
        .aks
        .as_ref()
        .ok_or_else(|| anyhow!("cluster '{}' is not an AKS cluster", state.name))?;
    println!("name:              {}", state.name);
    println!("resource group:    {}", state.resource_group);
    println!("location:          {}", state.location);
    println!("target:            aks");
    println!("aks cluster:       {}", aks.aks_cluster_name);
    println!("node rg:           {}", aks.node_resource_group);
    println!(
        "api fqdn:          {}",
        aks.fqdn.as_deref().unwrap_or("<none>")
    );
    println!(
        "gpu pool:          {} ({} x {})",
        aks.gpu_pool_name, aks.gpu_node_count, aks.gpu_sku
    );
    println!(
        "operator stages:   {}",
        if aks.stages_completed.is_empty() {
            "<none recorded>".to_string()
        } else {
            aks.stages_completed.join(", ")
        }
    );
    if state.storage_enabled {
        println!(
            "storage account:   {}",
            state.storage_account_name.as_deref().unwrap_or("<none>")
        );
        println!(
            "data container:    {}",
            state
                .storage_data_container_url
                .as_deref()
                .unwrap_or("<none>")
        );
        println!(
            "kubelet client id: {}",
            aks.kubelet_identity_client_id
                .as_deref()
                .unwrap_or("<none>")
        );
    }

    println!("live health (runCommand, best-effort):");
    let arm = crate::arm_client()?;
    match arm.aks_run_command(
        &state.resource_group,
        &aks.aks_cluster_name,
        HEALTH_SCRIPT,
        None,
    ) {
        Ok(r) => {
            for line in r.logs.lines() {
                println!("  {line}");
            }
            if r.exit_code != 0 {
                println!("  (probe exit_code={})", r.exit_code);
            }
        }
        Err(e) => println!("  SKIP (runCommand failed: {e})"),
    }
    Ok(())
}

const HEALTH_SCRIPT: &str = r#"set +e
echo '== gpu nodes =='
kubectl get nodes -l agentpool=gpu -o custom-columns=NAME:.metadata.name,READY:.status.conditions[-1].type,GPU:.status.allocatable.nvidia\.com/gpu --no-headers 2>/dev/null
echo '== operator daemonsets (ready/desired) =='
for ns in gpu-operator nvidia-network-operator cert-manager; do
  kubectl -n "$ns" get ds --no-headers 2>/dev/null | awk -v ns="$ns" '{print ns"/"$1": "$4"/"$2}'
done
echo '== kueue =='
kubectl -n kueue-system get deploy kueue-controller-manager --no-headers 2>/dev/null | awk '{print "kueue-controller-manager: "$2}'
kubectl get clusterqueue --no-headers 2>/dev/null | awk '{print "clusterqueue/"$1}'
exit 0
"#;
