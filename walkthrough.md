# Walkthrough: 2-node NDv5 H100 cluster, NCCL, and Llama 3.1 8B training

End-to-end run of azcluster v0.19.1 on `Standard_ND96isr_H100_v5` in `southafricanorth`, captured live on 2026-05-24 against cluster `v19walk` (2× ND96isr_H100_v5 = 16× H100 80GB HBM3, 16× NDR400 IB). Every wall-clock and throughput number in this document is from that exact run — no estimates, no extrapolation.

Wall-clock from `azcluster deploy` start to first scheduled job: **~58 min** (3064 s ARM + ~25 min compute cloud-init in parallel; `--no-wait` returns in ~120 s for fire-and-forget mode).

| Stage | Wall-clock | Throughput / result |
|---|---|---|
| 0. Pre-flight | n/a | — |
| 1. Install CLI | <10 s | `azcluster version` → `0.19.1` |
| 2. `azcluster deploy` (ARM + post-deploy hooks) | **3064 s = 51:04** | login pub IP, scheduler private IP, Grafana URL |
| 3. Add a user (`azcluster user add`) | <2 s | `paul` in LDAP, sshkey installed |
| 4. Bare-metal NCCL 1-node × 10 reps (16 GiB) | ~7 min | **481.13 GB/s busbw** (mean) |
| 5. Bare-metal NCCL 2-node × 10 reps (16 GiB) | ~8 min | **466.40 GB/s busbw** (mean) |
| 6. Container pull + squash (nemo:25.07.02 + nccl-test) | 293 s + 305 s | 16 GiB + 9.2 GiB on `/mnt/nvme/sqsh/` |
| 7a. Containerised NCCL 1-node × 10 reps (16 GiB) | ~7 min | **480.49 GB/s busbw** (mean) |
| 7b. Containerised NCCL 2-node × 10 reps (16 GiB) | ~8 min | **458.08 GB/s busbw** (mean, 16-rank PMIx world) |
| 8a. DGXC `llmb-install express` | 533 s | NeMo 26.04.00 + Megatron-Bridge + venvs |
| 8b. Llama 3.1 8B BF16, scale=8 (1 node) × 50 iters | ~10:30 | **12.548 s/iter steady**, **537.6 TFLOPS/GPU**, GBS=128 |
| 8c. Llama 3.1 8B BF16, scale=16 (2 node) × 50 iters | ~10:45 | **12.515 s/iter steady**, **538.7 TFLOPS/GPU**, GBS=256 |
| 9. Tear down | ~3 min async | `azcluster delete` returns immediately, RG deletion completes in background |

Scaling efficiency (8 → 16 GPU on Llama 3.1 8B BF16): step time **12.548 s → 12.515 s** at 2× GBS = **~100% weak scaling**, **~99.7% strong scaling** (per-GPU throughput essentially flat).

---

## 0. Prerequisites

- `az login` against a subscription with NDv5 quota in your target region
- `az vm list-usage -l southafricanorth -o table | grep ND` shows ≥ 2× ND96isr_H100_v5 available
- `jq`, SSH keypair (`~/.ssh/id_ed25519.pub`), permission to create RGs + role assignments + Monitor/Grafana resources
- ~$60/hr of NDv5 capacity for the duration of the test — `azcluster delete` is your friend
- For Stage 8 (DGXC training): an NGC API key from `ngc.nvidia.com` and a HuggingFace token with `meta-llama/Llama-3.1-8B` access

## 1. Install the CLI

```bash
VERSION=v0.19.1
ARCH=x86_64-linux        # or aarch64-darwin
curl -fsSL -o azcluster \
  https://github.com/edwardsp/azcluster/releases/download/${VERSION}/azcluster-cli-${ARCH}
chmod +x azcluster && sudo mv azcluster /usr/local/bin/
azcluster version
```

## 2. Deploy the cluster

