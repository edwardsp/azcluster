use crate::aks::single_quote;
use crate::cluster_state::ClusterState;
use crate::TrainArgs;
use anyhow::{anyhow, bail, Result};
use base64::Engine;

const PRETRAIN_PY: &str = include_str!("manifests/megatron-pretrain.py");
const TRAINING_PYTORCHJOB: &str = include_str!("manifests/training-pytorchjob.yaml");

// Trimmed training-operator v1.8.1: operator + RBAC + self-cert Secret + a
// minimal PyTorchJob CRD. Upstream's `kubectl apply -k github.com/...` needs git
// (absent in the runCommand pod) and bundles every job CRD (3.3 MB). The
// ValidatingWebhookConfiguration is dropped on purpose — its failurePolicy=Fail
// pytorchjob webhook races the operator's runtime cert self-gen, and the
// operator reconciles PyTorchJobs without admission validation.
const TRAINING_OPERATOR_MANIFEST: &str = include_str!("manifests/training-operator.yaml");

fn install_training_operator_script() -> String {
    let b64 = base64::engine::general_purpose::STANDARD.encode(TRAINING_OPERATOR_MANIFEST);
    format!(
        r#"set -eu
printf %s {b64} | base64 -d | kubectl apply --server-side --force-conflicts -f -
kubectl -n kubeflow wait --for=condition=Available deploy/training-operator --timeout=300s
kubectl get crd pytorchjobs.kubeflow.org >/dev/null
echo "training-operator ready"
"#,
        b64 = single_quote(&b64)
    )
}

fn wait_and_log_script(target: usize) -> String {
    format!(
        r#"set -eu
target={target}
status=warming
for i in $(seq 1 36); do
  n=$(kubectl -n default logs azcluster-llama-train-master-0 --tail=-1 2>/dev/null | grep -cE 'MODEL_TFLOP/s/GPU' || true)
  ph=$(kubectl -n default get pod azcluster-llama-train-master-0 -o jsonpath='{{.status.phase}}' 2>/dev/null || true)
  echo "poll $i: samples=${{n:-0}} phase=${{ph:-pending}}"
  if [ "${{n:-0}}" -ge "$target" ]; then status=ready; break; fi
  if [ "${{ph:-}}" = "Failed" ]; then status=failed; break; fi
  sleep 13
done
echo "WAIT_STATUS=$status"
echo '==== metrics ===='
kubectl -n default logs azcluster-llama-train-master-0 --tail=-1 2>&1 | grep -E 'MODEL_TFLOP/s/GPU' || true
"#
    )
}

