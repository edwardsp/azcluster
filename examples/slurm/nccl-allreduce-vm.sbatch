#!/bin/bash -l
#
# Slurm-only bare-metal NCCL baseline: 2 nodes × 8 GPUs, no container.
#
# Run:
#   azcluster scp <cluster> examples/slurm/nccl-allreduce-vm.sbatch :/shared/home/clusteradmin/
#   azcluster exec <cluster> --user clusteradmin -- "sbatch nccl-allreduce-vm.sbatch"
#
# This is the plain-VM reference for comparing Pyxis/AKS container overhead.
# Expect `# Avg bus bandwidth :` in the ~440-466 GB/s band on 2× ND H100/H200.
#SBATCH --job-name=nccl-vm-N10
#SBATCH --output=nccl-vm-N10-%j.out
#SBATCH --partition=gpu
#SBATCH --nodes=2
#SBATCH --ntasks-per-node=8
#SBATCH --gpus-per-node=8
#SBATCH --exclusive
#SBATCH --time=00:30:00

set -euo pipefail

HPCX_DIR=$(ls -d /opt/hpcx-*-gcc-doca_ofed-ubuntu24.04-cuda*-x86_64 2>/dev/null | head -1)
[ -n "${HPCX_DIR}" ] || { echo "HPC-X not found under /opt" >&2; exit 1; }
# HPC-X's hpcx-init.sh references unbound vars (e.g. HPCX_ENABLE_NCCL_MRC_PLUGIN);
# relax `set -u` around sourcing it.
set +u
source "${HPCX_DIR}/hpcx-init.sh"
hpcx_load
set -u

export NCCL_DEBUG=INFO
export NCCL_IB_HCA=mlx5_ib
export NCCL_TOPO_FILE=/opt/microsoft/ndv5-topo.xml
export UCX_NET_DEVICES=mlx5_ib0:1,mlx5_ib1:1,mlx5_ib2:1,mlx5_ib3:1,mlx5_ib4:1,mlx5_ib5:1,mlx5_ib6:1,mlx5_ib7:1

srun --mpi=pmix /opt/nccl-tests/build/all_reduce_perf -b 16G -e 16G -N 10 -g 1
