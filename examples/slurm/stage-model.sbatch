#!/bin/bash -l
#
# Stage a HuggingFace model to the per-cluster Blob container.
# Slurm mirror of examples/aks/stage-model.yaml.
#
# Run examples:
#   sbatch stage-model.sbatch neuralmagic/Meta-Llama-3.1-8B-Instruct-FP8 llama-3.1-8b-fp8
#   sbatch stage-model.sbatch deepseek-ai/DeepSeek-R1-0528 dsr1-fp8
#
# Uses cluster-provided env: AZCLUSTER_USER_BLOB_URL, AZCLUSTER_USER_NVME,
# AZURE_CLIENT_ID. Optional gated models: put token in ~/.hf-token (not in git).
#SBATCH --job-name=stage-model
#SBATCH --output=stage-model-%j.out
#SBATCH --partition=gpu
#SBATCH --nodes=1
#SBATCH --ntasks-per-node=1
#SBATCH --gpus-per-node=1
#SBATCH --time=04:00:00

set -euo pipefail
date -u

MODEL_REPO=${1:-neuralmagic/Meta-Llama-3.1-8B-Instruct-FP8}
DEST_PREFIX=${2:-llama-3.1-8b-fp8}
SCRATCH=${AZCLUSTER_USER_NVME:-/mnt/nvme/users/${USER}}/models

mkdir -p "${SCRATCH}"
if [ ! -d "${AZCLUSTER_USER_NVME:-/mnt/nvme/users/${USER}}/hfvenv" ]; then
  python3 -m venv "${AZCLUSTER_USER_NVME:-/mnt/nvme/users/${USER}}/hfvenv"
fi
source "${AZCLUSTER_USER_NVME:-/mnt/nvme/users/${USER}}/hfvenv/bin/activate"
pip install -q -U huggingface_hub hf_transfer

[ -f "${HOME}/.hf-token" ] && export HF_TOKEN="$(tr -d '[:space:]' < "${HOME}/.hf-token")"

echo "[stage] hf download ${MODEL_REPO} -> ${SCRATCH}/${DEST_PREFIX}"
time hf download "${MODEL_REPO}" --local-dir "${SCRATCH}/${DEST_PREFIX}"
du -sh "${SCRATCH}/${DEST_PREFIX}"

echo "[stage] azcp copy -> ${AZCLUSTER_USER_BLOB_URL}/models/${DEST_PREFIX}/"
time azcp copy "${SCRATCH}/${DEST_PREFIX}/" "${AZCLUSTER_USER_BLOB_URL}/models/${DEST_PREFIX}/" \
  --recursive --workers 4 --concurrency 32 --block-size 16777216

echo STAGE_OK
date -u
