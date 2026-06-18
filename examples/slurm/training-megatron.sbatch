#!/bin/bash -l
#
# Megatron-Bridge Llama-3.1-8B mock-data training benchmark.
# Slurm/Pyxis mirror of examples/aks/training-megatron-pytorchjob.yaml.
#
# Prereq: copy the shared launcher to the cluster:
#   azcluster scp <cluster> examples/megatron-pretrain.py :/shared/home/clusteradmin/megatron-pretrain.py
#   azcluster scp <cluster> examples/slurm/training-megatron.sbatch :/shared/home/clusteradmin/
#   azcluster exec <cluster> --user clusteradmin -- "sbatch training-megatron.sbatch"
#
# Matched payload: nvcr.io/nvidia/nemo:26.04.00, 2 nodes × 8 GPUs, torchrun,
# TP=1 PP=1 CP=2 GBS=256 MBS=1 TRAIN_ITERS=50. Watch for MODEL_TFLOP/s/GPU.
#SBATCH --job-name=megatron-train
#SBATCH --output=megatron-train-%j.out
#SBATCH --partition=gpu
#SBATCH --nodes=2
#SBATCH --ntasks-per-node=1
#SBATCH --gpus-per-node=8
#SBATCH --exclusive
#SBATCH --time=01:00:00

set -euo pipefail
date -u

IMAGE=${IMAGE:-docker://nvcr.io/nvidia/nemo:26.04.00}
SCRIPT=${SCRIPT:-/shared/home/${USER}/megatron-pretrain.py}
[ -f "${SCRIPT}" ] || { echo "missing ${SCRIPT}; scp examples/megatron-pretrain.py there first" >&2; exit 1; }

MASTER_HOST=$(scontrol show hostnames "${SLURM_JOB_NODELIST}" | head -1)
MASTER_ADDR=$(getent hosts "${MASTER_HOST}" | awk '{print $1; exit}')
[ -n "${MASTER_ADDR}" ] || { echo "could not resolve ${MASTER_HOST}" >&2; exit 1; }

export MASTER_ADDR MASTER_PORT=${MASTER_PORT:-29500}
export TRAIN_ITERS=${TRAIN_ITERS:-50}
export TP=${TP:-1}
export PP=${PP:-1}
export CP=${CP:-2}
export GBS=${GBS:-256}
export MBS=${MBS:-1}
export NCCL_DEBUG=${NCCL_DEBUG:-WARN}
export NCCL_SOCKET_IFNAME=${NCCL_SOCKET_IFNAME:-eth0}
export NCCL_IB_PCI_RELAXED_ORDERING=1
export NCCL_MIN_NCHANNELS=32
export NCCL_P2P_NET_CHUNKSIZE=524288
export NCCL_COLLNET_ENABLE=1

SUMMARY_LOG="${SLURM_SUBMIT_DIR:-.}/megatron-train-${SLURM_JOB_ID}-summary.log"
# Megatron-Bridge does not exit cleanly after the final iteration (its rank
# processes are SIGTERM'd on teardown), so srun returns non-zero even on a
# successful run. Tee a copy of the log, then treat "reached the final
# iteration" as success rather than trusting the teardown exit code.
set +e
srun --mpi=pmix \
  --container-image="${IMAGE}" \
  --container-mounts=/shared:/shared,/mnt/nvme:/mnt/nvme \
  --no-container-mount-home \
  --export=ALL,MASTER_ADDR,MASTER_PORT,TRAIN_ITERS,TP,PP,CP,GBS,MBS,NCCL_DEBUG,NCCL_SOCKET_IFNAME,NCCL_IB_PCI_RELAXED_ORDERING,NCCL_MIN_NCHANNELS,NCCL_P2P_NET_CHUNKSIZE,NCCL_COLLNET_ENABLE \
  bash -lc 'set -euo pipefail; torchrun --nnodes="${SLURM_NNODES}" --node_rank="${SLURM_NODEID}" --nproc_per_node=8 --master_addr="${MASTER_ADDR}" --master_port="${MASTER_PORT}" "'"${SCRIPT}"'"' 2>&1 | tee "${SUMMARY_LOG}"
rc=${PIPESTATUS[0]}
set -e

echo "=== steady-state MODEL_TFLOP/s/GPU (last 10 iterations) ==="
grep -oE 'GPU utilization: [0-9.]+MODEL_TFLOP/s/GPU' "${SUMMARY_LOG}" | tail -10 || true
if grep -qE "iteration +${TRAIN_ITERS}/" "${SUMMARY_LOG}"; then
  echo "TRAIN_OK (reached iteration ${TRAIN_ITERS}; ignored non-clean teardown rc=${rc})"
  rm -f "${SUMMARY_LOG}"
  date -u
  exit 0
fi
echo "TRAIN_FAILED: did not reach iteration ${TRAIN_ITERS} (rc=${rc})" >&2
rm -f "${SUMMARY_LOG}"
exit "${rc}"
