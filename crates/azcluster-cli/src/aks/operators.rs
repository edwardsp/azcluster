use crate::aks::single_quote;
use crate::arm::client::{ArmClient, RunCommandResult};
use anyhow::{bail, Context, Result};

const NETWORK_OPERATOR_VALUES: &str = include_str!("manifests/network-operator-values.yaml");
const GPU_OPERATOR_VALUES: &str = include_str!("manifests/gpu-operator-values.yaml");
const NFD_NETWORK_RULE: &str = include_str!("manifests/nfd-network-rule.yaml");
const NIC_CLUSTER_POLICY: &str = include_str!("manifests/nic-cluster-policy.yaml");
const KUEUE_VALUES: &str = include_str!("manifests/kueue-values.yaml");
const KUEUE_QUEUES: &str = include_str!("manifests/kueue-queues.yaml");

pub const STAGES: &[&str] = &[
    "gpu-nodes-ready",
    "cert-manager",
    "network-operator",
    "gpu-operator",
    "kueue",
    "mpi-operator",
];

pub fn install_all(
    arm: &ArmClient,
    resource_group: &str,
    cluster: &str,
    gpu_node_count: u32,
) -> Result<Vec<String>> {
    let mut completed = Vec::with_capacity(STAGES.len());
    run_stage(
        arm,
        resource_group,
        cluster,
        "0",
        "waiting for GPU nodes",
        &gpu_nodes_ready_script(),
    )?;
    completed.push("gpu-nodes-ready".to_string());
    run_stage(
        arm,
        resource_group,
        cluster,
        "1",
        "cert-manager v1.18.2",
        &cert_manager_script(),
    )?;
    completed.push("cert-manager".to_string());
    run_stage(
        arm,
        resource_group,
        cluster,
        "2",
        "NVIDIA Network Operator v26.1.0",
        &network_operator_script(),
    )?;
    completed.push("network-operator".to_string());
    run_stage(
        arm,
        resource_group,
        cluster,
        "3",
        "NVIDIA GPU Operator v26.3.0",
        &gpu_operator_script(),
    )?;
    completed.push("gpu-operator".to_string());
    run_stage(
        arm,
        resource_group,
        cluster,
        "4",
        "Kueue v0.13.0",
        &kueue_script(gpu_node_count),
    )?;
    completed.push("kueue".to_string());
    run_stage(
        arm,
        resource_group,
        cluster,
        "5",
        "MPI Operator v0.6.0",
        mpi_operator_script(),
    )?;
    completed.push("mpi-operator".to_string());
    Ok(completed)
}

fn run_stage(
    arm: &ArmClient,
    resource_group: &str,
    cluster: &str,
    stage: &str,
    label: &str,
    script: &str,
) -> Result<()> {
    eprintln!("==> [aks] stage {stage}: {label}");
    let result = arm
        .aks_run_command(resource_group, cluster, script, None)
        .with_context(|| format!("AKS runCommand stage {stage} ({label})"))?;
    ensure_run_command_success(stage, label, result)
}

fn ensure_run_command_success(stage: &str, label: &str, result: RunCommandResult) -> Result<()> {
    if result.exit_code == 0 {
        return Ok(());
    }
    bail!(
        "AKS operator stage {stage} ({label}) failed: provisioning_state={}, exit_code={}\n{}",
        result.provisioning_state,
        result.exit_code,
        result.logs
    )
}

fn gpu_nodes_ready_script() -> String {
    script_with_body("kubectl wait --for=condition=Ready node -l agentpool=gpu --timeout=900s")
}

fn cert_manager_script() -> String {
    script_with_body(
        r#"helm repo add jetstack https://charts.jetstack.io --force-update
helm repo update jetstack
helm upgrade --install cert-manager jetstack/cert-manager \
  -n cert-manager --create-namespace \
  --version v1.18.2 \
  --set crds.enabled=true
kubectl -n cert-manager wait --for=condition=Available deploy --all --timeout=300s"#,
    )
}

