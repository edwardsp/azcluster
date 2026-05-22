# walkthrough-dgxc

Running NVIDIA's [dgxc-benchmarking](https://github.com/NVIDIA/dgxc-benchmarking) recipes on an azcluster-deployed Azure NDv5 H100 cluster. Two tiers:

1. **Infra smoke test** (~10 minutes, no NGC creds) — `/shared/examples/dgxc-nemo-container-smoke.sbatch`, validates the full Pyxis → NVMe → NCCL-in-container path by running NCCL all-reduce across 8 H100 from inside the production DGXC NeMo container.
2. **Full DGXC `llmb-run` flow** — install the DGXC toolchain on the scheduler, register an NGC API key, run any of the supported recipes (Llama 3.1 8B/70B, NeMo Megatron, NCCL benchmarks, GPT-OSS, ...).

The smoke test is the recommended first run after `azcluster deploy`. The full flow is for sustained benchmarking work.

---

## Prereqs

- A live azcluster cluster ≥ v0.13.5 with at least one `Standard_ND96isr_H100_v5` node available. Example:
  ```bash
  azcluster deploy \
    --name dgxc \
    --location southafricanorth \
    --resource-group paul-azcluster-dgxc \
    --pool name=cpu,sku=Standard_D8as_v5,count=1,default \
    --pool name=gpu,sku=Standard_ND96isr_H100_v5,count=1 \
    --login-public-ip
  ```
- v0.13.5 wires three pieces of DGXC compatibility into every compute node automatically:
  - `/etc/enroot/enroot.conf` with `ENROOT_REMAP_ROOT yes`,
  - `/etc/enroot/environ.d/50-nccl.env` (propagates `NCCL_IB_HCA`, `NCCL_TOPO_FILE`, `UCX_NET_DEVICES`, ... into the container at start),
  - `/etc/enroot/mounts.d/50-azcluster.fstab` (bind-mounts `/opt/microsoft` so `NCCL_TOPO_FILE` resolves inside the container).
- v0.13.5 also auto-builds **`/mnt/nvme`** as a RAID-0 ext4 across all `Microsoft NVMe Direct Disk(s)` on the SKU (~28 TB usable on ND96isr_H100_v5). Enroot extracts into this filesystem, so the first container import of a ~20 GB image completes in tens of seconds, not minutes.

---

## Tier 1 — infra smoke test (no NGC creds)

The example sbatch is dropped by the scheduler at `/shared/examples/dgxc-nemo-container-smoke.sbatch`. It uses `nvcr.io/nvidia/nemo:25.07.02` (public, anonymous pull) and runs a 1 GiB NCCL all-reduce across all 8 H100 on the node, from inside the container. No NeMo / Megatron-Bridge recipe API is touched — purely PyTorch + NCCL — so it survives container version churn.

```bash
azcluster ssh dgxc
sbatch /shared/examples/dgxc-nemo-container-smoke.sbatch
squeue
# When state goes from PD → R, tail the log:
tail -f dgxc-nemo-smoke-<jobid>.out
```

What you should see in the log, in order:

1. `pyxis: importing docker image: nvcr.io#nvidia/nemo:25.07.02` (first run only, lands on `/mnt/nvme/enroot-data/...`).
2. `pyxis: imported docker image: nvcr.io/nvidia/nemo:25.07.02`
3. `rank 0 / world 8 on device 0 (NVIDIA H100 80GB HBM3)` (one line per rank).
4. `[NCCL INFO] Channel ...` lines that mention `IBext_v11` and `mlx5_ib0:1`, ..., `mlx5_ib7:1`. **If you see `Channel ... via SOCKET`, NCCL fell back to TCP — check the `/etc/enroot/environ.d/50-nccl.env` propagation.**
5. Final summary: `all_reduce size=1GiB iters=20 elapsed=... algbw=... avg busbw=... GB/s`.

A healthy run on a single NDv5 H100 (8x NVLink within the node, NCCL using SHARP/NVLS) should report avg busbw well above 100 GB/s. Cross-node would show similar bandwidth, but cross-node containerised collectives are blocked today by the PMIx limitation below.

### What this smoke test exercises

| Surface | How |
|---|---|
| NVMe RAID-0 (`/mnt/nvme`) | Enroot caches + extracts the ~20 GB container onto it. |
| Pyxis container launch | `srun --container-image=nvcr.io/nvidia/nemo:25.07.02` |
| Enroot `environ.d` | NCCL/UCX env vars appear inside the container. |
| Enroot `mounts.d` | `/opt/microsoft/ndv5-topo.xml` visible inside the container so `NCCL_TOPO_FILE` resolves. |
| NCCL all-reduce across 8 H100 in-container | `torch.distributed.all_reduce` of a 1 GiB fp16 tensor, 20 iters. |

---

