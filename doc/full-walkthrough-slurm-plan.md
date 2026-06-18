# azcluster Full Walkthrough Plan

End-to-end demonstration of azcluster on a 2-node `Standard_ND96isr_H100_v5` cluster (16× H100 SXM5 80GB total, 8× NDR400 InfiniBand per node, 28 TB NVMe RAID-0 per node). Designed to exercise every component the product ships: deploy, identity, storage, multi-node container orchestration, observability, and large-model inference.

This is the **plan** — the version-agnostic description of what we run and why. Concrete runs with commands, timings, and output go in version-specific companions like `full-walkthrough-slurm-v0.24.4.md`.

## Goals

1. Provision a fresh cluster from a single CLI invocation.
2. Validate every default user can submit work without manual setup.
3. Stage a large model to the cluster using the canonical storage path (HF → blob → IB broadcast → NVMe).
4. Show NCCL working on the plain VM and inside a Pyxis container collective performance.
5. Capture thermal/throttle/error telemetry under load.
6. Run a production-realistic inference benchmark single-node and multi-node.
7. Run a distributed training benchmark single-node and multi-node and confirm near-linear strong scaling over the IB fabric.
8. Compare to published external numbers where they exist.
9. View live metrics in Grafana.

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
| 8 | Distributed training benchmark (DGXC Llama 3.1 8B BF16) | NVIDIA `dgxc-benchmarking` `llmb-run`: NeMo/Megatron-Bridge pretraining single-node (8 GPU) then multi-node (16 GPU); strong-scaling efficiency of the data-parallel gradient all-reduce over the IB fabric | ~25-30 min one-time sqsh build+broadcast + ~10 min/scale |
| 9 | Observability tour | Read the same data we just generated via Grafana dashboards in the `azcluster` folder | n/a |
| 10 | Tear-down | `azcluster delete` removes the resource group asynchronously | ~10 min async |

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

## Concrete sbatch scripts

Every run uses scripts checked into this section. The same scripts go into both this plan and the version-specific walkthrough docs so numbers are reproducible.

Secrets are passed via environment variables — **never bake tokens, passwords, or other secrets into committed sbatch files**. The relevant env vars are noted per script.

### Prerequisites — credentials and secrets

The walkthrough pulls container images and model weights from third-party registries. **Set these as environment variables in your shell before running the relevant sbatches, or write them into per-user files under `${HOME}/.config/...` — never commit them into source.**

| Secret | Where it's used | How to set it on the cluster |
|---|---|---|
| **NGC API key** (for `nvcr.io/nvidia/...` pulls) | `dgxc-nemo-{container,multinode}-smoke.sbatch`; the DGXC training run (step 7 — `llmb-install`/`llmb-run` pull `nvcr.io/nvidia/nemo`); anything that imports a NeMo / NGC container under `nvcr.io/nvidia/` | NGC public images can pull anonymously (this walkthrough's runs all worked without a key) but anonymous pulls are heavily rate-limited and some images are gated. To set a key for an LDAP user: `azcluster ssh <name> --user clusteradmin` then `mkdir -p ~/.config/enroot && cat > ~/.config/enroot/.credentials <<EOF`<br/>`machine nvcr.io login $oauthtoken password <NGC_API_KEY>`<br/>`EOF`<br/>`chmod 0600 ~/.config/enroot/.credentials`<br/><br/>Get an NGC API key at `https://ngc.nvidia.com/setup/api-key` (free signup). The login is literally the string `$oauthtoken` (NGC convention). Set this if `enroot import` returns HTTP 401/403, or pre-emptively for production runs. |
| **Hugging Face token** (gated models only) | `llama-pipeline.sbatch`, `dsr1-pipeline.sbatch` if pulling a gated repo; **required** for the DGXC training run (step 7) because the `Meta-Llama-3.1` NeMo configs are gated | `azcluster ssh <name> --user clusteradmin` then store at `~/.hf-token` (mode 0600). In the sbatch, `export HF_TOKEN=$(cat ~/.hf-token)` before the `hf download`. The two inference models we use (`neuralmagic/Meta-Llama-3.1-8B-Instruct-FP8`, `deepseek-ai/DeepSeek-R1-0528`) are public and don't need a token; gated models (Meta's own Llama configs used by DGXC, Qwen3.5-FP8) do. The DGXC training flow (step 7) reads the token from `$HOME/.config/azcluster/hf_token` instead — see that section. |
| **Azure access** | Everything | `azcluster login` once on the operator's laptop — token cache lives at `~/.azure/azcli_tokens.json`. The cluster itself uses managed identities for blob and AMW access; no operator action needed. |

Without an NGC key, anonymous pulls from `nvcr.io` may succeed for public images but get heavily rate-limited (and some images require it outright). When `enroot import` fails with HTTP 401 or 403, the credentials file is the fix.

### 0. Cluster + Grafana — deploy

```bash
# Required env: AZURE login (run `azcluster login` once)
azcluster deploy --name <name> \
  --pool name=gpu,sku=Standard_ND96isr_H100_v5,count=2,default \
  --bastion
```

Wait ~17 min ARM + ~10 s dashboard import. Then:

```bash
$ azcluster monitor <name>
https://amg-<name>-<hash>.eus.grafana.azure.com
```

Open the URL → `azcluster` folder → 4 dashboards (Node Health, Slurm Scheduler, GPU + InfiniBand, Node Health Checks).

### 1. Default-user smoke

```bash
azcluster exec <name> --user clusteradmin -- "id && hostname && sinfo"
```

Expect both compute nodes to report `idle` in the `gpu` partition.

### 2. NCCL on plain VM, 2 nodes × 16 ranks, `-b 16G -e 16G -N 10`

Script: `nccl-N10.sbatch` (lands in operator home or `/shared/home/<user>/`).