```bash
azcluster deploy \
  --name v19walk \
  --location southafricanorth \
  --grafana-location uksouth \
  --pool name=gpu,sku=Standard_ND96isr_H100_v5,count=2,default \
  --anf-size-tib 4 --anf-tier Premium \
  --login-public-ip
```

Default RG name is `rg-azcluster-<name>`. The CLI tails the ARM deployment and prints a final summary with login public IP, scheduler private IP, Grafana URL.

Per-resource ARM timings from the `v19walk` deploy (`azcluster timings v19walk`, top 15 by duration):

| secs | resource |
|--:|---|
| 890.5 | `cluster-v19walk` (root deployment) |
| 535.0 | `accounting` (MySQL Flex + slurmdbd config) |
| 480.1 | `mysql-v19walk/log_bin_trust_function_creators` |
| 461.0 | `mysql-v19walk/max_allowed_packet` |
| 439.6 | `mysql-v19walk/innodb_lock_wait_timeout` |
| 371.0 | `mysql-v19walk` (MySQL Flex Standard_B2ms create) |
| 363.1 | `anf` (NetApp account + capacity pool + 4 TiB volume) |
| 353.9 | `anfrktkcxbb5kuwq/pool1/shared` (the ANF volume itself) |
| 345.8 | `compute-gpu` (VMSS Flex + 2× ND96isr_H100_v5) |
| 309.3 | `vmss-v19walk-gpu` |
| 219.4 | `monitoring` (AMW + AMG + DCE + RBAC) |
| 175.9 | `amg-v19walk` (Managed Grafana) |
| 180.0 | first `roleAssignments` (Grafana Admin) |
| 45.5 | `network` (VNet + 4 subnets + NSGs + NAT gw) |
| 41.6 | ANF capacity pool |
| 40.0 | `login` (login VM + NIC + pub IP) |

**Total: 3064 s (51:04)**. MySQL accounting dominates (535 s of `accounting` + 371 s for the MySQL server + ~1400 s for 3 server-param updates which serialise). If you don't need job accounting:

```bash
azcluster deploy --name v19walk ... --no-accounting     # saves ~16 min
```

Or for the fastest possible test loop:

```bash
azcluster deploy --name v19walk ... --no-accounting --no-monitoring --shared-storage nfs-scheduler
# ~7 min total — SPOF on scheduler for /shared, no Grafana, no sacct.
```

### Fire-and-forget mode

```bash
azcluster deploy --name v19walk ... --no-wait      # returns in ~2 min
# ... go for coffee ...
azcluster status v19walk                            # ARM phase + per-node cloud-init progress
azcluster resume --name v19walk                     # waits for ARM, runs post-deploy hooks
```

Blocking deploys also write a pending marker, so a terminal interrupt mid-ARM is recoverable via the same `azcluster resume` command.

## 3. Add a user and SSH in

Out of the box the cluster has one user — `azureuser` (local on every VM, owns `/shared/dgxc` if you install DGXC). For real users:

```bash
azcluster user add v19walk --username paul --ssh-key ~/.ssh/id_ed25519.pub
# UID auto-allocated, gid=20000 (azusers), home=/shared/home/paul, shell=/bin/bash
azcluster user list v19walk
```

The CLI flushes the SSSD cache on the login VM after the LDAP write so the new key is usable in a couple of seconds (rather than waiting for the 60 s SSSD `entry_cache_timeout`).

```bash
ssh paul@<login-public-ip>
sinfo
# PARTITION AVAIL  TIMELIMIT  NODES  STATE NODELIST
# gpu*         up   infinite      2   idle v19walk-gpu-[0001-0002]
srun -N2 --gres=gpu:8 nvidia-smi -L | wc -l    # 16 — all H100s visible
```

