#!/bin/bash -l
#
# Single-node Llama-3.1-8B-FP8 vLLM benchmark.
# Slurm/Pyxis mirror of examples/aks/inference-vllm.yaml.
#
# Prereq: stage/distribute the model first:
#   sbatch stage-model.sbatch neuralmagic/Meta-Llama-3.1-8B-Instruct-FP8 llama-3.1-8b-fp8
#   sbatch distribute-azcp-cluster.sbatch llama-3.1-8b-fp8
#
# Run:
#   azcluster scp <cluster> examples/slurm/inference-vllm.sbatch :/shared/home/clusteradmin/
#   azcluster exec <cluster> --user clusteradmin -- "sbatch inference-vllm.sbatch"
#
# Matched payload: vllm/vllm-openai:latest, model prefix llama-3.1-8b-fp8,
# `vllm bench serve` random ISL/OSL 1000, num-prompts 1280, concurrency 128.
#SBATCH --job-name=vllm-llama
#SBATCH --output=vllm-llama-%j.out
#SBATCH --partition=gpu
#SBATCH --nodes=1
#SBATCH --ntasks-per-node=1
#SBATCH --gpus-per-node=1
#SBATCH --exclusive
#SBATCH --time=01:00:00

set -euo pipefail
date -u

IMAGE=${IMAGE:-docker://vllm/vllm-openai:latest}
export MODEL_PREFIX=${MODEL_PREFIX:-llama-3.1-8b-fp8}
HOST_MODEL=${AZCLUSTER_USER_NVME:-/mnt/nvme/users/${USER}}/models/${MODEL_PREFIX}
[ -d "${HOST_MODEL}" ] || { echo "missing ${HOST_MODEL}; run stage-model + distribute first" >&2; exit 1; }

srun --mpi=pmix \
  --container-image="${IMAGE}" \
  --container-mounts="${HOST_MODEL}:/models/${MODEL_PREFIX}" \
  --no-container-mount-home \
  --no-container-entrypoint \
  --export=ALL,MODEL_PREFIX \
  bash -lc '
    set -euo pipefail
    MODEL_DIR="/models/${MODEL_PREFIX}"
    export HF_HUB_OFFLINE=1
    echo "[vllm] serving ${MODEL_DIR}"
    vllm serve "${MODEL_DIR}" --served-model-name llama --port 8888 \
      --tensor-parallel-size 1 --max-model-len 8192 --gpu-memory-utilization 0.9 &
    pid=$!
    for i in $(seq 1 120); do
      python3 -c "import urllib.request; urllib.request.urlopen('"'"'http://127.0.0.1:8888/health'"'"')" 2>/dev/null && break
      sleep 5
    done
    echo "[vllm] server ready; benchmark conc=128 ISL/OSL=1000"
    vllm bench serve --backend vllm --model llama --base-url http://127.0.0.1:8888 \
      --tokenizer "${MODEL_DIR}" \
      --dataset-name random --random-input-len 1000 --random-output-len 1000 \
      --random-range-ratio 0.2 --num-prompts 1280 --max-concurrency 128
    kill "${pid}" 2>/dev/null || true
  '

date -u