fn network_operator_script() -> String {
    let nfd = single_quote(NFD_NETWORK_RULE);
    let nic = single_quote(NIC_CLUSTER_POLICY);
    script_with_body(&format!(
        r#"cat > /tmp/network-operator-values.yaml <<'EOF'
{values_body}
EOF
helm repo add nvidia https://helm.ngc.nvidia.com/nvidia --force-update
helm repo update nvidia
helm upgrade --install network-operator nvidia/network-operator \
  --wait --create-namespace -n nvidia-network-operator \
  --values /tmp/network-operator-values.yaml \
  --version v26.1.0
printf %s {nfd} | kubectl apply --server-side -f -
printf %s {nic} | kubectl apply --server-side -f -
for i in $(seq 1 180); do
  state=$(kubectl get NicClusterPolicy nic-cluster-policy -o jsonpath='{{.status.state}}' 2>/dev/null || true)
  [ "$state" = ready ] && exit 0
  echo "waiting for NicClusterPolicy ready (state=${{state:-missing}})"
  sleep 10
done
kubectl describe NicClusterPolicy nic-cluster-policy || true
exit 1"#,
        values_body = NETWORK_OPERATOR_VALUES,
        nfd = nfd,
        nic = nic,
    ))
}

fn gpu_operator_script() -> String {
    script_with_body(&format!(
        r#"cat > /tmp/gpu-operator-values.yaml <<'EOF'
{values_body}
EOF
helm repo add nvidia https://helm.ngc.nvidia.com/nvidia --force-update
helm repo update nvidia
helm upgrade --install gpu-operator nvidia/gpu-operator \
  --wait -n gpu-operator --create-namespace \
  --values /tmp/gpu-operator-values.yaml \
  --version v26.3.0
for i in $(seq 1 180); do
  ready=$(kubectl get nodes -l agentpool=gpu -o jsonpath='{{range .items[*]}}{{.status.allocatable.nvidia\.com/gpu}}{{"\n"}}{{end}}' \
    | awk 'BEGIN {{ ok=0 }} $1+0 >= 8 {{ ok++ }} END {{ print ok }}')
  total=$(kubectl get nodes -l agentpool=gpu --no-headers 2>/dev/null | wc -l)
  if [ "$total" -gt 0 ] && [ "$ready" -eq "$total" ]; then
    exit 0
  fi
  echo "waiting for nvidia.com/gpu allocatable on GPU nodes ($ready/$total)"
  sleep 10
done
kubectl get nodes -l agentpool=gpu -o wide || true
exit 1"#,
        values_body = GPU_OPERATOR_VALUES,
    ))
}

fn kueue_script(gpu_node_count: u32) -> String {
    let quota = gpu_node_count.saturating_mul(8).to_string();
    let queues = KUEUE_QUEUES.replace("{{GPU_QUOTA}}", &quota);
    let queues_q = single_quote(&queues);
    script_with_body(&format!(
        r#"cat > /tmp/kueue-values.yaml <<'EOF'
{values_body}
EOF
helm upgrade --install kueue oci://registry.k8s.io/kueue/charts/kueue \
  --wait --create-namespace --namespace kueue-system \
  --values /tmp/kueue-values.yaml \
  --version 0.13.0
printf %s {queues} | kubectl apply --server-side -f -
kubectl -n kueue-system wait --for=condition=Available deploy/kueue-controller-manager --timeout=300s"#,
        values_body = KUEUE_VALUES,
        queues = queues_q,
    ))
}

fn mpi_operator_script() -> &'static str {
    r#"set -eu
kubectl apply --server-side -f https://raw.githubusercontent.com/kubeflow/mpi-operator/v0.6.0/deploy/v2beta1/mpi-operator.yaml
kubectl -n mpi-operator wait --for=condition=Available deploy --all --timeout=300s
"#
}

fn script_with_body(body: &str) -> String {
    format!("set -eu\n{body}\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kueue_quota_is_nodes_times_eight() {
        let s = kueue_script(2);
        assert!(s.contains("nominalQuota: \"16\""));
        assert!(!s.contains("{{GPU_QUOTA}}"));
    }

    #[test]
    fn scripts_pin_reference_versions() {
        assert!(cert_manager_script().contains("--version v1.18.2"));
        assert!(network_operator_script().contains("--version v26.1.0"));
        assert!(gpu_operator_script().contains("--version v26.3.0"));
        assert!(kueue_script(1).contains("--version 0.13.0"));
        assert!(mpi_operator_script().contains("/v0.6.0/"));
    }
}
