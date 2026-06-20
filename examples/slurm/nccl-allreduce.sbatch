#!/bin/bash -l
#
# Containerized NCCL: Slurm/Pyxis mirror of examples/aks/nccl-allreduce-mpijob.yaml.
#
# Matched payload:
#   image: ghcr.io/azure/ai-infrastructure-on-azure/nccl-test:latest
#   ranks: 2 nodes × 8 GPUs = 16
#   command: all_reduce_perf_mpi -b 16G -e 16G -f 2 -g 1 -N 10
#
# Run:
#   azcluster scp <cluster> examples/slurm/nccl-allreduce.sbatch :/shared/home/clusteradmin/
#   azcluster exec <cluster> --user clusteradmin -- "sbatch nccl-allreduce.sbatch"
#
# Pass criteria: avg busbw ≥ 400 GB/s, `NET/IB : Using ... /IB/SHARP`, no TCP fallback.
#SBATCH --job-name=nccl-container-N10
#SBATCH --output=nccl-container-N10-%j.out
#SBATCH --partition=gpu
#SBATCH --nodes=2
#SBATCH --ntasks-per-node=8
#SBATCH --gpus-per-node=8
#SBATCH --exclusive
#SBATCH --time=00:50:00

set -euo pipefail

IMAGE=${IMAGE:-docker://ghcr.io/azure/ai-infrastructure-on-azure/nccl-test:latest}

srun --mpi=pmix \
  --container-image="${IMAGE}" \
  --container-mounts=/shared:/shared,/mnt/nvme:/mnt/nvme \
  --no-container-mount-home \
  --export=ALL,NCCL_DEBUG=INFO \
  bash -lc 'set -euo pipefail; BIN=$(command -v all_reduce_perf_mpi || echo /usr/local/bin/all_reduce_perf_mpi); [ "${SLURM_PROCID:-0}" = "0" ] && echo "[rank0] using ${BIN} on $(hostname)"; exec "${BIN}" -b 16G -e 16G -f 2 -g 1 -N 10'