```bash
#!/bin/bash -l
#SBATCH --job-name=nccl-N10
#SBATCH --output=nccl-N10-%j.out
#SBATCH --nodes=2
#SBATCH --ntasks-per-node=8
#SBATCH --gpus-per-node=8
#SBATCH --exclusive

# Pick the in-image HPC-X (PMIx 4.x; matches Slurm 25.11's mpi_pmix_v4.so)
HPCX_DIR=$(ls -d /opt/hpcx-*-gcc-doca_ofed-ubuntu24.04-cuda*-x86_64 2>/dev/null | head -1)
[ -n "$HPCX_DIR" ] || { echo "HPC-X not found"; exit 1; }
source "${HPCX_DIR}/hpcx-init.sh"
hpcx_load

# NDv5 NCCL key env vars per AGENTS.md
export NCCL_DEBUG=INFO
export NCCL_IB_HCA=mlx5_ib
export NCCL_TOPO_FILE=/opt/microsoft/ndv5-topo.xml
export UCX_NET_DEVICES=mlx5_ib0:1,mlx5_ib1:1,mlx5_ib2:1,mlx5_ib3:1,mlx5_ib4:1,mlx5_ib5:1,mlx5_ib6:1,mlx5_ib7:1

srun --mpi=pmix /opt/nccl-tests/build/all_reduce_perf -b 16G -e 16G -N 10 -g 1
```

Submit:

```bash
azcluster scp <name> --user clusteradmin nccl-N10.sbatch :/shared/home/clusteradmin/
azcluster exec <name> --user clusteradmin -- "sbatch nccl-N10.sbatch"
```

Expect `# Avg bus bandwidth : 440-466 GB/s` (live runs land in this band with `-b 16G -e 16G -N 10`: `v0.24.12` 461.6, `v2420walk` job 28 440.21 — node-to-node variance, not a regression).

### 3. NCCL in a NeMo container, 2 nodes × 16 ranks (apples-to-apples with §2)

This is the containerised counterpart of §2. To make the bare-metal-vs-container comparison meaningful, run the **identical** `all_reduce_perf -b 16G -e 16G -N 10` binary inside the NeMo container via Pyxis — the only variable is the execution environment (the in-image HPC-X on the bare VM vs the same HPC-X shipped inside the container). The NeMo image (`nvcr.io/nvidia/nemo:25.07.02`) ships `all_reduce_perf` and `all_reduce_perf_mpi` prebuilt, so nothing is compiled.

**Consistency note (why not the Python helper).** Earlier walkthroughs measured the container path with a Python `torchrun` helper (`nccl_allreduce_smoke.py`, a 1 GiB fp16 tensor × 20 iters). That drives a *different* message size and collective path than §2's `all_reduce_perf -b 16G -e 16G`, so its IB-throughput chart was **not** comparable to the plain-VM chart — on the 1 GiB payload the per-NIC receive rates stayed near the noise floor and the chart looked empty next to §2's multi-Gbps spike. The Python helper remains a fine quick functional smoke (3a), but the `all_reduce_perf` container run (3b) is the benchmark of record: it reproduces §2 exactly, so the two NCCL charts in §8 are a true matched pair. Live `v2420walk` result: §2 (bare metal) **440.21 GB/s** vs §3b (container) **451.08 GB/s** — within ~2.5%, i.e. the Pyxis/Enroot path adds no measurable NCCL overhead.

The smoke sbatch (3a) and the legacy Python multinode smoke are shipped by cloud-init at `/shared/examples/`; `nccl-N10-container.sbatch` (3b) is reproduced here and scp'd by the operator (it mirrors `nccl-N10.sbatch` from §2).

#### 3a. Container import + single-node smoke (`dgxc-nemo-container-smoke.sbatch`)

Optional warm-up. Drops the `nccl_allreduce_smoke.py` helper into `/shared/dgxc/`, then runs an 8-rank intra-node all-reduce inside the NeMo container. Its real value is warming the squashfs cache: first-run import of `nvcr.io/nvidia/nemo:25.07.02` (~16 GB) takes ~25 min on a cold node, so running this first means 3b's import is seconds. This is a functional check (Pyxis import + IB-in-container), not the benchmark of record — see the consistency note above.

```bash
#!/usr/bin/env bash
#SBATCH --job-name=dgxc-nemo-smoke
#SBATCH --output=dgxc-nemo-smoke-%j.out
#SBATCH --partition=gpu
#SBATCH --nodes=1
#SBATCH --ntasks-per-node=1
#SBATCH --gpus-per-node=8
#SBATCH --exclusive
#SBATCH --time=00:30:00

NEMO_IMAGE=${NEMO_IMAGE:-nvcr.io/nvidia/nemo:25.07.02}

mkdir -p /shared/dgxc
cat > /shared/dgxc/nccl_allreduce_smoke.py <<'PY'
import os, time, torch
import torch.distributed as dist

def main():
    local_rank = int(os.environ["LOCAL_RANK"])
    torch.cuda.set_device(local_rank)
    dist.init_process_group(backend="nccl")
    rank, world = dist.get_rank(), dist.get_world_size()
    print(f"rank {rank} / world {world} on device {torch.cuda.current_device()} "
          f"({torch.cuda.get_device_name(local_rank)})", flush=True)

    # Warmup + measured all-reduce of a 1 GiB float16 tensor.
    numel = 512 * 1024 * 1024          # 512 M elements = 1 GiB fp16
    tensor = torch.ones(numel, dtype=torch.float16, device="cuda")
    for _ in range(5):
        dist.all_reduce(tensor)
    torch.cuda.synchronize()

    iters = 20
    dist.barrier()
    t0 = time.perf_counter()
    for _ in range(iters):
        dist.all_reduce(tensor)
    torch.cuda.synchronize()
    elapsed = time.perf_counter() - t0

    if rank == 0:
        size_bytes = numel * 2
        # busbw factor for ring/tree all-reduce = 2*(N-1)/N
        algbw = size_bytes * iters / elapsed / 1e9
        busbw = algbw * 2 * (world - 1) / world
        print(f"all_reduce size=1GiB iters={iters} elapsed={elapsed:.3f}s "
              f"algbw={algbw:.2f} GB/s avg busbw={busbw:.2f} GB/s",
              flush=True)
    dist.destroy_process_group()

if __name__ == "__main__":
    main()
PY

srun \
  --container-image="${NEMO_IMAGE}" \
  --container-mounts=/shared:/shared,/mnt/nvme:/mnt/nvme \
  --no-container-mount-home \
  --export=ALL,NCCL_DEBUG=INFO \
  bash -c 'set -euo pipefail; cd /; torchrun --nproc_per_node=8 /shared/dgxc/nccl_allreduce_smoke.py'
```

