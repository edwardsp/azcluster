use crate::aks::single_quote;
use crate::cluster_state::ClusterState;
use crate::ValidateArgs;
use anyhow::{anyhow, bail, Result};
use std::collections::BTreeSet;

const NCCL_VALIDATE_MPIJOB: &str = include_str!("manifests/nccl-validate-mpijob.yaml");
const MIN_BUSBW_GBPS: f64 = 400.0;
const MIN_IB_SHARP_DEVICES: usize = 8;

#[derive(Debug, Clone, PartialEq)]
struct NcclVerdict {
    avg_busbw_gbps: Option<f64>,
    ib_sharp_devices: usize,
    used_tcp_fallback: bool,
}

impl NcclVerdict {
    fn passes(&self) -> bool {
        self.avg_busbw_gbps
            .is_some_and(|busbw| busbw >= MIN_BUSBW_GBPS)
            && self.ib_sharp_devices >= MIN_IB_SHARP_DEVICES
            && !self.used_tcp_fallback
    }
}

pub(crate) fn validate_aks(state: &ClusterState, _args: &ValidateArgs) -> Result<()> {
    let aks = state
        .aks
        .as_ref()
        .ok_or_else(|| anyhow!("cluster '{}' is not an AKS cluster", state.name))?;
    let arm = crate::arm_client()?;
    let script = validation_script(aks.gpu_node_count);

    eprintln!(
        "==> [aks validate] submitting 2-node NCCL MPIJob to gpu-local-queue on '{}'",
        aks.aks_cluster_name
    );
    let result =
        arm.aks_run_command(&state.resource_group, &aks.aks_cluster_name, &script, None)?;
    if result.exit_code != 0 {
        bail!(
            "AKS NCCL validation job failed: provisioning_state={}, exit_code={}\n{}",
            result.provisioning_state,
            result.exit_code,
            result.logs
        );
    }

    let verdict = evaluate_nccl_log(&result.logs);
    let busbw = verdict
        .avg_busbw_gbps
        .map(|v| format!("{v:.2} GB/s"))
        .unwrap_or_else(|| "missing".to_string());
    eprintln!(
        "==> [aks validate] NCCL avg busbw: {busbw}; IB/SHARP devices: {}; TCP fallback: {}",
        verdict.ib_sharp_devices, verdict.used_tcp_fallback
    );
    if !verdict.passes() {
        bail!(
            "AKS NCCL validation gates failed: avg_busbw_gbps={:?} (required >= {MIN_BUSBW_GBPS}), ib_sharp_devices={} (required >= {MIN_IB_SHARP_DEVICES}), used_tcp_fallback={}",
            verdict.avg_busbw_gbps,
            verdict.ib_sharp_devices,
            verdict.used_tcp_fallback
        );
    }
    eprintln!("==> [aks validate] NCCL busbw + InfiniBand gates passed");
    Ok(())
}

fn validation_script(node_count: u32) -> String {
    let manifest =
        single_quote(&NCCL_VALIDATE_MPIJOB.replace("{{NODES}}", &node_count.to_string()));
    format!(
        r#"set -eu
kubectl delete mpijob azcluster-nccl-validate -n default --ignore-not-found
printf %s {manifest} | kubectl apply --server-side -f -
done=0
for i in $(seq 1 90); do
  s=$(kubectl -n default get job azcluster-nccl-validate-launcher -o jsonpath='{{.status.succeeded}}' 2>/dev/null || true)
  f=$(kubectl -n default get job azcluster-nccl-validate-launcher -o jsonpath='{{.status.failed}}' 2>/dev/null || true)
  case "${{s:-0}}" in ''|*[!0-9]*) s=0 ;; esac
  case "${{f:-0}}" in ''|*[!0-9]*) f=0 ;; esac
  if [ "$s" -ge 1 ] || [ "$f" -ge 1 ]; then done=1; break; fi
  echo "waiting for NCCL launcher ($i/90) succeeded=$s failed=$f"
  sleep 15
done
if [ "$done" -ne 1 ]; then
  echo "NCCL launcher did not reach a terminal state in time" >&2
  kubectl -n default describe mpijob azcluster-nccl-validate 2>&1 | tail -30 >&2 || true
  exit 1
fi
echo '==== NCCL launcher result ===='
kubectl -n default logs job/azcluster-nccl-validate-launcher --tail=-1 2>&1 \
  | grep -E 'Avg bus bandwidth|NET/IB : Using|NET/Socket|No device found' || true
"#
    )
}

