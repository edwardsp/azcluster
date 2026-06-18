# AKS target — full walkthrough (v0.25.0)

Full end-to-end `azcluster deploy --target aks` walkthrough on the v0.25.0 build. Captured on cluster `aksm5` (representative of the `cmpaks` controlled run), **2× Standard_ND96isr_H200_v5 / mexicocentral**.

This run serves as the AKS counterpart to the Slurm `cmpsl5` walkthrough, using identical hardware and workload parameters.

## 0. Deploy + monitoring

```bash
azcluster deploy --target aks --name aksm5 \
  --location mexicocentral --grafana-location mexicocentral \
  --pool name=gpu,sku=Standard_ND96isr_H200_v5,count=2,default
```

- ARM: 30 resources / **959 s**, including the M5 managed-Prometheus chain.
- Operator stages: cert-manager → NVIDIA Network Operator → NVIDIA GPU Operator → Managed Prometheus DCGM ServiceMonitor (M5) → Kueue → MPI Operator → Azure Container Storage (`local-csi`).
- All operators reported healthy; `rdma/ib: 8` allocatable per node.

## 1. Native operate client (no laptop `kubectl`)

`exec`/`logs`/`ssh` talk to the API server directly over client-cert TLS + WebSocket:

```bash
azcluster exec aksm5 --host gpu-operator/<pod> -- nvidia-smi -L   # lists 8× H200
azcluster logs aksm5 --component gpu-operator/<pod> --tail 8       # streams pod logs
azcluster ssh  aksm5 --host aks-gpu-...-vmss000000                      # host-root chroot shell
```

## 2. NCCL validation (2-node, container)

```bash
export NODES=2
envsubst '${NODES}' < examples/aks/nccl-allreduce-mpijob.yaml | kubectl apply -f -
```

- 2-node MPIJob, `all_reduce_perf_mpi -b 16G -e 16G -f 2 -g 1 -N 10`, 16 ranks.
- **avg busbw 483.36 GB/s**, **8 IB/SHARP devices per node** (16 NICs across the job).
- No TCP fallback. Performance matches Slurm within 1%.

## 3. Training (DGXC Megatron-Bridge, Llama-3.1-8B)

```bash
# Apply training operator and ConfigMap first
# ...
envsubst ... < examples/aks/training-megatron-pytorchjob.yaml | kubectl apply -f -
```

- PyTorchJob, 16 GPUs / 2 nodes, gbs=256, 50 iters.
- steady-state **506.4 MODEL_TFLOP/s/GPU**.

## 4. Storage — azcp stage + blobcache over InfiniBand

```bash
# Phase 1: Stage to Blob
# ...
# Phase 2: Distribute to all nodes (blobcache)
envsubst ... < examples/aks/blobcache-rdma.yaml | kubectl apply -f -
```

- azcp upload: 8.47 GiB in 4.6 s = **15.91 Gbps**.
- blobcache hydrate sharded 9.09 GB; reading on node-1 **peer-fetched 4.54 GB over RDMA** from node-0.
- All IB peer reads verified via `blobcache_peer_chunk_bytes_served_total`.

## 5. Inference — vLLM (Llama-3.1-8B-FP8)

```bash
envsubst ... < examples/aks/inference-vllm.yaml | kubectl apply -f -
```

- **output 9,912 tok/s, median TPOT 12.55 ms, median TTFT 64 ms**.
- Matches Slurm baseline (9918 tok/s) almost exactly.

## 6. Inference — DeepSeek-R1-0528 SGLang TP=16 (multi-node)

```bash
envsubst ... < examples/aks/inference-sglang-multinode.yaml | kubectl apply -f -
```

- Aggregate 16 GPUs across 2 nodes into a single tensor-parallel worker.
- **output 1,258.84 tok/s, median TPOT 47.92 ms, median TTFT 184.56 ms**.
- Note the delta from Slurm (1574 tok/s); see `doc/walkthrough-plan.md` for contributors.

## 7. Controlled Comparison (aksm5 vs. cmpsl5)

| Test | AKS (aksm5) | Slurm (cmpsl5) |
|---|---|---|
| NCCL container (16 ranks) | 483.4 GB/s | 486.2 GB/s |
| Megatron training (16 GPUs) | 506.4 TFLOP/s/GPU | 511.8 TFLOP/s/GPU |
| vLLM Llama-3.1-8B-FP8 | 9912 tok/s | 9918 tok/s |
| DeepSeek-R1 SGLang TP=16 | 1259 tok/s | 1574 tok/s |

## 8. Tear-down

```bash
azcluster delete aksm5 --yes
azcluster purge-kv --name aksm5 --location mexicocentral --yes
```