## Tier 2 — full `llmb-run` flow

DGXC ships its own driver (`llmb-run`) that automates: NeMo container builds, dataset prep, multi-scale sweeps (8 GPU → 1024 GPU), FP8/NVFP4/BF16 variants, and result aggregation. It requires an NGC API key (free at `ngc.nvidia.com`).

### One-time setup on the scheduler

```bash
azcluster ssh dgxc --scheduler
sudo mkdir -p /shared/dgxc && sudo chown azureuser:azureuser /shared/dgxc
cd /shared/dgxc
git clone https://github.com/NVIDIA/dgxc-benchmarking.git
git clone https://github.com/NVIDIA/llmb-install.git
```

### Register your NGC API key

```bash
# Paste your NGC API key when prompted (starts with "nvapi-")
ngc config set
# Or env-only, no interactive prompt:
export NGC_API_KEY="nvapi-..."
```

### Install the LLM Benchmarking toolchain

```bash
cd /shared/dgxc/llmb-install
export LLMB_INSTALL=/shared/dgxc/llmb        # canonical install root
mkdir -p "$LLMB_INSTALL"
./install.sh                                  # ~30-60 min: builds NeMo container, pulls datasets
```

`LLMB_INSTALL` MUST live on `/shared` (or another cluster-wide filesystem) — compute nodes need read access to the container image and dataset.

### Run Llama 3.1 8B at 8 GPUs (single node)

```bash
cd /shared/dgxc/dgxc-benchmarking/llama3.1
export LLMB_INSTALL=/shared/dgxc/llmb
export GPU_TYPE=h100
export JOB_TOTAL_GPUS=8
export MODEL_SIZE=8b
export DTYPE=fp8                              # H100 supports fp8 (cs only) or bf16
export SBATCH_ACCOUNT=default
export SBATCH_PARTITION=gpu
./launch.sh
```

`launch.sh` calls Megatron-Bridge's `setup_experiment.py`, which generates a NeMo-Run sbatch and submits it. Output lands under `$LLMB_INSTALL/workloads/pretrain_llama3.1/`.

DGXC's H100 BF16 table covers Llama 3.1 8B at the **8-128 GPU** range. On a 16-GPU cluster you can run 8 GPU (single node, works today) and — once the PMIx blocker below is resolved — 16 GPU (cross-node).

---

## The multi-node containerised PMIx blocker

Slurm 25.11 from `packages.microsoft.com/repos/slurm-ubuntu-noble` only ships `mpi_pmix_v4.so`, linked against PMIx 4.2.9 (`libpmix.so.2.9.5`). Most current AI/ML containers (NVIDIA NGC 25.x, the DGXC NeMo container, `ai-infrastructure-on-azure/nccl-test`) ship PMIx 5.x (`libpmix.so.2.13.x`).

When you run `srun --container-image=... --mpi=pmix ...` across more than one node, MPI_Init does NOT abort — but each container instance ends up in its own single-rank world. Cross-node collective bandwidth collapses (`busbw≈0`) and DGXC perf metrics are meaningless.

This affects:
- Any DGXC recipe with `JOB_TOTAL_GPUS > 8` on this cluster (Llama 8B 16-128, Llama 70B all scales, Nemotron, ...).
- The Pyxis-containerised version of the multi-node NCCL all-reduce.

**What still works today on multi-node:**
- The bare-metal NCCL all-reduce in `/shared/examples/nccl-allreduce.sbatch`. It uses HPC-X (in-image, PMIx 4.x compatible) and the prebuilt `/opt/nccl-tests/build/all_reduce_perf`. Live-validated at 466.33 GB/s peak / 348.02 GB/s avg busbw on 2× H100.

**Workarounds being evaluated for v0.14+:**
1. Rebuild Slurm 25.11 with PMIx 5 (`--with-pmix=/path/to/pmix5`) and publish `slurm-smd-mpi-pmix-v5`.
2. Repackage DGXC containers against PMIx 4. Not all containers cooperate.
3. Switch DGXC's launch path from `srun --mpi=pmix` to `mpirun` over SSH inside the container. Requires sshd inside the container, which the upstream NVIDIA images don't ship.
4. `LD_LIBRARY_PATH` bind-mounting host PMIx 4 over container PMIx 5 — confirmed NOT working (symbol mismatch still produces isolated worlds).

The smoke test above is a 1-node job and is unaffected by this blocker.

---

## Tear-down

```bash
azcluster delete dgxc
```

Then verify the RG is gone:

```bash
az group show -n paul-azcluster-dgxc -o table 2>/dev/null || echo "deleted"
```

NVMe RAID-0 is ephemeral — deallocating compute nodes destroys `/mnt/nvme` contents. Anything you want to keep (NeMo container, datasets, checkpoints) MUST live on `/shared` (NFS/ANF).
