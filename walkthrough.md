# Walkthrough: Deploy a 2-node NDv5 Slurm cluster and run NCCL all-reduce

This walks through the full happy path:

1. Install the CLI.
2. Deploy a 2-node `Standard_ND96isr_H100_v5` cluster on `southafricanorth`.
3. Run a 16-GPU NCCL all-reduce across both nodes over 8x NDR400 InfiniBand.
4. Interpret the result.
5. Tear the cluster down.

It was validated end-to-end against azcluster v0.13.4 on 2026-05-22. Wall-clock for a fresh deploy: ~54 minutes (ANF + monitoring + accounting all on).

---

## 0. Prerequisites

- Azure CLI logged in: `az login`
- Subscription with NDv5 quota in your chosen region (`az vm list-usage -l southafricanorth -o table | grep ND`)
- `jq`, SSH key (`~/.ssh/id_ed25519.pub`), and permissions to create RGs + role assignments + Monitor/Grafana resources
- ~$60/hr of NDv5 capacity for the duration of the test (delete the RG as soon as you're done)

## 1. Install the CLI

```bash
VERSION=v0.13.4
ARCH=x86_64-linux        # or aarch64-darwin
curl -fsSL -o azcluster \
  https://github.com/edwardsp/azcluster/releases/download/${VERSION}/azcluster-cli-${ARCH}
chmod +x azcluster && sudo mv azcluster /usr/local/bin/
azcluster version
```

## 2. Deploy the cluster

One CLI invocation provisions everything: VNet + subnets, NAT Gateway, scheduler VM, login VM (with public IP for SSH ingress), 2-node H100 VMSS Flex, ANF volume mounted at `/shared`, Azure Monitor Workspace + Managed Grafana + RBAC + dashboards, and Azure Database for MySQL Flex + `slurmdbd` for job accounting.

```bash
azcluster deploy \
  --name h100test \
  --location southafricanorth \
  --grafana-location uksouth \
  --resource-group paul-azcluster-h100test \
  --pool name=gpu,sku=Standard_ND96isr_H100_v5,count=2,default \
  --anf-size-tib 4 --anf-tier Premium \
  --login-public-ip
```

What happens, roughly:

| Stage | Wall-clock |
|---|---|
| ARM deployment (VNet, NICs, ANF, AMW, AMG, MySQL Flex, VMSS Flex create) | ~25 min |
| Cloud-init on scheduler (`slurmctld` + accounting + Prometheus) | ~6 min |
| Cloud-init on each compute node (drivers already in image; Slurm + Pyxis + Enroot + NCCL config) | ~8 min in parallel |
| AMG dashboard import (after RBAC propagates) | ~3 min, retried |

The CLI tails the ARM deployment and prints a final summary with the login public IP, scheduler private IP, and Grafana URL.

```bash
azcluster status h100test
azcluster timings h100test    # per-resource breakdown of the deploy you just did
```

## 3. Sanity check

```bash
azcluster ssh h100test                 # SSH to login VM
sinfo                                  # both nodes Idle, partition=gpu
srun -N2 --gres=gpu:8 nvidia-smi -L    # 16 H100 GPUs visible across both nodes
```

If `sinfo` shows nodes in state `INVAL` (invalid registration), see the troubleshooting section at the bottom.

## 4. Bare-metal NCCL all-reduce (HPC-X path)

The marketplace image (`microsoft-dsvm:ubuntu-hpc:2404`) ships HPC-X 2.25.1 and prebuilt `nccl-tests` at `/opt/nccl-tests/build/`. Run them directly on the host (no container) — this is the fastest, most reliable path on Azure today.

A ready-to-use sbatch is dropped at `/shared/examples/nccl-allreduce.sbatch` by every scheduler bootstrap. From the login VM:

```bash
sbatch /shared/examples/nccl-allreduce.sbatch
squeue
# wait for the job to finish (~1-2 min)
cat nccl-allreduce-*.out | tail -40
```

The sbatch body (for reference):

```bash
#!/usr/bin/env bash
#SBATCH --job-name=nccl-allreduce
#SBATCH --output=nccl-allreduce-%j.out
#SBATCH --nodes=2
#SBATCH --ntasks-per-node=8
#SBATCH --gpus-per-node=8
#SBATCH --exclusive

HPCX_DIR=$(ls -d /opt/hpcx-*-gcc-doca_ofed-ubuntu24.04-cuda*-x86_64 | head -1)
source "${HPCX_DIR}/hpcx-init.sh"
hpcx_load

export NCCL_DEBUG=INFO
export NCCL_IB_HCA=mlx5_ib
export NCCL_TOPO_FILE=/opt/microsoft/ndv5-topo.xml
export UCX_NET_DEVICES=mlx5_ib0:1,mlx5_ib1:1,mlx5_ib2:1,mlx5_ib3:1,mlx5_ib4:1,mlx5_ib5:1,mlx5_ib6:1,mlx5_ib7:1

srun --mpi=pmix /opt/nccl-tests/build/all_reduce_perf -b 8M -e 16G -f 2 -g 1
```

### What "good" looks like

Actual output from the v0.13.4 validation run (job 6 on `paul-azcluster-h100a`, 2x `Standard_ND96isr_H100_v5`, `southafricanorth`, 2026-05-22):

```
# nccl-tests version 2.18.3 nccl-headers=22907 nccl-library=22907
# Collective test starting: all_reduce_perf
# nThread 1 nGpus 1 minBytes 8388608 maxBytes 17179869184 step: 2(factor) warmup iters: 1 iters: 20
# Using devices
#  Rank  0 .. Rank  7 on h100a-gpu-0001  NVIDIA H100 80GB HBM3
#  Rank  8 .. Rank 15 on h100a-gpu-0002  NVIDIA H100 80GB HBM3
#                                                              out-of-place                       in-place
#       size         count      type   redop    root     time   algbw   busbw  #wrong     time   algbw   busbw  #wrong
#        (B)    (elements)                               (us)  (GB/s)  (GB/s)             (us)  (GB/s)  (GB/s)
     8388608       2097152     float     sum      -1   175.69   47.75   89.52       0   150.32   55.80  104.63       0
    16777216       4194304     float     sum      -1   200.32   83.75  157.04       0   201.43   83.29  156.17       0
    33554432       8388608     float     sum      -1   274.33  122.31  229.34       0   274.66  122.17  229.06       0
    67108864      16777216     float     sum      -1   480.83  139.57  261.69       0   476.16  140.94  264.26       0
   134217728      33554432     float     sum      -1   759.11  176.81  331.52       0   763.14  175.87  329.77       0
   268435456      67108864     float     sum      -1  1304.40  205.79  385.86       0  1314.20  204.26  382.98       0
   536870912     134217728     float     sum      -1  2387.39  224.88  421.65       0  2387.93  224.83  421.55       0
  1073741824     268435456     float     sum      -1  4531.91  236.93  444.24       0  4537.36  236.64  443.71       0
  2147483648     536870912     float     sum      -1  8813.94  243.65  456.84       0  8830.41  243.19  455.98       0
  4294967296    1073741824     float     sum      -1  17394.4  246.92  462.97       0  17371.9  247.24  463.57       0
  8589934592    2147483648     float     sum      -1  34613.7  248.17  465.31       0  34896.1  246.16  461.55       0
 17179869184    4294967296     float     sum      -1  69076.8  248.71  466.33       0  68981.5  249.05  466.97       0
# Out of bounds values : 0 OK
# Avg bus bandwidth    : 348.02
# Collective test concluded: all_reduce_perf
```

Key numbers to look for:

- **Peak busbw at the largest message size (16 GiB)**: 466.33 GB/s on this run.
- **Avg busbw across all message sizes**: 348.02 GB/s on this run.
- `#wrong` must be `0` for every row.
- `NCCL_DEBUG=INFO` output (in the job stderr) should mention `NVLS multicast support is available`, `Initialized NET plugin IBext_v11`, `NET/IB : Using [0]mlx5_ib0:1/IB/SHARP ... [7]mlx5_ib7:1/IB/SHARP`, and `Loaded net plugin NCCL RDMA Plugin v11` (from HPC-X's `nccl_rdma_sharp_plugin`). Those confirm NCCL picked up the NDv5 topology file and is using NVLink SHARP intra-node + IB SHARP RDMA inter-node on all 8 NDR400 NICs.

If you see avg busbw well below the figures above, NCCL probably fell back to TCP. Re-check that:

- `/opt/microsoft/ndv5-topo.xml` exists on every node (`ls -l /opt/microsoft/ndv5-topo.xml`).
- All 8 IB devices report `State: Active LinkUp Rate: 400` (`ibstat | grep -E '(CA |State|Rate)'`).
- The job actually ran on 2 different nodes (`grep "Hostname" nccl-allreduce-*.out` should show two distinct names).

## 5. Pyxis container path (currently broken cross-node)

Pyxis lets you replace bare-metal with `srun --container-image=docker://...`. The container import itself works — the `ghcr.io/azure/ai-infrastructure-on-azure/nccl-test:latest` image (9.4 GB) pulls and extracts onto every compute node successfully. Single-node container runs that don't need cross-node MPI also work. Single-node 8-GPU `all_reduce_perf` inside the container was not measured against this image in v0.13.4 because the container ships the binary at `/usr/local/bin/all_reduce_perf`, not the `/opt/nccl-tests/build/...` path used elsewhere — your invocation must use the in-container path.

**Cross-node containerised NCCL is currently broken** because of a PMIx ABI mismatch:

- `ghcr.io/azure/ai-infrastructure-on-azure/nccl-test:latest` ships PMIx 5.x (`libpmix.so.2.13.3`).
- Slurm 25.11 from `packages.microsoft.com/repos/slurm-ubuntu-noble` only ships `mpi_pmix_v4.so`, linked against PMIx 4.2.9.
- HPC-X 2.25.1 (in the image) also bundles PMIx 4.2.x.

The observed failure mode on the v0.13.4 validation cluster (jobs 9-11): `srun --container-image=... --mpi=pmix` produces two isolated single-node MPI worlds. `all_reduce_perf` runs to completion on each node independently but reports `# Avg bus bandwidth : 0` because the ranks never actually exchange data. Tracked for v0.14; the bare-metal path above is the workaround for now.

## 6. Inspect the job in Grafana and accounting

```bash
azcluster monitor h100test          # prints the Grafana URL
azcluster ssh h100test
sacct -j <jobid> --format=JobID,JobName,Partition,Account,State,Elapsed,AllocTRES%50
```

`sacct` should show `gres/gpu=16` in `AllocTRES` and `State=COMPLETED`. The Grafana "GPU + IB" dashboard should show 8 NDR400 NICs at line rate on both nodes during the all-reduce.

## 7. Tear down

NDv5 is expensive. Delete the RG as soon as you're done:

```bash
azcluster delete h100test
# or, equivalently:
az group delete --name paul-azcluster-h100test --yes --no-wait
```

---

## Troubleshooting

### GPU nodes show `state=INVAL` in `sinfo`

Slurm caches the first registration. If a compute node registers with bad Gres/Parameters once (e.g. before a patched cloud-init), restarting slurmd alone won't help. From the scheduler:

```bash
scontrol update nodename=<node> state=DOWN reason="reset"
scontrol delete nodename=<node>
```

Then `systemctl restart slurmd` on the compute node and wait ~10s for re-registration.

### `srun` from login fails with `Dlopen of plugin file failed`

You're hitting the Pyxis spank loader without the shared library. Confirm `/opt/pyxis/spank_pyxis.so` exists on the **login** VM (not just scheduler/compute). The bootstrap installs it everywhere it needs to be, but a half-applied rolling change can leave one node behind.

### Login VM client config is stale after a server-side `slurm.conf` change

The login VM runs `sackd` (Slurm Authentication and Configuration daemon) to fetch configless configs. After any change to `slurm.conf` on the scheduler:

```bash
sudo systemctl restart sackd
```

then re-run your command on the login VM.

### `sacct` returns nothing despite accounting being on

Check that `slurmdbd` is healthy on the scheduler and that the cluster registered:

```bash
azcluster ssh h100test --scheduler
sudo systemctl status slurmdbd
sacctmgr show cluster
sacctmgr show account
```

If the cluster is missing, re-run:

```bash
sudo sacctmgr -i add cluster $(hostname -s | sed 's/-scheduler$//')
sudo sacctmgr -i add account default Description="Default account" Organization=azcluster
sudo sacctmgr -i add user azureuser DefaultAccount=default
```