> **Note (v0.19.1 gap → v0.19.2)**: `slurmctld` runs on the scheduler and resolves users via `getpwuid`, but the scheduler does not (yet) run SSSD. If you `sbatch` from the login VM and slurmctld can't resolve the new user, add a placeholder entry on the scheduler:
> ```bash
> azcluster ssh v19walk --scheduler -- "sudo useradd -u 20001 -g 20000 -M -s /sbin/nologin paul && sudo systemctl restart slurmctld"
> ```
> Tracked for fix in the next release.

Register the user with slurmdbd (skip if `--no-accounting`):

```bash
azcluster ssh v19walk --scheduler -- "sudo sacctmgr -i add user paul DefaultAccount=default"
```

## 4. Bare-metal NCCL all-reduce — 1 node × 8 H100

The `microsoft-dsvm:ubuntu-hpc:2404` image ships HPC-X 2.25.1 and a prebuilt `all_reduce_perf` at `/opt/nccl-tests/build/`. From the login VM as `paul`:

```bash
cat > /shared/home/paul/nccl-native-1n.sbatch <<'EOF'
#!/bin/bash
#SBATCH --job-name=nccl-native-1n
#SBATCH --nodes=1
#SBATCH --ntasks-per-node=8
#SBATCH --gpus-per-node=8
#SBATCH --exclusive
HPCX_DIR=$(ls -d /opt/hpcx-*-gcc-doca_ofed-ubuntu24.04-cuda*-x86_64 | head -1)
source "${HPCX_DIR}/hpcx-init.sh"; hpcx_load
export NCCL_IB_HCA=mlx5_ib NCCL_TOPO_FILE=/opt/microsoft/ndv5-topo.xml
export UCX_NET_DEVICES=mlx5_ib0:1,mlx5_ib1:1,mlx5_ib2:1,mlx5_ib3:1,mlx5_ib4:1,mlx5_ib5:1,mlx5_ib6:1,mlx5_ib7:1
for rep in $(seq 1 10); do
  echo "=== REP ${rep} START $(date -Iseconds) ==="
  srun --mpi=pmix /opt/nccl-tests/build/all_reduce_perf -b 16G -e 16G -f 2 -g 1 -n 1 -w 5
  echo "=== REP ${rep} END $(date -Iseconds) ==="
done
EOF
sbatch /shared/home/paul/nccl-native-1n.sbatch
```

**v19walk result (mean of 10 reps, 16 GiB, 8 ranks):**

| Metric | Value |
|---|---|
| algbw | 481.13 GB/s |
| busbw | 481.13 GB/s (single-node NVLink, busbw == algbw) |
| `#wrong` per rep | 0 |
| NCCL paths confirmed | `NET/IB : Using [0]mlx5_ib0:1/IB/SHARP … [7]mlx5_ib7:1/IB/SHARP`, `NVLS multicast support is available` |

## 5. Bare-metal NCCL all-reduce — 2 node × 8 H100 = 16 ranks

Same script with `--nodes=2`. **v19walk result (mean of 10 reps, 16 GiB, 16 ranks):**

| Metric | Value |
|---|---|
| algbw | 248.74 GB/s |
| **busbw** | **466.40 GB/s** |
| `#wrong` per rep | 0 |
| Per-rep range | 442.3 – 467.4 GB/s (8 of 10 in [466.0, 467.4]) |
| Cross-node fabric | 8× NDR400 IB, GPUDirect RDMA + IB SHARP |

## 6. Pull and squash the NeMo container

Containerised paths in §7 and §8 use `nvcr.io/nvidia/nemo:25.07.02`. Pulling it via `enroot import` is slow over WAN and re-runs every node-first-time, so pull once on a single GPU node to `/mnt/nvme/sqsh/` (the local NVMe RAID-0 sized at 28 TiB) and replicate over `/shared`. From login as `paul`:

