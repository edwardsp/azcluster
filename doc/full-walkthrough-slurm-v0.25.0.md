# Slurm target — full walkthrough (v0.25.0)

Full end-to-end `azcluster deploy` walkthrough on the v0.25.0 build. Captured on cluster `cmpsl5`, **2× Standard_ND96isr_H200_v5 / mexicocentral**.

This run serves as the Slurm baseline for the **controlled comparison** with the AKS target. Both targets use identical hardware, region, and workload parameters.

## 0. Deploy + monitoring

```bash
azcluster deploy --name cmpsl5 \
  --location mexicocentral --grafana-location mexicocentral \
  --pool name=gpu,sku=Standard_ND96isr_H200_v5,count=2,default \
  --shared-storage nfs-scheduler --no-accounting --login-public-ip \
  --scheduler-sku Standard_D8s_v5 --login-sku Standard_D4s_v5
```

- ARM: 26 resources / **518 s** (8 min 38 s). Using `nfs-scheduler` and `--no-accounting` saved ~10 minutes.
- Monitoring: AMG and AMW provisioned in mexicocentral. Dashboards imported by the scheduler-side `azcluster-grafana-import` service.
- Status: `azcluster status cmpsl5` reported `READY` for all nodes.

## 1. Default-User Smoke

```bash
azcluster exec cmpsl5 --user clusteradmin -- "sinfo"
# PARTITION AVAIL  TIMELIMIT  NODES  STATE NODELIST
# gpu*         up   infinite      2   idle cmpsl5-gpu-[0003-0004]
```

- Verified LDAP resolution (`getent passwd clusteradmin`) and home dir auto-creation on login.
- Verified inter-node internal keypair propagation (alice can ssh login → gpu-0003 without operator key).

## 2. NCCL validation (Bare Metal vs. Container)

### Plain VM (HPC-X)
```bash
sbatch examples/slurm/nccl-allreduce-vm.sbatch
```
- **avg busbw 485.572 GB/s**, 8 IB/SHARP devices per node.

### Container (nccl-test:latest)
```bash
sbatch examples/slurm/nccl-allreduce.sbatch
```
- **avg busbw 486.217 GB/s**, 8 IB/SHARP devices per node.
- Zero container overhead; Pyxis/Enroot reached the same fabric performance as bare metal.

## 3. Training (DGXC Megatron-Bridge, Llama-3.1-8B)

```bash
sbatch examples/slurm/training-megatron.sbatch
```

- 16 GPUs / 2 nodes, gbs=256, 50 iters.
- steady-state **511.8 MODEL_TFLOP/s/GPU**.
- Scaling efficiency 1→2 nodes measured at ~99.7%.

## 4. Storage — azcp stage + MPI broadcast over InfiniBand

```bash
# Phase 1: Stage to Blob
sbatch examples/slurm/stage-model.sbatch

# Phase 2: Distribute to all nodes
sbatch examples/slurm/distribute-azcp-cluster.sbatch
```

- Stage: `hf download` → NVMe → `azcp copy` to Blob.
- Distribute: `azcp-cluster` MPI broadcast from Blob to all 16 GPUs' local NVMe RAID-0.
- **DeepSeek-R1-0528 (642 GiB)** distributed in **134 s** at **41 Gbps**.

## 5. Inference — vLLM (Llama-3.1-8B-FP8)

```bash
sbatch examples/slurm/inference-vllm.sbatch
```

- Served from per-node NVMe scratch.
- **output 9917.96 tok/s, median TPOT 12.38 ms, median TTFT 62.5 ms** (concurrency 128).

## 6. Inference — DeepSeek-R1-0528 SGLang TP=16

```bash
sbatch examples/slurm/inference-sglang.sbatch
```

- Aggregate 16 GPUs across 2 nodes into a single tensor-parallel worker.
- Weights served from NVMe scratch (broadcast in step 4).
- **output 1573.57 tok/s, median TPOT 39.01 ms, median TTFT 162.9 ms** (concurrency 64).

## 7. Controlled Comparison (cmpsl5 vs. cmpaks)

| Test | Slurm (cmpsl5) | AKS (cmpaks) |
|---|---|---|
| NCCL container (16 ranks) | 486.2 GB/s | 483.6 GB/s |
| Megatron training (16 GPUs) | 511.8 TFLOP/s/GPU | 502.1 TFLOP/s/GPU |
| vLLM Llama-3.1-8B-FP8 | 9918 tok/s | 9888 tok/s |
| DeepSeek-R1 SGLang TP=16 | 1574 tok/s | 1664 tok/s (with `ndv5-topo`) |

**DeepSeek SGLang — `NCCL_TOPO_FILE`:** the original AKS run got only 1,280 tok/s because the sglang example was missing the NDv5 NCCL topology (NCCL fell back to a generic GPU↔NIC↔NVLink graph, ~20% slower on the latency-bound TP=16 decode). Slurm gets the topo automatically via enroot. With the topo added to the AKS example (the `ndv5-topo` ConfigMap), AKS reaches **1,664 tok/s** — matching/exceeding Slurm. Confirmed by the inverse test (Slurm `NCCL_IGNORE_CPU_AFFINITY=1` unchanged → the benefit is NCCL channel/routing, not CPU pinning). See `doc/walkthrough-plan.md` and AGENTS.md.

## 8. Tear-down

```bash
azcluster delete cmpsl5 --yes
azcluster purge-kv --name cmpsl5 --location mexicocentral --yes
```
