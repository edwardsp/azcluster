# azcluster Full Workflow Plan

End-to-end demonstration of azcluster on a 2-node `Standard_ND96isr_H100_v5` cluster (16× H100 SXM5 80GB total, 8× NDR400 InfiniBand per node, 28 TB NVMe RAID-0 per node). Designed to exercise every component the product ships: deploy, identity, storage, multi-node container orchestration, observability, and large-model inference.

This is the **plan** — the version-agnostic description of what we run and why. Concrete runs with commands, timings, and output go in version-specific companions like `full-workflow-v0.24.4.md`.

## Goals

1. Provision a fresh cluster from a single CLI invocation.
2. Validate every default user can submit work without manual setup.
3. Stage a large model to the cluster using the canonical storage path (HF → blob → IB broadcast → NVMe).
4. Show NCCL working on the plain VM and inside a Pyxis container collective performance.
5. Capture thermal/throttle/error telemetry under load.
6. Run a production-realistic inference benchmark single-node and multi-node.
7. Compare to published external numbers where they exist.
8. View live metrics in Grafana.

## Cluster shape

Single canonical command:

```bash
azcluster deploy \
  --name <name> \
  --pool name=gpu,sku=Standard_ND96isr_H100_v5,count=2,default \
  --bastion
```

This provisions:

- Scheduler + login VMs (control plane only, no compute)
- 2× ND96isr_H100_v5 compute nodes (16× H100 SXM5 80GB)
- ANF NFSv4.1 `/shared` (2 TiB Standard)
- Azure Storage account + private endpoint for per-cluster blob (`/data/users/<user>/`)
- Azure Monitor Workspace (Prometheus) + Azure Managed Grafana
- MySQL Flexible Server for Slurm accounting
- Azure Bastion (no public IPs anywhere)
- Per-cluster Key Vault holding cluster state + secrets + admin SSH keypair

## The runs

Each run is independent and idempotent. Run them in order on a fresh cluster; later runs build on artifacts the earlier ones leave in place.

| # | Run | What it exercises | Time |
|---|---|---|---|
| 1 | Deploy + bootstrap probe | ARM provisioning, cloud-init, Bastion routing, identity (Key Vault, LDAP, sacctmgr) | ~15-20 min |
| 2 | Default-user smoke | LDAP user (`clusteradmin`) works without operator intervention; `sinfo`, `srun hostname`, simple sbatch | ~1 min |
| 3 | Bare-metal NCCL all-reduce | NDR400 IB fabric end-to-end, NCCL + HPCX + SHARP, thermal/throttle behaviour under sustained load | ~5 min |
| 4 | Containerised NCCL — single node | Pyxis + Enroot + NGC container; NVLink/NVSwitch path inside container | ~3 min import + 1 min run |
| 5 | Containerised NCCL — multi-node | PMIx across two containers, IB visible inside container (via Mellanox enroot hook) | ~2 min run |
| 6 | Small-model inference (Llama 3.1 8B FP8) | Tests the full storage pipeline at small scale: `hf download` → NVMe → `azcp` → blob → `azcp-cluster` → all-node NVMe → vLLM serve → InferenceX bench client | ~10 min |
| 7 | Large-model inference (DeepSeek-R1-0528 FP8, 671B) | Same pipeline at production scale (~640 GB model), then SGLang TP=16 across both nodes | ~80 min total (most of it model download from HuggingFace) |
| 8 | Observability tour | Read the same data we just generated via Grafana dashboards in the `azcluster` folder | n/a |
| 9 | Tear-down | `azcluster delete` removes the resource group asynchronously | ~10 min async |

## Storage pipeline (used by runs 6 + 7)

Every large artifact follows the same path. The canonical reason is to keep models off `/shared` (which is ANF, ~5 GB/s per session) and onto per-node NVMe RAID-0 (~28 GB/s per node), but go through blob as the cross-node distribution vector so the operator's laptop is never on the critical path.

```
            HuggingFace                Azure Blob              Local NVMe per node
            -----------                ----------              -------------------
[1] one compute node:                                     /mnt/nvme/users/<u>/models/<m>/
    hf download <m> --local-dir /mnt/nvme/users/<u>/models/<m>
                       |
                       v
[2] same compute node (or any node):                       
    azcp copy /mnt/nvme/.../<m>/  $BLOB_URL/models/<m>/
                                          |
                                          v
[3] every compute node (MPI broadcast):
    azcp-cluster $BLOB_URL/models/<m>/  /mnt/nvme/users/<u>/models/<m>/
                                                                  |
                                                                  v
[4] every compute node bind-mounts /mnt/nvme/users/<u>/models into the inference container
```

`azcp-cluster` runs as a 1-rank-per-node MPI job under Pyxis/Enroot. Rank 0 (and other ranks) pull byte-ranges from blob in parallel (`azcp` is range-sharded; each rank takes a fraction of the file set), then exchange via `MPI_Ibcast` over IB so every rank ends with the full set on its local NVMe.

### Tuning recipe for ND96isr_H100_v5

Documented upstream at https://github.com/edwardsp/azcp/blob/main/docs/cluster-h100-tuning.md. Bake into example sbatch:

- `taskset -c 0-47` — pin the rank to NUMA-0 cores (`mlx5_ib0..3` and the frontend NIC are both on NUMA-0; matters for both blob download and IB broadcast)
- `UCX_TLS=rc,sm,self`, `UCX_NET_DEVICES=mlx5_ib0:1`, `OMPI_MCA_pml=ucx`, `OMPI_MCA_osc=ucx` — force IB RDMA, no TCP fallback
- `--bcast-pipeline 128 --bcast-writers 8 --bcast-chunk 67108864` — 128 in-flight chunks, 8 parallel NVMe writers, 64 MiB chunks
- `--concurrency 32 --block-size 16777216` — 32 parallel HTTP requests, 16 MiB blob blocks
- Skip `--container-mounts=/dev/infiniband:/dev/infiniband` — redundant once `MELLANOX_VISIBLE_DEVICES=all` is set in `/etc/enroot/environ.d/50-nccl.env` (enroot auto-mounts `/dev/infiniband/{uverbs,umad,issm}*` and `/dev/infiniband/rdma_cm` via its `99-mellanox.sh` hook)

### What scales and what doesn't

The blob upload (run [2]) is single-node; throughput is bounded by your node's NIC and the storage account's ingress limit, not by cluster size. Expect ~10 Gbps.

The cluster broadcast (run [3]) is range-sharded download from blob plus an MPI-Ibcast across nodes. **Data still has to be read from each rank's local NVMe before it can be broadcast out**; the more nodes you have, the smaller each rank's read share, so the less the NVMe read is a bottleneck. **2 nodes is the worst case** because each rank's NVMe is reading ~50% of the bytes while writing the other ~50% concurrently. At 16 nodes each rank reads ~6.25% and writes ~93.75%, and the doc upstream measures 110 Gbps broadcast at that scale.

## Inference framework: NVIDIA's InferenceMAX (InferenceX)

Open-source benchmark suite from SemiAnalysis: https://github.com/SemiAnalysisAI/InferenceX. We use it because:

1. It's the canonical apples-to-apples comparison for token-throughput numbers on different accelerators and frameworks.
2. The repo ships single-node H100 sbatch wrappers (`benchmarks/single_node/*_h100.sh`) that take a model path, TP count, concurrency, ISL, OSL and produce JSON results + GPU metrics CSV.
3. It includes a benchmark client (`utils/bench_serving/benchmark_serving.py`) that drives the OpenAI-compatible server and measures TTFT, TPOT, ITL, E2EL with percentiles — the standard inference perf vocabulary.

InferenceX's published H100 lineup is 70B+ models (GPT-OSS 120B FP4, DeepSeek-R1 671B FP8, Qwen3.5-397B FP8, MiniMax-M2.5, Kimi-K2.5). For the small-model run we point the same harness at Llama 3.1 8B FP8 (not in their lineup but lets us shake out the pipeline with an 8 GB model in seconds rather than half an hour). For the large-model run we use DeepSeek-R1-0528 FP8 — matches their headline H100 benchmark exactly (config name: `dsr1-fp8-h100-dynamo-sglang`).

We omit the Dynamo orchestrator wrapper and run plain SGLang two-node `--dist-init-addr` directly because:

- Dynamo adds an orchestration layer that needs `srtctl` and its NVIDIA srt-slurm dependency — out of scope for this walkthrough
- The bottleneck we want to demonstrate is the cluster transport, not the orchestrator
- Aggregated TP=16 (single prefill+decode worker) is the SemiAnalysis-published smallest config and is what fits our 2-node test cluster

## Observability

Every metric we plot was scraped by Prometheus (running on each compute node, scraping local node-exporter at `:9100` and dcgm-exporter at `:9400`), then `remote_write`'d to Azure Monitor Workspace via the cluster's managed identity. **The charts in this doc are matplotlib renders of PromQL queries against AMW.** The exact same data is queryable live in Grafana for as long as the cluster exists, in the `azcluster` folder inside the cluster's AMG instance. Find the URL with:

```bash
azcluster monitor <cluster-name>
```

Dashboards shipped:

- **azcluster / Node Health** — CPU, memory, disk, network from node-exporter
- **azcluster / Slurm Scheduler** — queue, partition state, jobs by state (from prometheus-slurm-exporter, requires v0.24.5+)
- **azcluster / GPU + InfiniBand** — DCGM (util, memory, clocks, power, temperature, tlimit, throttle reasons, SM_ACTIVE, PIPE_TENSOR_ACTIVE, NVLink errors, ECC) + node_infiniband (per-port RX/TX rates)
- **azcluster / Node Health Checks** — per-node/per-check status from azhealthcheck (every 5 min via Slurm `HealthCheckProgram`)

For ad-hoc queries the AMG instance also has the auto-registered `Azure Monitor` datasource pointed at AMW — use Explore mode and pick any metric, e.g.:

- `max by (nodename) (DCGM_FI_DEV_GPU_TEMP)` — per-node max GPU die temp
- `rate(node_infiniband_port_data_received_bytes_total[1m]) * 8` — per-IB-port receive bits/sec
- `DCGM_FI_PROF_PIPE_TENSOR_ACTIVE` — tensor-core utilization ratio per GPU
- `DCGM_FI_DEV_GPU_MAX_OP_TEMP` — the in-band tlimit constant (87 on H100 SXM5)