pub(crate) fn train_aks(state: &ClusterState, args: &TrainArgs) -> Result<()> {
    let aks = state
        .aks
        .as_ref()
        .ok_or_else(|| anyhow!("cluster '{}' is not an AKS cluster", state.name))?;
    let arm = crate::arm_client()?;
    let rg = &state.resource_group;
    let cluster = &aks.aks_cluster_name;

    let nodes = args.nodes.max(1);
    if nodes > aks.gpu_node_count {
        bail!(
            "--nodes {nodes} exceeds the cluster's {} GPU node(s)",
            aks.gpu_node_count
        );
    }
    let worker_replicas = nodes - 1;
    let gbs = args.gbs.unwrap_or(nodes * 128);

    eprintln!("==> [aks train] installing Kubeflow training-operator (idempotent)");
    let r = arm.aks_run_command(rg, cluster, &install_training_operator_script(), None)?;
    if r.exit_code != 0 {
        bail!(
            "training-operator install failed (exit {}):\n{}",
            r.exit_code,
            r.logs
        );
    }

    let job = TRAINING_PYTORCHJOB
        .replace("{{WORKER_REPLICAS}}", &worker_replicas.to_string())
        .replace("{{TRAIN_ITERS}}", &args.iters.to_string())
        .replace("{{GBS}}", &gbs.to_string())
        .replace("{{CP}}", &args.cp.to_string());
    eprintln!(
        "==> [aks train] submitting Llama-3.1-8B Megatron-Bridge pretrain: {nodes} node(s) / {} GPUs, gbs={gbs}, iters={}",
        nodes * 8,
        args.iters
    );
    let r = arm.aks_run_command(rg, cluster, &submit_script(&job), None)?;
    if r.exit_code != 0 {
        bail!("training submit failed (exit {}):\n{}", r.exit_code, r.logs);
    }
    eprintln!("{}", r.logs.trim());

    if !args.wait {
        eprintln!(
            "==> [aks train] submitted. Re-run with --wait to block and report steady-state MODEL_TFLOP/s/GPU."
        );
        return Ok(());
    }

    let target = args.iters.clamp(8, 25) as usize;
    eprintln!(
        "==> [aks train] waiting for the run to reach steady state (target {target} samples)..."
    );
    let mut values = Vec::new();
    let mut last_logs = String::new();
    for attempt in 1..=8 {
        let r = arm.aks_run_command(rg, cluster, &wait_and_log_script(target), None)?;
        last_logs = r.logs;
        values = parse_tflops_values(&last_logs);
        if values.len() >= target || last_logs.contains("WAIT_STATUS=failed") {
            break;
        }
        eprintln!(
            "==> [aks train] warming up: {} metric sample(s) so far (poll {attempt}/8)",
            values.len()
        );
    }
    if last_logs.contains("WAIT_STATUS=failed") && values.is_empty() {
        bail!("training launcher pod entered Failed with no metrics:\n{last_logs}");
    }
    match steady_state(&values) {
        Some(t) => {
            eprintln!("==> [aks train] steady-state MODEL_TFLOP/s/GPU: {t:.1}");
            eprintln!(
                "    ({} GPUs across {nodes} node(s); run `azcluster validate {}` for the NCCL IB/SHARP busbw gate)",
                nodes * 8,
                state.name
            );
            Ok(())
        }
        None => bail!(
            "training produced no MODEL_TFLOP/s/GPU metric — launcher may have failed:\n{last_logs}"
        ),
    }
}

fn submit_script(job_yaml: &str) -> String {
    let py = single_quote(PRETRAIN_PY);
    let job = single_quote(job_yaml);
    format!(
        r#"set -eu
printf %s {py} > /tmp/pretrain.py
kubectl -n default delete configmap azcluster-llama-pretrain --ignore-not-found
kubectl -n default create configmap azcluster-llama-pretrain --from-file=pretrain.py=/tmp/pretrain.py
kubectl -n default delete pytorchjob azcluster-llama-train --ignore-not-found
printf %s {job} | kubectl apply --server-side -f -
echo "submitted PyTorchJob azcluster-llama-train"
"#
    )
}

fn parse_tflops_values(log: &str) -> Vec<f64> {
    let marker = "GPU utilization:";
    let mut out = Vec::new();
    for line in log.lines() {
        let Some(i) = line.find(marker) else {
            continue;
        };
        let rest = &line[i + marker.len()..];
        let Some(j) = rest.find("MODEL_TFLOP") else {
            continue;
        };
        if let Ok(v) = rest[..j].trim().parse::<f64>() {
            out.push(v);
        }
    }
    out
}

fn steady_state(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let skip = values.len() / 4;
    let mut tail: Vec<f64> = values[skip..].to_vec();
    if tail.is_empty() {
        tail = values.to_vec();
    }
    tail.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    Some(tail[tail.len() / 2])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tflops_from_step_lines() {
        let log = "Step Time : 31.79s GPU utilization: 212.2MODEL_TFLOP/s/GPU\n\
                   Step Time : 13.35s GPU utilization: 505.3MODEL_TFLOP/s/GPU\n\
                   Step Time : 13.41s GPU utilization: 503.1MODEL_TFLOP/s/GPU\n\
                   unrelated line\n";
        let v = parse_tflops_values(log);
        assert_eq!(v, vec![212.2, 505.3, 503.1]);
    }

    #[test]
    fn steady_state_skips_warmup() {
        let v = vec![212.0, 500.0, 503.0, 504.0, 502.0, 503.0, 503.0, 502.0];
        let s = steady_state(&v).unwrap();
        assert!((502.0..=504.0).contains(&s), "got {s}");
    }

    #[test]
    fn steady_state_none_when_empty() {
        assert!(steady_state(&[]).is_none());
        assert!(parse_tflops_values("no metric here").is_empty());
    }
}