Success criteria: `pyxis: imported docker image: ...` + `rank 0 / world 8` + `all_reduce avg busbw` > 100 GB/s + `NCCL_DEBUG INFO` mentions `IBext_v11` + `mlx5_ib` (not `via SOCKET`).

#### 3b. Consistent multinode (`nccl-N10-container.sbatch`) — reproduces §2

The benchmark of record. Identical `all_reduce_perf -b 16G -e 16G -N 10` as §2, run across 2 nodes inside the NeMo container via `srun --mpi=pmix`. Exercises cross-container PMIx world (v0.13.6) + IB device visibility inside the container (v0.13.8 `MELLANOX_VISIBLE_DEVICES=all` enroot hook). Unlike §2 the sbatch sets **no** NCCL env itself: the NCCL/UCX/IB vars (`NCCL_IB_HCA`, `NCCL_TOPO_FILE`, `UCX_NET_DEVICES`, `MELLANOX_VISIBLE_DEVICES=all`) are injected inside the container by `/etc/enroot/environ.d/50-nccl.env`, and `/opt/microsoft` is bind-mounted by `/etc/enroot/mounts.d/50-azcluster.fstab` so `NCCL_TOPO_FILE` resolves.

```bash
#!/bin/bash -l
#SBATCH --job-name=nccl-N10-container
#SBATCH --output=nccl-N10-container-%j.out
#SBATCH --partition=gpu
#SBATCH --nodes=2
#SBATCH --ntasks-per-node=8
#SBATCH --gpus-per-node=8
#SBATCH --exclusive
#SBATCH --time=00:50:00

# Containerised mirror of nccl-N10.sbatch: identical all_reduce_perf -b 16G -e 16G -N 10,
# run inside the NeMo container via Pyxis instead of bare-metal HPC-X.
NEMO_IMAGE=${NEMO_IMAGE:-nvcr.io/nvidia/nemo:25.07.02}

srun --mpi=pmix \
  --container-image="${NEMO_IMAGE}" \
  --container-mounts=/shared:/shared,/mnt/nvme:/mnt/nvme \
  --no-container-mount-home \
  --export=ALL,NCCL_DEBUG=INFO \
  bash -c 'set -e; BIN=$(command -v all_reduce_perf_mpi || command -v all_reduce_perf || echo /usr/local/bin/all_reduce_perf_mpi); [ "${SLURM_PROCID:-0}" = "0" ] && echo "[rank0] using $BIN on $(hostname)"; exec "$BIN" -b 16G -e 16G -N 10 -g 1'
```

Submit (warm the container first via 3a so the import is seconds):

```bash
# Optional: warm the squashfs cache + container sanity (first import ~25 min)
azcluster exec <name> --user clusteradmin -- "sbatch /shared/examples/dgxc-nemo-container-smoke.sbatch"
# Then the consistent measured run:
azcluster scp <name> --user clusteradmin nccl-N10-container.sbatch :/shared/home/clusteradmin/
azcluster exec <name> --user clusteradmin -- "sbatch nccl-N10-container.sbatch"
```

Expect `# Avg bus bandwidth :` within a few % of §2 (live `v2420walk` job 29: **451.08 GB/s** vs §2 job 28: 440.21 GB/s). The binary auto-resolves to `all_reduce_perf_mpi` (the MPI-launcher build NeMo ships); the `srun --mpi=pmix` launcher provides the PMIx world.

> The legacy Python multinode smoke (`dgxc-nemo-multinode-smoke.sbatch`, shipped at `/shared/examples/`) still works as a quick 2-node functional check and produces ~425-435 GB/s on its 1 GiB payload, but it is **not** apples-to-apples with §2 — prefer `nccl-N10-container.sbatch` for the §8 chart.

### 4. Storage pipeline — small model (Llama 3.1 8B FP8)

Goal: validate `hf download` → NVMe → `azcp copy` → blob → `azcp-cluster` → both nodes' NVMe, end-to-end.

Script `llama-pipeline.sbatch`:

```bash
#!/bin/bash -l
#SBATCH --job-name=llama-pipe
#SBATCH --output=/shared/home/clusteradmin/llama-pipe-%j.out
#SBATCH --nodes=1
#SBATCH --ntasks-per-node=1
#SBATCH --gpus-per-node=1
#SBATCH --time=00:30:00
set -euo pipefail
date -u

# One-time setup: install python3.12-venv on each compute node:
#   srun -N <num_compute_nodes> --ntasks-per-node=1 \
#        sudo apt-get install -y -o DPkg::Lock::Timeout=600 python3.12-venv

mkdir -p /mnt/nvme/users/${USER}/models
if [ ! -d /mnt/nvme/users/${USER}/hfvenv ]; then
  python3 -m venv /mnt/nvme/users/${USER}/hfvenv
fi
source /mnt/nvme/users/${USER}/hfvenv/bin/activate
pip install -q -U huggingface_hub hf_transfer
export HF_HUB_ENABLE_HF_TRANSFER=1

# HF_TOKEN is OPTIONAL for public models like this one but REQUIRED
# for gated models (Llama gated, Qwen3.5-FP8 gated, etc).
# If you need it:  export HF_TOKEN=$(cat /shared/home/${USER}/.hf-token)
# Never bake the token into this sbatch script.

cd /mnt/nvme/users/${USER}/models
MODEL=neuralmagic/Meta-Llama-3.1-8B-Instruct-FP8
time hf download $MODEL --local-dir llama-3.1-8b-fp8
du -sh llama-3.1-8b-fp8

# Stage 2: upload to per-cluster blob (env vars come from /etc/profile.d/azcluster-storage.sh)
SRC=/mnt/nvme/users/${USER}/models/llama-3.1-8b-fp8/
DST=${AZCLUSTER_USER_BLOB_URL}/models/llama-3.1-8b-fp8/
time azcp copy "$SRC" "$DST" --recursive
date -u
```