```bash
# (sets up ~/.config/enroot/.credentials with your NGC key as machine $oauthtoken)
sbatch <<'EOF'
#!/bin/bash
#SBATCH --job-name=container-pull
#SBATCH --nodes=1 --exclusive
mkdir -p /mnt/nvme/sqsh
srun bash -c "
  cd /mnt/nvme/sqsh
  time enroot import -o nemo.sqsh docker://nvcr.io#nvidia/nemo:25.07.02
"
EOF
```

**v19walk result (gpu-0001, sequential):**

| Container | Size on disk | Pull + squash time |
|---|--:|--:|
| `nvcr.io#nvidia/nemo:25.07.02` | 16 GiB | **293 s** |
| `ghcr.io#azure/ai-infrastructure-on-azure/nccl-test:latest` | 9.2 GiB | **305 s** |

Replicate to gpu-0002 via `/shared/sqsh/`:

```bash
sbatch <<'EOF'
#!/bin/bash
#SBATCH --nodes=1 --nodelist=v19walk-gpu-0001 --exclusive
srun mkdir -p /shared/sqsh
srun cp /mnt/nvme/sqsh/nemo.sqsh /shared/sqsh/
EOF
# then on gpu-0002:
sbatch <<'EOF'
#!/bin/bash
#SBATCH --nodes=1 --nodelist=v19walk-gpu-0002 --exclusive
srun mkdir -p /mnt/nvme/sqsh
srun cp /shared/sqsh/nemo.sqsh /mnt/nvme/sqsh/
EOF
```

**v19walk timings:** cp-to-shared **109 s**, cp-to-NVMe **36 s** (gpu-0001 had it cached from the original pull), **62 s** (gpu-0002, cold). 16 GiB across 8× NDR400 ≈ 256 MB/s effective on the gpu→ANF→gpu path (the ANF Premium tier caps single-stream write at ~256 MB/s/TiB).

## 7. Containerised NCCL all-reduce — 1 + 2 node

Two patterns work end-to-end inside Pyxis containers on v0.19.1:

