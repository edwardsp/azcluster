#!/bin/bash -l
#
# Distribute a staged model directory from Blob to each node's local NVMe over IB.
# Slurm analog of AKS blobcache RDMA distribution.
#
# Run:
#   sbatch distribute-azcp-cluster.sbatch llama-3.1-8b-fp8
#   sbatch distribute-azcp-cluster.sbatch dsr1-fp8
#
# Reads from ${AZCLUSTER_USER_BLOB_URL}/models/<prefix>/ and writes to
# ${AZCLUSTER_USER_NVME}/models/<prefix>/ on every allocated node.
#SBATCH --job-name=azcp-dist-model
#SBATCH --output=azcp-dist-model-%j.out
#SBATCH --partition=gpu
#SBATCH --nodes=2
#SBATCH --ntasks-per-node=1
#SBATCH --gpus-per-node=8
#SBATCH --exclusive
#SBATCH --time=01:00:00

set -euo pipefail
date -u

MODEL_PREFIX=${1:-llama-3.1-8b-fp8}
SRC="${AZCLUSTER_USER_BLOB_URL}/models/${MODEL_PREFIX}/"
DST="${AZCLUSTER_USER_NVME}/models/${MODEL_PREFIX}/"
mkdir -p "${DST}"

export UCX_TLS=rc,sm,self
export UCX_NET_DEVICES=mlx5_ib0:1
export OMPI_MCA_pml=ucx
export OMPI_MCA_osc=ucx
EXP='ALL,AZURE_CLIENT_ID,AZCLUSTER_USER_BLOB_URL,AZCLUSTER_USER_NVME,UCX_TLS,UCX_NET_DEVICES,OMPI_MCA_pml,OMPI_MCA_osc'

srun --mpi=pmix --export="${EXP}" \
  --container-image=docker://ghcr.io/edwardsp/azcp/azcp-cluster:v0.4.3 \
  --container-mounts=/mnt/nvme:/mnt/nvme \
  taskset -c 0-47 \
  azcp-cluster "${SRC}" "${DST}" \
    --concurrency 32 --block-size 16777216 \
    --bcast-chunk 67108864 --bcast-pipeline 128 --bcast-writers 8 \
    --compare size --no-progress

echo DISTRIBUTE_OK
date -u