Then broadcast to every compute node via `azcp-cluster` (uses the shipped template at `/shared/examples/azcp-cluster-distribute-sqsh.sbatch`, but the template is sqsh-pathed; for models we use an inline sbatch):

```bash
#!/bin/bash -l
#SBATCH --job-name=azcp-dist-llama
#SBATCH --output=/shared/home/clusteradmin/azcp-dist-llama-%j.out
#SBATCH --partition=gpu --nodes=2 --ntasks-per-node=1 --time=00:20:00 --gpus-per-node=8 --exclusive
set -euo pipefail
date -u
SRC=${AZCLUSTER_USER_BLOB_URL}/models/llama-3.1-8b-fp8/
DST=${AZCLUSTER_USER_NVME}/models/llama-3.1-8b-fp8/
export UCX_TLS=rc,sm,self
export UCX_NET_DEVICES=mlx5_ib0:1
export OMPI_MCA_pml=ucx
export OMPI_MCA_osc=ucx
EXP='ALL,AZURE_CLIENT_ID,AZCLUSTER_USER_BLOB_URL,AZCLUSTER_USER_NVME,UCX_TLS,UCX_NET_DEVICES,OMPI_MCA_pml,OMPI_MCA_osc'
srun --mpi=pmix --export=$EXP \
  --container-image=docker://ghcr.io/edwardsp/azcp/azcp-cluster:v0.4.5 \
  --container-mounts=/mnt/nvme:/mnt/nvme \
  taskset -c 0-47 \
  azcp-cluster "$SRC" "$DST" \
    --concurrency 32 --block-size 16777216 \
    --bcast-chunk 67108864 --bcast-pipeline 128 --bcast-writers 8 \
    --compare size --no-progress
date -u
```

### 5. Llama-3.1-8B FP8 inference (single node, vLLM + InferenceX bench)

Clone InferenceX once:

```bash
azcluster exec <name> --user clusteradmin -- \
  "git clone --depth 1 https://github.com/SemiAnalysisAI/InferenceX.git /shared/dgxc/InferenceX"
```

`inferencex-llama.sbatch`:

```bash
#!/bin/bash -l
#SBATCH --job-name=infmax-llama
#SBATCH --output=/shared/home/clusteradmin/infmax-llama-%j.out
#SBATCH --partition=gpu --nodes=1 --ntasks-per-node=1 --gpus-per-node=8 --exclusive --time=01:00:00
set -euo pipefail
date -u

export MODEL=/models/llama-3.1-8b-fp8
export TP=1
export CONC=128
export ISL=1000
export OSL=1000
export RANDOM_RANGE_RATIO=0.2
export RESULT_FILENAME=llama-3.1-8b-fp8-tp1-c128
export PORT=8888
export HF_HUB_CACHE=/tmp/hf-cache

srun --mpi=pmix \
  --container-image=docker://vllm/vllm-openai:latest \
  --container-mounts=/mnt/nvme/users/${USER}/models:/models,/shared/dgxc/InferenceX:/workspace \
  --container-workdir=/workspace \
  --no-container-mount-home \
  --no-container-entrypoint \
  --export=ALL,MODEL,TP,CONC,ISL,OSL,RANDOM_RANGE_RATIO,RESULT_FILENAME,PORT,HF_HUB_CACHE \
  bash /workspace/benchmarks/single_node/gptoss_fp4_h100.sh
date -u
```