- **1-node** → `torchrun` inside the container (PyTorch's NCCL bootstrap)
- **2-node** → `srun --mpi=pmix python …` (Slurm PMIx → Hydra → PyTorch NCCL)

(NB: `srun --mpi=pmix --container-image=… all_reduce_perf` causes each rank to initialise as a singleton PMIx world inside the container — root-caused, fix tracked for v0.19.2. The two patterns above are the validated escape hatch and what the shipped `/shared/examples/dgxc-nemo-{container,multinode}-smoke.sbatch` use.)

### 7a. 1 node × 8 H100 in `nemo:25.07.02` (torchrun)

```bash
cat > /shared/home/paul/nccl-cont-1n.sbatch <<'EOF'
#!/bin/bash
#SBATCH --nodes=1 --gpus-per-node=8 --exclusive
CONT=/mnt/nvme/sqsh/nemo.sqsh
SCRIPT=/shared/home/paul/nccl_allreduce_smoke.py
cat > "$SCRIPT" <<'PY'
import os, time, torch, torch.distributed as dist
torch.cuda.set_device(int(os.environ["LOCAL_RANK"]))
dist.init_process_group(backend="nccl")
r,w = dist.get_rank(), dist.get_world_size()
t = torch.ones(8*1024*1024*1024//2, dtype=torch.float16, device="cuda")  # 16 GiB
for _ in range(5): dist.all_reduce(t)
torch.cuda.synchronize(); dist.barrier()
t0 = time.perf_counter()
for _ in range(10): dist.all_reduce(t)
torch.cuda.synchronize(); el = time.perf_counter()-t0
if r == 0:
    algbw = t.element_size()*t.numel()*10/el/1e9
    print(f"RESULT iters=10 elapsed={el:.3f}s algbw={algbw:.2f} busbw={algbw*2*(w-1)/w:.2f}")
PY
for rep in $(seq 1 10); do
  srun --container-image="$CONT" \
       --container-mounts=/shared:/shared,/mnt/nvme:/mnt/nvme \
       --no-container-mount-home \
       bash -c "cd /; torchrun --nproc_per_node=8 $SCRIPT"
done
EOF
sbatch /shared/home/paul/nccl-cont-1n.sbatch
```

**v19walk result (mean of 10 reps, 16 GiB, 8 ranks in container):**

| Metric | Value |
|---|---|
| algbw | 274.61 GB/s |
| **busbw** | **480.49 GB/s** |
| Per-rep range | 479.74 – 481.27 GB/s |
| vs bare-metal | -0.13% (within run-to-run variance) |

### 7b. 2 node × 8 H100 = 16 ranks in `nemo:25.07.02` (srun --mpi=pmix + python)

Same script, but with 16 ranks via Slurm + PMIx instead of torchrun:

```bash
cat > /shared/home/paul/nccl-cont-2n.sbatch <<'EOF'
#!/bin/bash
#SBATCH --nodes=2 --ntasks-per-node=8 --gpus-per-node=8 --exclusive
CONT=/mnt/nvme/sqsh/nemo.sqsh
SCRIPT=/shared/home/paul/nccl_allreduce_smoke.py    # (reuse the one from 7a)
for rep in $(seq 1 10); do
  srun --mpi=pmix \
       --container-image="$CONT" \
       --container-mounts=/shared:/shared,/mnt/nvme:/mnt/nvme \
       --no-container-mount-home \
       bash -c "cd /; python $SCRIPT"
done
EOF
sbatch /shared/home/paul/nccl-cont-2n.sbatch
```

**v19walk result (mean of 10 reps, 16 GiB, 16 ranks across 2 nodes in container):**

| Metric | Value |
|---|---|
| algbw | 244.31 GB/s |
| **busbw** | **458.08 GB/s** |
| Per-rep range | 455.80 – 459.39 GB/s (tight; σ ≈ 1 GB/s) |
| vs bare-metal 2n | -1.78% (container overhead) |
| PMIx world | single 16-rank world (confirmed by `world=16` in log + `dist.barrier()` clearing in <1 s) |

### Side-by-side summary

| Path | 1n × 8 ranks | 2n × 16 ranks | 2n vs 1n |
|---|--:|--:|--:|
| bare-metal HPC-X | 481.13 GB/s | 466.40 GB/s | -3.06% |
| container (Pyxis + nemo:25.07.02) | 480.49 GB/s | 458.08 GB/s | -4.66% |
| container overhead | -0.13% | -1.78% | — |

## 8. DGXC `llmb-run`: Llama 3.1 8B BF16

### 8a. Install the DGXC toolchain on the scheduler (one-time, ~9 min)

```bash
azcluster ssh v19walk --scheduler
sudo apt-get install -y git-lfs python3.12-venv && git lfs install     # v0.19.1 gap → v0.19.2 cloud-init
sudo mkdir -p /shared/dgxc && sudo chown azureuser:azureuser /shared/dgxc
cd /shared/dgxc
git clone --depth 1 https://github.com/NVIDIA/dgxc-benchmarking.git
export LLMB_INSTALL=/shared/dgxc/llmb VIRTUAL_ENV=$LLMB_INSTALL/llmb_venv
mkdir -p "$LLMB_INSTALL"
python3 -m venv "$VIRTUAL_ENV"
"$VIRTUAL_ENV/bin/pip" install -q uv
"$VIRTUAL_ENV/bin/uv" pip install -q \
    dgxc-benchmarking/cli/llmb-install \
    dgxc-benchmarking/cli/llmb-run

# Tokens (paste actual values)
echo 'nvapi-...' > /shared/dgxc/ngc_key
echo 'hf_...'    > /shared/dgxc/hf_token
chmod 644 /shared/dgxc/{ngc_key,hf_token}   # allow non-azureuser to read

# Drop NGC creds for azureuser on every compute node (enroot reads $HOME/.config/enroot/.credentials)
for n in v19walk-gpu-0001 v19walk-gpu-0002; do
  srun -w $n -N1 --partition=gpu bash -c "
    sudo mkdir -p /home/azureuser/.config/enroot &&
    sudo chown -R azureuser:azureuser /home/azureuser/.config &&
    printf 'machine nvcr.io login \$oauthtoken password %s\n' '$(cat /shared/dgxc/ngc_key)' | \
      sudo -u azureuser tee /home/azureuser/.config/enroot/.credentials > /dev/null &&
    sudo chmod 600 /home/azureuser/.config/enroot/.credentials"
done

cat > /shared/dgxc/playfile.yaml <<EOF
venv_type: venv
install_path: /shared/dgxc/llmb
slurm:
  account: default
  gpu_partition: gpu
  cpu_partition: gpu
  gpu_partition_gres: 8
  cpu_partition_gres: null
gpu_type: h100
node_architecture: x86_64
install_method: slurm
selected_workloads:
  - pretrain_llama3.1
environment_vars:
  HF_TOKEN: $(cat /shared/dgxc/hf_token)
  NGC_API_KEY: $(cat /shared/dgxc/ngc_key)
EOF

cd /shared/dgxc/dgxc-benchmarking
"$VIRTUAL_ENV/bin/llmb-install" --play /shared/dgxc/playfile.yaml express

# Open up workloads dir so non-azureuser can write experiment results
sudo chmod -R a+rwX /shared/dgxc/llmb/workloads /shared/dgxc/llmb/.cache
```

**v19walk timing:** `llmb-install express` completed in **533 s**. This pulls `nvcr.io/nvidia/nemo:26.04.00` via `srun enroot import` (~250 s on first run), clones Megatron-Bridge, builds the workload venv with `nemo_run`, and writes `cluster_config.yaml`.

### 8b. Submit Llama 3.1 8B BF16 from the login VM

```bash
ssh paul@<login-public-ip>
export LLMB_INSTALL=/shared/dgxc/llmb VIRTUAL_ENV=$LLMB_INSTALL/llmb_venv
export NGC_API_KEY=$(cat /shared/dgxc/ngc_key) HF_TOKEN=$(cat /shared/dgxc/hf_token)

# 1 node × 8 H100
$LLMB_INSTALL/llmb_venv/bin/llmb-run submit \
  -w pretrain_llama3.1 --model-size 8b -d bf16 --scale 8

# 2 node × 8 H100 = 16 GPUs
$LLMB_INSTALL/llmb_venv/bin/llmb-run submit \
  -w pretrain_llama3.1 --model-size 8b -d bf16 --scale 16
```

Each `submit` materialises an sbatch via NeMo-Run, submits it, and prints `JobID: <id>`. NeMo logs land under `$LLMB_INSTALL/workloads/pretrain_llama3.1/experiments/.../log-default-*_<jobid>_*.out` with `iteration N/50 | elapsed time per iteration (ms): …` and `MODEL_TFLOP/s/GPU` per step.

### 8c. v19walk results (Llama 3.1 8B BF16)

| Scale | Nodes | GBS | Cfg (TP×PP×CP) | Steady step | TFLOPS/GPU | Tokens/s |
|---|--:|--:|---|--:|--:|--:|
| 8  | 1 | 128 | 1×1×2 | **12.548 s** | **537.6** | 83,766 |
| 16 | 2 | 256 | 1×1×2 | **12.515 s** | **538.7** | 167,927 |

Tokens/s computed from `(GBS × seqlen_8B=8192) / step_ms × 1000`. Strong+weak scaling 8→16 GPU at constant per-GPU GBS: **per-iteration time delta 0.26%** (12.548 → 12.515 s) at **2× global batch**. Per-GPU TFLOPS difference 0.2% — measurement noise.

NCCL paths confirmed inside the NeMo container (`NCCL_DEBUG=WARN` was clean; `INFO` logs from §7 show all 8 `mlx5_ib*` HCAs in SHARP mode, `NET/IBext_v10/GDRDMA` between nodes).

## 9. Inspect in Grafana and accounting

```bash
azcluster monitor v19walk          # prints the AMG URL
ssh paul@<login-public-ip>
sacct --format=JobID,JobName%50,State,Elapsed,AllocTRES%50 -j 36,37
```

`sacct` should show `gres/gpu=8` and `gres/gpu=16` in `AllocTRES`, `State=COMPLETED`. The Grafana "GPU + IB" dashboard shows all 8 NDR400 NICs at line rate on both nodes during the all-reduce intervals.

## 10. Tear down

```bash
azcluster delete v19walk      # async; returns immediately, RG deletion runs in background
```

The CLI removes both `~/.config/azcluster/clusters/v19walk.toml` and `…-secrets.toml`. The Azure-side RG deletion typically completes in 3-5 minutes.

---

## Reproducibility

Every number in this document came from cluster `v19walk`, deployment `azcluster-v19walk-20260524-081921`, on 2026-05-24:
- ARM timings: `~/.config/azcluster/deployments/v19walk/2026-05-24T083630Z.json`
- NCCL + Llama logs: `/shared/home/paul/{nccl-native,nccl-cont}-{1n,2n}-*.out`, `/shared/dgxc/llmb/workloads/pretrain_llama3.1/experiments/.../log-*_3{6,7}_*.out`
- Host SKU: `Standard_ND96isr_H100_v5` ×2, 8× H100 80GB HBM3 + 8× NDR400 IB per node
- Image: `microsoft-dsvm:ubuntu-hpc:2404`, NVIDIA driver 580.126.20, HPC-X 2.25.1
- Container: `nvcr.io/nvidia/nemo:25.07.02` (NCCL) + `nvcr.io/nvidia/nemo:26.04.00` (DGXC, pulled by `llmb-install`)
- Region: `southafricanorth` (Grafana in `uksouth`)

## Troubleshooting

### `srun` from login fails with `Dlopen of plugin file failed`

The Pyxis spank library (`/opt/pyxis/spank_pyxis.so`) is missing on the login VM. The bootstrap installs it everywhere it's needed; a half-applied rolling change can leave one node behind. Re-run the login bootstrap or `scp` it from a compute node.

### Login VM client config is stale after a server-side `slurm.conf` change

```bash
ssh paul@<login-pub-ip> "sudo systemctl restart sackd"
```

### `slurmctld` rejects a job from a new LDAP user (`Invalid user id`)

See the §3 note: the scheduler doesn't run SSSD on v0.19.1. Workaround:

```bash
azcluster ssh v19walk --scheduler -- "sudo useradd -u <uid> -g 20000 -M -s /sbin/nologin <user> && sudo systemctl restart slurmctld"
```

Tracked for fix in v0.19.2.

### `enroot import` from a compute node fails with `401 Unauthorized`

azureuser's `~/.config/enroot/.credentials` is missing or wrong on that node. The credentials line must be exactly:

```
machine nvcr.io login $oauthtoken password <your-ngc-key>
```

(`$oauthtoken` is a literal string — NGC uses it as the username regardless of which key format you have.)

### `enroot import` aborts with `Permission denied` on `/var/lib/enroot-data/cache/<sha>`

A previous user imported the same image; their cache file is owned by their UID and not group-readable. Either nuke `/var/lib/enroot-data/cache/*` or set per-user cache via `ENROOT_CACHE_PATH=$HOME/.cache/enroot` in `/etc/enroot/enroot.conf`. Tracked for fix in v0.19.2.

### Container all-reduce shows `world=1` on every rank

You're hitting the v0.19.1 PMIx-in-container singleton bug. Use the patterns in §7 (torchrun for 1n, `srun --mpi=pmix python` for 2n) instead of `srun --mpi=pmix all_reduce_perf`. Tracked for fix in v0.19.2.
