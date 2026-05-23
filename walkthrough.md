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

The job should:

- Complete with `#wrong = 0` on every row of the size sweep (8 MiB → 16 GiB).
- Log `NVLS multicast support is available`, `Initialized NET plugin IBext_v11`, `NET/IB : Using [0]mlx5_ib0:1/IB/SHARP ... [7]mlx5_ib7:1/IB/SHARP`, and `Loaded net plugin NCCL RDMA Plugin v11` (from HPC-X's `nccl_rdma_sharp_plugin`) in the `NCCL_DEBUG=INFO` output. Those confirm NCCL picked up the NDv5 topology file and is using NVLink SHARP intra-node + IB SHARP RDMA inter-node on all 8 NDR400 NICs.

If `NCCL_DEBUG=INFO` shows `NET/Socket` channels instead, NCCL fell back to TCP. Re-check that:

- `/opt/microsoft/ndv5-topo.xml` exists on every node (`ls -l /opt/microsoft/ndv5-topo.xml`).
- All 8 IB devices report `State: Active LinkUp Rate: 400` (`ibstat | grep -E '(CA |State|Rate)'`).
- The job actually ran on 2 different nodes (`grep "Hostname" nccl-allreduce-*.out` should show two distinct names).

azcluster does not publish a bare-metal busbw target — `all_reduce_perf` numbers depend on NCCL version, image patch level, IB tuning, and message-size sweep, and we don't run a qualified bandwidth-acceptance baseline. Treat the `NCCL_DEBUG` signals above as the pass/fail criterion.

## 5. Pyxis container path (cross-node)

Pyxis lets you replace bare-metal with `srun --container-image=docker://...`. The end-to-end containerised path (Pyxis import → enroot extract → cross-node PMIx world → NCCL over InfiniBand) is live-validated since v0.13.8.

`/shared/examples/dgxc-nemo-multinode-smoke.sbatch` runs a 16-rank NCCL all-reduce from inside `nvcr.io/nvidia/nemo:25.07.02`:

```bash
sbatch /shared/examples/dgxc-nemo-multinode-smoke.sbatch
```

Healthy log shape (search the `.out`):

- `pyxis: imported docker image: nvcr.io/nvidia/nemo:25.07.02` on every compute node (first run only — subsequent runs hit the `/mnt/nvme/enroot-data/` cache and start in seconds).
- `NCCL INFO NET/IB : Using [0]mlx5_ib0:1/IB/SHARP [1]mlx5_ib1:1/IB/SHARP ... [7]mlx5_ib7:1/IB/SHARP` on every rank — all 8 NDR400 HCAs visible and in SHARP mode inside the container.
- Cross-node channels via `NET/IBext_v10/0/GDRDMA` (GPUDirect RDMA between H100s on different nodes).
- Final summary line `all_reduce size=1GiB iters=20 elapsed=...s algbw=... avg busbw=... GB/s`. On the v0.13.8 validation run (`paul-azcluster-h100d`, 16 ranks, 1 GiB message) this reported `avg busbw=434.40 GB/s`.

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