`gptoss_fp4_h100.sh` in InferenceX is model-agnostic (`vllm serve` auto-detects FP8 from the model's quantization config) — the harness drives `vllm serve` + `benchmark_serving.py`, producing TTFT / TPOT / ITL / E2EL percentiles + per-second GPU metrics CSV.

### 6. DeepSeek-R1-0528 FP8 multinode (SGLang TP=16)

Same storage pipeline at production scale, then SGLang two-node serve.

`dsr1-pipeline.sbatch` (HF download + azcp upload — identical to Llama but with `MODEL=deepseek-ai/DeepSeek-R1-0528` and `--local-dir dsr1-fp8`). Then broadcast via the same `azcp-cluster` sbatch with `dsr1-fp8` paths substituted.

After broadcast, the SGLang serve+bench needs a runner script (`run-dsr1.sh`, placed in `/shared/dgxc/InferenceX/`):

```bash
#!/bin/bash
set -eo pipefail

MODEL=/models/dsr1-fp8
TP=16
CONC=64
ISL=1024
OSL=1024
RANDOM_RANGE_RATIO=0.2
RESULT_FILENAME=dsr1-fp8-h100-tp16-c64
PORT=8888

# Head node IP must be reachable from both ranks. Get it from Slurm:
# Head node IP — hardcoded to the first compute node's IP. Set this once
# after deploy; the same value works for every job on this cluster.
HEAD_IP=10.42.4.5
NODE_RANK=${SLURM_NODEID:-${SLURM_PROCID:-0}}
echo "node-rank=$NODE_RANK host=$(hostname) head=$HEAD_IP"

mkdir -p /workspace/logs

python3 -m sglang.launch_server \
  --model-path $MODEL \
  --host 0.0.0.0 --port $PORT \
  --tp $TP --nnodes 2 --node-rank $NODE_RANK \
  --dist-init-addr ${HEAD_IP}:5000 \
  --mem-fraction-static 0.85 \
  --chunked-prefill-size 8192 \
  --max-running-requests $CONC \
  --trust-remote-code \
  --quantization fp8 --kv-cache-dtype fp8_e4m3 \
  --attention-backend flashinfer \
  > /workspace/logs/server-rank${NODE_RANK}.log 2>&1 &

SERVER_PID=$!

if [ "$NODE_RANK" = "0" ]; then
  source /workspace/benchmarks/benchmark_lib.sh
  start_gpu_monitor
  wait_for_server_ready --port $PORT --server-log /workspace/logs/server-rank0.log --server-pid $SERVER_PID
  pip install -q datasets pandas
  run_benchmark_serving \
    --model $MODEL --port $PORT --backend sglang \
    --input-len $ISL --output-len $OSL \
    --random-range-ratio $RANDOM_RANGE_RATIO \
    --num-prompts $((CONC*10)) --max-concurrency $CONC \
    --result-filename $RESULT_FILENAME --result-dir /workspace/
  stop_gpu_monitor
  # SIGTERM (default `kill`) then `wait` lets sglang shut down gracefully so the
  # sbatch exits 0. Using `kill -9` propagates 137 (128+SIGKILL) up through the
  # srun -> sbatch chain and sacct reports FAILED even though the bench succeeded.
  kill $SERVER_PID 2>/dev/null || true
  wait $SERVER_PID 2>/dev/null || true
else
  wait $SERVER_PID
fi
```

Wrapper `infmax-dsr1.sbatch`:

```bash
#!/bin/bash -l
#SBATCH --job-name=infmax-dsr1
#SBATCH --output=/shared/home/clusteradmin/infmax-dsr1-%j.out
#SBATCH --partition=gpu --nodes=2 --ntasks-per-node=1 --gpus-per-node=8 --exclusive --time=02:00:00
set -euo pipefail
date -u

# DO NOT use --export=ALL,HEAD_IP — that double-exports HEAD_IP through both
# the sbatch environment and srun's per-task env, which under pmix concatenates
# the two values with an embedded newline. The result is dist_init_addr
# becomes 'IP\nIP:5000' and torch.distributed fails with gai error.
#
# Simplest fix: hardcode HEAD_IP inside run-dsr1.sh (which is what the
# v24walk2 working baseline did). On a fresh deploy, find the first compute
# node's IP via:
#   azcluster exec <name> --user clusteradmin -- "scontrol show hostnames '<pool>' | head -1 | xargs getent hosts"
# Then edit run-dsr1.sh's HEAD_IP=... line accordingly.

srun --mpi=pmix \
  --container-image=docker://lmsysorg/sglang:v0.5.8-cu130 \
  --container-mounts=/mnt/nvme/users/${USER}/models:/models,/shared/dgxc/InferenceX:/workspace \
  --container-workdir=/workspace \
  --no-container-mount-home \
  --no-container-entrypoint \
  /workspace/run-dsr1.sh
date -u
```

### 7. DGXC Llama 3.1 8B BF16 pretraining (NVIDIA `dgxc-benchmarking`, `llmb-run`)

This is the one **training** run in the walkthrough. Everything above is inference or a micro-benchmark; here we run a real data-parallel pretraining step loop and measure strong-scaling efficiency of the gradient all-reduce over the IB fabric. Tool: NVIDIA's [`dgxc-benchmarking`](https://github.com/NVIDIA/dgxc-benchmarking) harness (`llmb-install` + `llmb-run`), which drives a NeMo / Megatron-Bridge container.

Unlike the earlier version of this run, the NeMo container is **not** pulled onto `/shared` by `llmb-install`. Instead we stage it as a squashfs through the same NVMe → blob → `azcp-cluster` pipeline as the inference runs above: build the `.sqsh` once on a compute node, publish to the per-cluster blob, broadcast to every node's NVMe over IB, then point `llmb-run` at the local squashfs via `RUN_CONF_IMAGE`. The container never touches `/shared`, so a fast `--shared-storage nfs-scheduler` deploy stays viable.

**Prerequisites:**

- Deploy with `--extra-package python3.12-venv` (the `llmb` installer builds a uv venv on login + compute).
- **Storage must be on** (the default — do NOT pass `--no-storage`). The `azcp-build-and-publish-sqsh.sbatch` + `azcp-cluster-distribute-sqsh.sbatch` templates only exist under `/shared/examples/` when the cluster has a blob account.
- `--shared-storage nfs-scheduler` is **fine** here (reversed from older guidance). The squashfs lives on per-node NVMe, not `/shared`; the only `llmb` footprint on `/shared` is the uv venv + recipe clone + dataset/config staging (a few GB). The old "17 GiB NeMo import ENOSPCs a 64 GiB scheduler-NFS export" warning no longer applies because we skip the install-time import (see 7a).
- NGC API key (NeMo container pull) AND a HuggingFace token (the `Meta-Llama-3.1` NeMo configs are gated). See the credentials table above.

#### 7a. One-time setup (login VM, as `clusteradmin`)

```bash
azcluster ssh <name> --user clusteradmin

# Stash credentials where the harness expects them (mode 0600).
mkdir -p "$HOME/.config/azcluster" && chmod 0700 "$HOME/.config/azcluster"
printf '%s' '<NGC_API_KEY>' > "$HOME/.config/azcluster/ngc_key"  && chmod 0600 "$HOME/.config/azcluster/ngc_key"
printf '%s' '<HF_TOKEN>'    > "$HOME/.config/azcluster/hf_token" && chmod 0600 "$HOME/.config/azcluster/hf_token"
export NGC_API_KEY="$(cat "$HOME/.config/azcluster/ngc_key")"
export HF_TOKEN="$(cat "$HOME/.config/azcluster/hf_token")"

# enroot needs NGC credentials to import the (gated, rate-limited) NeMo image in 7b.
# $HOME is on /shared, so this file is visible to the compute node that runs the import.
# Note the LITERAL `$oauthtoken` (NGC's required username); only NGC_API_KEY expands.
mkdir -p "$HOME/.config/enroot"
cat > "$HOME/.config/enroot/.credentials" <<EOF
machine nvcr.io login \$oauthtoken password ${NGC_API_KEY}
EOF
chmod 0600 "$HOME/.config/enroot/.credentials"

# Clone + build the llmb tooling into a uv venv on /shared (visible to all nodes).
mkdir -p "$HOME/dgxc" && cd "$HOME/dgxc"
git clone https://github.com/NVIDIA/dgxc-benchmarking.git
export LLMB_INSTALL="$HOME/dgxc/llmb"
python3 -m venv "$LLMB_INSTALL/llmb_venv"
. "$LLMB_INSTALL/llmb_venv/bin/activate"
pip install --upgrade pip uv
uv pip install ./dgxc-benchmarking/cli/llmb-install ./dgxc-benchmarking/cli/llmb-run
```

The recipe pins the NeMo container tag. Read it from the recipe rather than hardcoding — it changes between `dgxc-benchmarking` releases:

```bash
# llama3.1/launch.sh hardcodes e.g. FW_VERSION=26.04.00; llama3.1/metadata.yaml
# carries the matching `nvcr.io#nvidia/nemo:<tag>`. Confirm both agree.
FW_VERSION=$(sed -n 's/^FW_VERSION=//p' "$HOME/dgxc/dgxc-benchmarking/llama3.1/launch.sh" | head -1)
echo "recipe NeMo tag: ${FW_VERSION}"          # expect 26.04.00 for the v0.24.20-era recipe
NEMO_TAG="nvcr.io/nvidia/nemo:${FW_VERSION}"
SQSH_NAME="nemo-${FW_VERSION}"
```

Write the non-interactive install playfile. Note `account: clusteradmin` (the per-user Slurm account created at deploy time — NOT the old `default` account, which only exists for `azureuser`) and the default pool name `gpu` for both partitions:

```bash
cat > "$HOME/dgxc/playfile.yaml" <<'YAML'
venv_type: uv
gpu_type: h100
node_architecture: x86_64
install_method: slurm
account: clusteradmin
gpu_partition: gpu
cpu_partition: gpu
selected_workloads:
  - pretrain_llama3.1
YAML

# Sanity: confirm the account/partition assoc actually exists for clusteradmin.
sacctmgr -n show assoc user=clusteradmin format=account,partition

# Pre-place an empty placeholder at the path llmb-install would import the NeMo
# squashfs to. llmb-install skips the (17 GiB, /shared-bound) import when the
# target file already exists, so `express` only stages datasets + configs.
# We stage the real container as a per-node NVMe squashfs in 7b instead.
mkdir -p "$LLMB_INSTALL/images"
touch "$LLMB_INSTALL/images/nvidia+nemo+${FW_VERSION}.sqsh"

# Express install: stages datasets/configs (image import skipped by the placeholder).
llmb-install --play "$HOME/dgxc/playfile.yaml" express
```

#### 7b. Stage the NeMo container as a per-node squashfs (NVMe → blob → broadcast)

Same pipeline as the inference runs above. Build once on one compute node, publish to blob, then broadcast to every node's NVMe over IB. Both templates ship under `/shared/examples/`.

```bash
# (1) Import nvcr.io/nvidia/nemo:<tag> to /mnt/nvme/.../sqsh/<name>.sqsh on one
#     node and upload to the per-cluster blob. ~25-30 min, NGC-CDN bound (17 GiB).
build=$(sbatch --parsable /shared/examples/azcp-build-and-publish-sqsh.sbatch \
  "${SQSH_NAME}" "${NEMO_TAG}")
echo "build+publish job: ${build}"

# (2) Broadcast the published sqsh to every node's NVMe over IB once (1) succeeds.
#     ~seconds at N=2 (NVMe-read-bound; see the storage-pipeline note above).
dist=$(sbatch --parsable --dependency=afterok:${build} \
  /shared/examples/azcp-cluster-distribute-sqsh.sbatch "${SQSH_NAME}")
echo "distribute job: ${dist}"

# Watch both to completion.
watch -n 10 "squeue -u $USER"
```

After both jobs report `COMPLETED`, every compute node has the squashfs at
`${AZCLUSTER_USER_NVME}/sqsh/${SQSH_NAME}/${SQSH_NAME}.sqsh`
(= `/mnt/nvme/users/clusteradmin/sqsh/nemo-<tag>/nemo-<tag>.sqsh`).

#### 7c. Submit the strong-scaling pair

`RUN_CONF_IMAGE` overrides the recipe's default `$LLMB_INSTALL/images/...` path (the empty placeholder from 7a) with the per-node NVMe squashfs. `llmb-run` passes it verbatim to `srun --container-image=`, and Pyxis treats the leading-`/` path as a local squashfs — no per-job import.

```bash
. "$LLMB_INSTALL/llmb_venv/bin/activate"
export LLMB_INSTALL="$HOME/dgxc/llmb"
export RUN_CONF_IMAGE="${AZCLUSTER_USER_NVME}/sqsh/${SQSH_NAME}/${SQSH_NAME}.sqsh"
RUN_START=$(date -u +%Y-%m-%dT%H:%M:%SZ)

# Single node, 8 GPUs.
llmb-run submit -w pretrain_llama3.1 --model-size 8b -d bf16 --scale 8

# Both nodes, 16 GPUs.
llmb-run submit -w pretrain_llama3.1 --model-size 8b -d bf16 --scale 16

RUN_END=$(date -u +%Y-%m-%dT%H:%M:%SZ)
echo "training window: ${RUN_START} .. ${RUN_END}"
```

Per-step throughput is in the experiment logs:

```bash
ls "$LLMB_INSTALL"/workloads/pretrain_llama3.1/experiments/*/log-default-*_*.out
grep -E 'iteration .*/50|MODEL_TFLOP/s/GPU' \
  "$LLMB_INSTALL"/workloads/pretrain_llama3.1/experiments/*/log-default-*_*.out | tail
```

**Reference baseline** (v0.13.9, southafricanorth, 2× ND96isr_H100_v5 — replace with fresh numbers for this release):

| Scale | GPUs | GBS | Step time | Throughput | MODEL_TFLOP/s/GPU |
|---|---|---|---|---|---|
| `--scale 8`  | 8 (1 node)  | 128 | 12522.40 ms | 83,737 tok/s  | ~537 |
| `--scale 16` | 16 (2 node) | 256 | 12513.10 ms | 167,594 tok/s | ~538 |

Strong-scaling 8→16 GPU = **2.001×** (100.07% efficiency) — the BF16 gradient all-reduce is fully hidden behind compute on the IB fabric, so doubling the node count doubles throughput at constant per-GPU step time. That near-perfect number is the headline result of this run.

#### 7d. Observability

The GPU + InfiniBand dashboard shows the training signature distinct from inference: sustained `DCGM_FI_PROF_PIPE_TENSOR_ACTIVE` near 1.0 on all 16 GPUs plus periodic IB-receive bursts at every gradient all-reduce. Capture the `${RUN_START}`/`${RUN_END}` window above and feed it to the chart appendix (Appendix A `INFERENCE_PANELS` + the NCCL IB-recv panel) the same way as the inference runs; cross-check live in Grafana via the `## Observability` queries.

### 8. Verification — check chart panels actually have data

After every run, before declaring success, query AMW directly with `curl` + the management-scope token:

```bash
TOKEN=$(az account get-access-token --resource "https://prometheus.monitor.azure.com" --query accessToken -o tsv)
ENDPOINT=$(az rest --method GET \
  --uri "https://management.azure.com/subscriptions/${SUB}/resourceGroups/rg-azcluster-${NAME}/providers/Microsoft.Monitor/accounts/amw-${NAME}?api-version=2023-04-03" \
  -o json | jq -r .properties.metrics.prometheusQueryEndpoint)

# Sample: per-GPU temp during a run window
curl -sG "${ENDPOINT}/api/v1/query_range" \
  -H "Authorization: Bearer ${TOKEN}" \
  --data-urlencode 'query=DCGM_FI_DEV_GPU_TEMP' \
  --data-urlencode "start=${RUN_START_EPOCH}" \
  --data-urlencode "end=${RUN_END_EPOCH}" \
  --data-urlencode 'step=15' \
  | jq '.data.result | length'   # expect > 0
```

Returning `0` = blank chart (scrape interval too long for run window, or wrong metric name). Returning `16` (= 2 nodes × 8 GPUs) = good. Don't render a matplotlib PNG without first confirming this returns the expected series count.

**Matched-pair check for the NCCL charts.** Because §3b runs the *identical* `all_reduce_perf -b 16G -e 16G -N 10` as §2, the two NCCL charts (`nccl-plain-vm` and `nccl-container-multinode`) must come out as a true matched pair: same panel set (`NCCL_PANELS`), comparable series counts, and IB-receive spikes within a few percent of each other. If the container chart looks visibly emptier or its IB peak is an order of magnitude lower, the container run used a different payload (a small-tensor `torchrun` Python smoke rather than `all_reduce_perf`) — re-run §3b, not the Python helper. Live `v2420walk`: both charts rendered 52 series, container IB peak ~1230 Gbps vs plain-VM ~1270 Gbps, busbw 451.08 vs 440.21 GB/s.

### 9. Job-accounting capture

After every walkthrough run completes (before tearing down), capture the full Slurm accounting record. This goes at the end of every version-specific walkthrough doc so reviewers can verify the actual job timing and exit status without re-running.

```bash
azcluster exec <name> --user clusteradmin -- "sacct --starttime $(date -d '6 hours ago' +%Y-%m-%dT%H:%M:%S) --format=JobID,JobName%24,Partition,NodeList%30,Start,End,Elapsed,State,ExitCode -P"
```

`-P` produces pipe-delimited output that pastes cleanly into a markdown table. `--format` is explicit because the default `sacct` columns truncate names. `--starttime` is required since the default range is "today" which can miss late-evening runs.

Field meanings:

- `JobID` — Slurm job ID. Child step rows (`<jobid>.batch`, `<jobid>.extern`, `<jobid>.0`) appear underneath the parent and account for the actual srun work.
- `JobName` — first 24 chars of the SBATCH `--job-name`
- `NodeList` — exact compute nodes the job landed on (use this to correlate with per-node DCGM/IB metrics in Grafana)
- `Start` / `End` — UTC timestamps; use these as the time-range when querying AMW or rendering matplotlib charts
- `State` — `COMPLETED` is the only success
- `ExitCode` — `0:0` is success; first number is the highest exit code returned by `srun`, second is the killing signal

### 10. Tear-down

```bash
azcluster delete <name>
```

Async; ~10 min to fully reap the RG and release H100 capacity. **Run this as soon as the walkthrough is captured** — leaving 2× ND96isr_H100_v5 idle is expensive.

---

## Appendix A — Chart generation script

The PNGs in the version-specific walkthroughs are matplotlib renders of PromQL queries against the cluster's AMW. The same data is queryable live in Grafana for the cluster's lifetime; the PNGs exist so the walkthrough doc stays useful after the cluster is gone.

Save as `plot-walkthrough.py`. Requires `matplotlib`, `python3` ≥ 3.8, and `az` CLI logged in.

```python
#!/usr/bin/env python3
"""
Render walkthrough charts from AMW PromQL queries.

Usage:
  CLUSTER=v249walk OUT=doc/full-walkthrough-slurm-v0.24.9 python3 plot-walkthrough.py

Required env:
  CLUSTER   azcluster name (used to find AMW + RG)
  OUT       output directory for PNGs

The script queries AMW for each run window, verifies the result has data,
and only then renders a chart. If a query returns zero series, it prints
a WARNING and skips that panel so the operator notices.
"""
import os, json, subprocess, urllib.parse, urllib.request
from datetime import datetime, timezone
import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
import matplotlib.dates as mdates

CLUSTER = os.environ["CLUSTER"]
OUT     = os.environ["OUT"]
os.makedirs(OUT, exist_ok=True)

# Resolve AMW Prometheus query endpoint via ARM
amw = json.loads(subprocess.check_output([
    "az","rest","--method","GET","--uri",
    f"https://management.azure.com/subscriptions/{subprocess.check_output(['az','account','show','--query','id','-o','tsv']).decode().strip()}/resourceGroups/rg-azcluster-{CLUSTER}/providers/Microsoft.Monitor/accounts/amw-{CLUSTER}?api-version=2023-04-03",
    "-o","json"
]).decode())
QUERY_URL = amw["properties"]["metrics"]["prometheusQueryEndpoint"] + "/api/v1/query_range"

# Mint a Prometheus-scoped access token
TOKEN = subprocess.check_output([
    "az","account","get-access-token",
    "--resource","https://prometheus.monitor.azure.com",
    "--query","accessToken","-o","tsv"
]).decode().strip()

def query(promql, start, end, step=15):
    url = f"{QUERY_URL}?{urllib.parse.urlencode({'query':promql,'start':start,'end':end,'step':step})}"
    req = urllib.request.Request(url, headers={"Authorization": f"Bearer {TOKEN}"})
    return json.loads(urllib.request.urlopen(req).read())["data"]["result"]

def plot_run(slug, title, start_dt, end_dt, panels):
    """Render a multi-panel chart. Each panel is (panel_title, promql, ylabel)."""
    start = int(start_dt.timestamp())
    end   = int(end_dt.timestamp())

    # Verify each query has data BEFORE rendering. Abort with warning if any panel is empty.
    panel_data = []
    for ptitle, promql, ylabel in panels:
        result = query(promql, start, end)
        if not result:
            print(f"  WARNING [{slug}] panel '{ptitle}' returned 0 series for query: {promql}")
        panel_data.append((ptitle, promql, ylabel, result))

    if all(not r for _,_,_,r in panel_data):
        print(f"  SKIPPING {slug} — every panel empty (run window may be wrong)")
        return

    nrows = len(panels)
    fig, axes = plt.subplots(nrows, 1, figsize=(14, 3.5*nrows), sharex=True)
    if nrows == 1:
        axes = [axes]
    fig.suptitle(title, fontsize=12)

    for ax, (ptitle, _, ylabel, result) in zip(axes, panel_data):
        if not result:
            ax.text(0.5, 0.5, "(no data)", ha="center", va="center", transform=ax.transAxes)
        for sr in result:
            m = sr["metric"]
            label = m.get("nodename", m.get("device", "?"))
            if "gpu" in m: label = f"{label}/gpu{m['gpu']}"
            ts = [datetime.fromtimestamp(float(t), tz=timezone.utc) for t,_ in sr["values"]]
            vs = [float(v) for _,v in sr["values"]]
            ax.plot(ts, vs, "-", linewidth=1, label=label, alpha=0.7)
        ax.set_ylabel(ylabel)
        ax.set_title(ptitle)
        ax.grid(True, alpha=0.3)
        if len(ax.get_lines()) <= 20 and result:
            ax.legend(loc="upper right", fontsize=6, ncol=4)
        ax.xaxis.set_major_formatter(mdates.DateFormatter("%H:%M:%S", tz=timezone.utc))

    fig.autofmt_xdate()
    plt.tight_layout()
    fp = os.path.join(OUT, f"{slug}.png")
    plt.savefig(fp, dpi=110, bbox_inches="tight")
    plt.close()
    print(f"  WROTE {fp}")


# Reference panel sets — adjust the time windows to match each run's actual
# start/end (from `date -u +%Y-%m-%dT%H:%M:%SZ` recorded in the sbatch).

NCCL_PANELS = [
    ("Per-GPU die temperature", "DCGM_FI_DEV_GPU_TEMP", "°C"),
    ("Aggregate IB receive per node (sum of 8 NICs)",
     "sum by (nodename) (rate(node_infiniband_port_data_received_bytes_total[1m])*8/1e9)", "Gbps"),
    ("Per-GPU power", "DCGM_FI_DEV_POWER_USAGE", "W"),
    ("DCGM PIPE_TENSOR_ACTIVE", "DCGM_FI_PROF_PIPE_TENSOR_ACTIVE", "ratio"),
]

INFERENCE_PANELS = [
    ("Per-GPU power", "DCGM_FI_DEV_POWER_USAGE", "W"),
    ("Per-GPU die temperature", "DCGM_FI_DEV_GPU_TEMP", "°C"),
    ("PIPE_TENSOR_ACTIVE (tensor-core busy ratio)",
     "DCGM_FI_PROF_PIPE_TENSOR_ACTIVE", "ratio"),
    ("SM_ACTIVE (warp occupancy)",
     "DCGM_FI_PROF_SM_ACTIVE", "ratio"),
]

# Example invocation — replace timestamps with what you recorded
if __name__ == "__main__":
    # NCCL plain VM (§2)
    plot_run("nccl-plain-vm",
        f"NCCL all-reduce 16 GiB x N=10 on plain VM, 16 ranks across 2 nodes — {CLUSTER}",
        datetime(2026,5,29,0,0,tzinfo=timezone.utc),   # REPLACE with date -u from sbatch
        datetime(2026,5,29,0,5,tzinfo=timezone.utc),
        NCCL_PANELS)

    # NCCL containerised (§3b) — SAME panel set as plain VM so the two charts
    # form a true matched pair (see §8). Only the run window differs; the
    # all_reduce_perf payload is identical (-b 16G -e 16G -N 10).
    plot_run("nccl-container-multinode",
        f"NCCL all-reduce 16 GiB x N=10 in NeMo container, 16 ranks across 2 nodes — {CLUSTER}",
        datetime(2026,5,29,0,0,tzinfo=timezone.utc),   # REPLACE with date -u from sbatch
        datetime(2026,5,29,0,6,tzinfo=timezone.utc),
        NCCL_PANELS)

    # ... repeat plot_run() per run window ...
```

Recommended usage in the walkthrough:

1. Capture `date -u +%Y-%m-%dT%H:%M:%SZ` immediately before each `sbatch` AND immediately after the run completes — paste both timestamps into the script.
2. Run `CLUSTER=<name> OUT=doc/full-walkthrough-vX.Y.Z python3 plot-walkthrough.py` — script tells you per-panel whether AMW returned data.
3. If a panel is empty: widen the time window, check the metric name, or check that DCGM/node-exporter on the relevant node is actually running. Don't ship a chart with blank panels.

Add `look_at` (or `feh`/`xdg-open`/scp-to-laptop-and-check) on the produced PNG to visually verify each chart before checking it into the walkthrough doc.
