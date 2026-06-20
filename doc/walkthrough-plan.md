# azcluster Walkthrough Plan

This is the version-agnostic end-to-end plan for demonstrating `azcluster` on both Slurm and AKS targets. It exercises every component the product provisions: infrastructure, identity, storage pipelines, multi-node container orchestration, observability, and large-model inference.

`azcluster` provisions the **infrastructure**. The workloads themselves are visible, runnable files under `examples/slurm/` and `examples/aks/`. These examples intentionally use the same containers, models, GPU counts, and benchmark parameters so that results differ only by orchestration and runtime, not by payload.

## Goals

1. Provision a fresh Slurm or AKS cluster from a single CLI invocation.
2. Validate that default users (Slurm) or native clients (AKS) can submit work without manual setup.
3. Stage a large model to the cluster using the canonical storage path (HuggingFace → Blob → per-node NVMe).
4. Demonstrate high-performance networking (NCCL) directly on the VM (Slurm, non-containerized) and inside containers (Slurm/AKS).
5. Run production-realistic inference benchmarks (vLLM, SGLang) at scale.
6. Run distributed training benchmarks (Megatron-Bridge) and confirm near-linear scaling.
7. Capture and view live telemetry (thermal, throttle, GPU utilization) in Managed Grafana.

## Matched Workloads

| Workload | Slurm Example | AKS Equivalent | Matched Payload |
|---|---|---|---|
| **NCCL plain VM** | `slurm/nccl-allreduce-vm.sbatch` | — | Slurm-only non-containerized HPC-X baseline |
| **NCCL container** | `slurm/nccl-allreduce.sbatch` | `aks/nccl-allreduce-mpijob.yaml` | `nccl-test:latest`, `all_reduce_perf_mpi`, 16 ranks |
| **Megatron training** | `slurm/training-megatron.sbatch` | `aks/training-megatron-pytorchjob.yaml` | `nemo:26.04.00`, Llama-3.1-8B BF16 pretraining |
| **vLLM inference** | `slurm/inference-vllm.sbatch` | `aks/inference-vllm.yaml` | `vllm-openai:latest`, Llama-3.1-8B-FP8 |
| **DeepSeek SGLang** | `slurm/inference-sglang.sbatch` | `aks/inference-sglang-multinode.yaml` | `sglang:v0.5.8-cu130`, DeepSeek-R1-0528, TP=16 |
| **Storage stage** | `slurm/stage-model.sbatch` | `aks/stage-model.yaml` | HF download to NVMe → `azcp` to Blob |
| **Storage distribute** | `slurm/distribute-azcp-cluster.sbatch` | `aks/blobcache-rdma.yaml` | Blob-backed model distribution to all nodes over IB |

## Cluster Shape

### Slurm (Production-style)
```bash
azcluster deploy --name <name> \
  --pool name=gpu,sku=Standard_ND96isr_H200_v5,count=2,default \
  --bastion
```
For rapid testing, use the fast-mode flags:
`--shared-storage nfs-scheduler --no-accounting --login-public-ip --scheduler-sku Standard_D8s_v5 --login-sku Standard_D4s_v5`

### AKS (GPU target)
```bash
azcluster deploy --target aks --name <name> \
  --location mexicocentral \
  --pool name=gpu,sku=Standard_ND96isr_H200_v5,count=2,default
```

## Run Order

### 1. Deploy + Bootstrap Probe
Provision the cluster and wait for the `status` command to report `READY`.
- **Slurm**: `azcluster status <name>` (wait for login/scheduler nodes).
- **AKS**: `azcluster kubeconfig <name> && export KUBECONFIG=... && azcluster status <name>` (wait for nodes and operators).

### 2. Default-User Smoke
- **Slurm**: `azcluster exec <name> --user clusteradmin -- "sinfo"`
- **AKS**: `azcluster exec <name> --host gpu-operator/<dcgm-pod> -- nvidia-smi -L`

### 3. NCCL Validation
Verify the InfiniBand fabric performance.
- **Slurm (non-containerized VM)**: `sbatch examples/slurm/nccl-allreduce-vm.sbatch`
- **Slurm (Container)**: `sbatch examples/slurm/nccl-allreduce.sbatch`
- **AKS (Container)**: `envsubst '${NODES}' < examples/aks/nccl-allreduce-mpijob.yaml | kubectl apply -f -`

