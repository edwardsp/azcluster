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

A healthy run on a single NDv5 H100 (8x NVLink within the node, NCCL using SHARP/NVLS) should report avg busbw well above 100 GB/s. The companion multinode sbatch (`/shared/examples/dgxc-nemo-multinode-smoke.sbatch`) exercises the same code on 2 nodes (16 ranks) over the 8x NDR400 InfiniBand fabric and reaches ~430 GB/s avg busbw at 1 GiB with SHARP + GPUDirect RDMA enabled.

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

### Install the LLM Benchmarking toolchain (headless `--play`)

DGXC v25.11 ships `llmb-install` as a Python package inside `dgxc-benchmarking/cli/llmb-install`. Use a `playfile.yaml` to drive it non-interactively:

```bash
export LLMB_INSTALL=/shared/dgxc/llmb
mkdir -p "$LLMB_INSTALL"

# Provision a venv with uv and install the CLI
python3 -m venv "$LLMB_INSTALL/llmb_venv"
"$LLMB_INSTALL/llmb_venv/bin/pip" install uv
"$LLMB_INSTALL/llmb_venv/bin/uv" pip install \
  /shared/dgxc/dgxc-benchmarking/cli/llmb-install \
  /shared/dgxc/dgxc-benchmarking/cli/llmb-run

# Headless playfile (selects pretrain_llama3.1, h100, slurm)
cat > /shared/dgxc/playfile.yaml <<EOF
venv_type: uv
gpu_type: h100
node_architecture: x86_64
install_method: slurm
account: default
gpu_partition: gpu
cpu_partition: gpu
selected_workloads:
  - pretrain_llama3.1
EOF

# NGC + HF tokens (gated Meta-Llama-3.1 configs require HF_TOKEN)
mkdir -p ~/.config/azcluster
chmod 700 ~/.config/azcluster
# paste tokens into these files (mode 600)
echo "<your nvapi-... key>" > ~/.config/azcluster/ngc_key
echo "<your hf_... token>" > ~/.config/azcluster/hf_token
chmod 600 ~/.config/azcluster/{ngc_key,hf_token}

export NGC_API_KEY=$(cat ~/.config/azcluster/ngc_key)
export HF_TOKEN=$(cat ~/.config/azcluster/hf_token)

"$LLMB_INSTALL/llmb_venv/bin/llmb-install" --play /shared/dgxc/playfile.yaml express
```

`express` mode skips datacenter sanity checks. Plan ~10-15 min: NeMo `nvcr.io#nvidia/nemo:26.04.00` (~20 GB) imports onto `/shared`, Megatron-Bridge + NeMo-Run are cloned, HF Llama configs are downloaded.

`LLMB_INSTALL` MUST live on `/shared` (or another cluster-wide filesystem) — compute nodes need read access to the container image and dataset.

> **Storage sizing.** The NeMo container squashfs is 17 GiB. With `--shared-storage nfs-scheduler` (test mode) the scheduler exports `/shared` from its 64 GiB root disk and enroot extraction will ENOSPC mid-import. Use `--shared-storage anf` (default) or attach a data disk to the scheduler. Hard requirement: ≥ 60 GiB free on `/shared` before running `llmb-install express`.

### Run Llama 3.1 8B via `llmb-run`

`llmb-install express` registers the workload + venv + container path in `$LLMB_INSTALL/cluster_config.yaml`. Use `llmb-run submit` from then on — it materialises the sbatch via NeMo-Run and submits with the correct partition + account:

```bash
export LLMB_INSTALL=/shared/dgxc/llmb
export NGC_API_KEY=$(cat ~/.config/azcluster/ngc_key)
export HF_TOKEN=$(cat ~/.config/azcluster/hf_token)

# 8 GPU single node (Tier-1 of the H100 BF16 table)
$LLMB_INSTALL/llmb_venv/bin/llmb-run submit \
  -w pretrain_llama3.1 --model-size 8b -d bf16 --scale 8

# 16 GPU two nodes
$LLMB_INSTALL/llmb_venv/bin/llmb-run submit \
  -w pretrain_llama3.1 --model-size 8b -d bf16 --scale 16
```

`squeue` shows the job; logs land under `$LLMB_INSTALL/workloads/pretrain_llama3.1/experiments/.../log-default-*_<jobid>_*.out`. NeMo prints one `iteration N/50` line per step with `elapsed time per iteration (ms)` and `MODEL_TFLOP/s/GPU` — that's the throughput signal you want.

### Tier-2 results (live-validated v0.13.9, southafricanorth, ND96isr_H100_v5)

| Scale | Nodes | GBS | Steady step (ms) | Throughput (tok/s) | Per-GPU (tok/s) | MODEL_TFLOP/s/GPU |
|---|---|---|---|---|---|---|
| 8 GPU  | 1 | 128 | 12522.40 | 83,737  | 10,467 | ~537 |
| 16 GPU | 2 | 256 | 12513.10 | 167,594 | 10,475 | ~538 |

Strong scaling 8→16 GPU = **2.001× → 100.07% efficiency**. Cross-node throughput matches single-node throughput per GPU because the cluster's 8× NDR400 IB + SHARP + GPUDirect RDMA absorb the all-reduce overhead at this model size (Llama 3.1 8B fits inside one node's HBM at TP=1 PP=1 CP=2 MBS=1, so the cross-node traffic is purely data-parallel gradient reduction over IB). NeMo's `MODEL_TFLOP/s/GPU` ~538 corresponds to ~54% MFU vs H100 BF16 peak (989 TFLOPS).

> **/etc/hosts gotcha** (fixed in v0.13.9). On v0.13.8 and earlier, compute nodes had `127.0.1.1 <hostname>` in `/etc/hosts` (Ubuntu cloud-image default). PyTorch/Gloo's `connectFullMesh` calls `gethostbyname(hostname)` for the rendezvous address; every remote rank then tried to dial its own loopback and stalled with `Connection refused, remote=[127.0.1.1]:...`. v0.13.9 maps the hostname to the eth0 IPv4 instead, in `cloud-init/compute.yaml.tmpl`.

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