fn evaluate_nccl_log(log: &str) -> NcclVerdict {
    let avg_busbw_gbps = log.lines().find_map(parse_avg_busbw);
    let mut devices = BTreeSet::new();
    for line in log.lines() {
        if line.contains("NET/IB : Using") && line.contains("mlx5_") && line.contains("SHARP") {
            collect_ib_devices(line, &mut devices);
        }
    }
    NcclVerdict {
        avg_busbw_gbps,
        ib_sharp_devices: devices.len(),
        used_tcp_fallback: log.contains("NET/Socket") || log.contains("NET/IB : No device found"),
    }
}

fn parse_avg_busbw(line: &str) -> Option<f64> {
    let marker = "Avg bus bandwidth";
    let idx = line.find(marker)?;
    line[idx + marker.len()..]
        .split_once(':')?
        .1
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

fn collect_ib_devices(line: &str, devices: &mut BTreeSet<String>) {
    for (idx, _) in line.match_indices("mlx5_") {
        let mut tail = &line[idx + "mlx5_".len()..];
        let mut name = String::from("mlx5_");
        if let Some(stripped) = tail.strip_prefix("ib") {
            name.push_str("ib");
            tail = stripped;
        }
        let digits: String = tail.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() {
            continue;
        }
        let after_digits = &tail[digits.len()..];
        if after_digits.starts_with(":") && after_digits.contains("/IB") {
            name.push_str(&digits);
            devices.insert(name);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aks_mlx5_naming_sample_passes() {
        let log = r#"
NCCL INFO NET/IB : Using [0]mlx5_0:1/IB/SHARP [1]mlx5_1:1/IB/SHARP [2]mlx5_2:1/IB/SHARP [3]mlx5_3:1/IB/SHARP [4]mlx5_4:1/IB/SHARP [5]mlx5_5:1/IB/SHARP [6]mlx5_6:1/IB/SHARP [7]mlx5_7:1/IB/SHARP [8]mlx5_8:1/RoCE [RO]; OOB eth0:10.0.0.2<0>
# Avg bus bandwidth    : 483.716
"#;
        let verdict = evaluate_nccl_log(log);
        assert_eq!(verdict.avg_busbw_gbps, Some(483.716));
        assert_eq!(verdict.ib_sharp_devices, 8);
        assert!(!verdict.used_tcp_fallback);
        assert!(verdict.passes());
    }

    #[test]
    fn pass_sample_has_busbw_and_eight_ib_sharp_devices() {
        let log = r#"
NCCL INFO NET/IB : Using [0]mlx5_ib0:1/IB/SHARP [1]mlx5_ib1:1/IB/SHARP [2]mlx5_ib2:1/IB/SHARP [3]mlx5_ib3:1/IB/SHARP [4]mlx5_ib4:1/IB/SHARP [5]mlx5_ib5:1/IB/SHARP [6]mlx5_ib6:1/IB/SHARP [7]mlx5_ib7:1/IB/SHARP
# Avg bus bandwidth    : 440.21
"#;
        let verdict = evaluate_nccl_log(log);
        assert_eq!(verdict.avg_busbw_gbps, Some(440.21));
        assert_eq!(verdict.ib_sharp_devices, 8);
        assert!(!verdict.used_tcp_fallback);
        assert!(verdict.passes());
    }

    #[test]
    fn tcp_fallback_sample_fails_both_gates() {
        let log = r#"
NCCL INFO NET/IB : No device found.
NCCL INFO NET/Socket : Using [0]eth0:10.0.0.2<0>
# Avg bus bandwidth    : 42.10
"#;
        let verdict = evaluate_nccl_log(log);
        assert_eq!(verdict.avg_busbw_gbps, Some(42.10));
        assert_eq!(verdict.ib_sharp_devices, 0);
        assert!(verdict.used_tcp_fallback);
        assert!(!verdict.passes());
    }

    #[test]
    fn low_busbw_sample_fails_busbw_gate() {
        let log = r#"
NCCL INFO NET/IB : Using [0]mlx5_ib0:1/IB/SHARP [1]mlx5_ib1:1/IB/SHARP [2]mlx5_ib2:1/IB/SHARP [3]mlx5_ib3:1/IB/SHARP [4]mlx5_ib4:1/IB/SHARP [5]mlx5_ib5:1/IB/SHARP [6]mlx5_ib6:1/IB/SHARP [7]mlx5_ib7:1/IB/SHARP
# Avg bus bandwidth    : 310.0
"#;
        let verdict = evaluate_nccl_log(log);
        assert_eq!(verdict.avg_busbw_gbps, Some(310.0));
        assert_eq!(verdict.ib_sharp_devices, 8);
        assert!(!verdict.used_tcp_fallback);
        assert!(!verdict.passes());
    }
}