### 4. Megatron Training
Measure strong-scaling efficiency (data-parallel gradient all-reduce).
- **Slurm**: `sbatch examples/slurm/training-megatron.sbatch`
- **AKS**: `kubectl apply -f examples/aks/training-operator.yaml` then apply `examples/aks/training-megatron-pytorchjob.yaml`

### 5. Storage Pipeline (Stage & Distribute)
Big models follow a two-phase path: stage once to Blob, distribute every job to NVMe over IB.
- **Phase 1 (Stage)**:
  - **Slurm**: `sbatch examples/slurm/stage-model.sbatch`
  - **AKS**: `envsubst ... < examples/aks/stage-model.yaml | kubectl apply -f -`
- **Phase 2 (Distribute)**:
  - **Slurm**: `sbatch examples/slurm/distribute-azcp-cluster.sbatch`
  - **AKS**: `envsubst ... < examples/aks/blobcache-rdma.yaml | kubectl apply -f -`

### 6. vLLM Inference
- **Slurm**: `sbatch examples/slurm/inference-vllm.sbatch`
- **AKS**: `envsubst ... < examples/aks/inference-vllm.yaml | kubectl apply -f -`

### 7. DeepSeek SGLang TP=16
Aggregate 16 GPUs across 2 nodes into a single tensor-parallel worker.
- **Slurm**: `sbatch examples/slurm/inference-sglang.sbatch`
- **AKS**: `envsubst ... < examples/aks/inference-sglang-multinode.yaml | kubectl apply -f -`

### 8. Observability
Open the Grafana dashboards to view live metrics during the runs.
```bash
azcluster monitor <name>
```

### 9. Teardown
```bash
azcluster delete <name> --yes
azcluster purge-kv --name <name> --location <region> --yes
```

## Storage Pipeline: Performance & Scaling

Keeping models off `/shared` (ANF) and onto per-node NVMe RAID-0 (~28 GB/s) is critical for performance. `azcluster` uses Blob as the cross-node distribution vector.

- **Phase 1 (Upload)** is bounded by the node's NIC and storage ingress (~10-15 Gbps).
- **Phase 2 (Broadcast)** scales near-linearly with node count. 2 nodes is the worst case (~40 Gbps) because each rank reads from NVMe while concurrently writing. At 16 nodes, broadcast reaches ~110 Gbps as read overhead is minimized.

## Controlled Comparison: Slurm vs. AKS

This comparison uses identical hardware (2× Standard_ND96isr_H200_v5 in mexicocentral) and identical container images/models/params. The goal is to isolate orchestration overhead.

| Test | Slurm (cmpsl5) | AKS (cmpaks) |
|---|---|---|
| NCCL plain-VM (HPC-X, 16 GiB, 20 iters, 16 ranks) | 485.572 GB/s avg busbw | — (Slurm-only) |
| NCCL container (nccl-test:latest, 16 ranks) | 486.217 GB/s avg busbw | 483.584 GB/s avg busbw |
| Megatron-Bridge Llama-3.1-8B training (16 GPUs) | ~511.8 MODEL_TFLOP/s/GPU | ~502 MODEL_TFLOP/s/GPU [session-captured] |
| vLLM Llama-3.1-8B-FP8 (concurrency 128) | 9917.96 tok/s output | 9888 tok/s output [session-captured] |
| DeepSeek-R1-0528 SGLang TP=16 (conc 64) | 1,536 tok/s output | 1,664 tok/s output |

**Takeaway**: NCCL, training, and vLLM match within ~2% between Slurm and AKS. Orchestration adds no measurable overhead on identical hardware.

**Multi-node NCCL needs `NCCL_TOPO_FILE`.** Any cross-node collective (TP=16 decode especially) must point NCCL at the NDv5 topology (`/etc/topology/ndv5-topo.xml`); without it NCCL falls back to a generic GPU↔NIC↔NVLink graph and decode runs ~20% slower (1,339 vs 1,664 tok/s on 2× ND H200). **azcluster sets this automatically on Slurm** (enroot `environ.d` + `profile.d`); on AKS set it via the `ndv5-topo` ConfigMap — the example does. Same requirement on both targets, only auto-vs-manual differs.
