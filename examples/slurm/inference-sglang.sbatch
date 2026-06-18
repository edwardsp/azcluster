#!/bin/bash -l
#
# Two-node DeepSeek-R1 SGLang TP=16 inference benchmark.
# Slurm/Pyxis mirror of examples/aks/inference-sglang-multinode.yaml.
#
# Prereq: stage/distribute the model first:
#   sbatch stage-model.sbatch deepseek-ai/DeepSeek-R1-0528 dsr1-fp8
#   sbatch distribute-azcp-cluster.sbatch dsr1-fp8
#
# Run:
#   azcluster scp <cluster> examples/slurm/inference-sglang.sbatch :/shared/home/clusteradmin/
#   azcluster exec <cluster> --user clusteradmin -- "sbatch inference-sglang.sbatch"
#
# Matched payload: lmsysorg/sglang:v0.5.8-cu130, DeepSeek-R1-0528 at TP=16
# across 2 nodes, random 1024/1024 prompts, range-ratio 0.2, 640 prompts,
# max concurrency 64. Do NOT set HF_HUB_OFFLINE for the benchmark; strip
# tokenizer_config.auto_map into /tmp/tok before running sglang.bench_serving.
#SBATCH --job-name=sglang-dsr1
#SBATCH --output=sglang-dsr1-%j.out
#SBATCH --partition=gpu
#SBATCH --nodes=2
#SBATCH --ntasks-per-node=1
#SBATCH --gpus-per-node=8
#SBATCH --exclusive
#SBATCH --time=04:00:00

set -euo pipefail
date -u

IMAGE=${IMAGE:-docker://lmsysorg/sglang:v0.5.8-cu130}
MODEL_PREFIX=${MODEL_PREFIX:-dsr1-fp8}
HOST_MODEL=${AZCLUSTER_USER_NVME:-/mnt/nvme/users/${USER}}/models/${MODEL_PREFIX}
[ -d "${HOST_MODEL}" ] || { echo "missing ${HOST_MODEL}; run stage-model + distribute first" >&2; exit 1; }

MASTER_HOST=$(scontrol show hostnames "${SLURM_JOB_NODELIST}" | head -1)
HEAD_IP=$(getent hosts "${MASTER_HOST}" | awk '{print $1; exit}')
[ -n "${HEAD_IP}" ] || { echo "could not resolve ${MASTER_HOST}" >&2; exit 1; }

export HEAD_IP MODEL_PREFIX

srun --mpi=pmix \
  --container-image="${IMAGE}" \
  --container-mounts="${HOST_MODEL}:/models/${MODEL_PREFIX}" \
  --no-container-mount-home \
  --no-container-entrypoint \
  --export=ALL,HEAD_IP,MODEL_PREFIX \
  bash -lc '
    set -euo pipefail
    MODEL_DIR="/models/${MODEL_PREFIX}"
    NODE_RANK=${SLURM_NODEID:-${SLURM_PROCID:-0}}
    PORT=8888
    echo "[sglang] rank=${NODE_RANK} host=$(hostname) head=${HEAD_IP}:5000"
    python3 -m sglang.launch_server \
      --model-path "${MODEL_DIR}" --served-model-name model \
      --host 0.0.0.0 --port "${PORT}" \
      --tp 16 --nnodes 2 --node-rank "${NODE_RANK}" \
      --dist-init-addr "${HEAD_IP}:5000" \
      --mem-fraction-static 0.85 --chunked-prefill-size 8192 \
      --max-running-requests 64 --trust-remote-code \
      --watchdog-timeout 1200 \
      --attention-backend flashinfer \
      > "server-rank${NODE_RANK}.log" 2>&1 &
    server_pid=$!

    if [ "${NODE_RANK}" = "0" ]; then
      for i in $(seq 1 360); do
        python3 -c "import urllib.request; urllib.request.urlopen('"'"'http://127.0.0.1:8888/health'"'"')" 2>/dev/null && break
        if ! kill -0 "${server_pid}" 2>/dev/null; then cat server-rank0.log; exit 1; fi
        sleep 10
      done
      mkdir -p /tmp/tok
      cp "${MODEL_DIR}/tokenizer.json" "${MODEL_DIR}/tokenizer_config.json" /tmp/tok/
      python3 - <<PY
import json
p = "/tmp/tok/tokenizer_config.json"
d = json.load(open(p))
d.pop("auto_map", None)
json.dump(d, open(p, "w"))
PY
      python3 -m sglang.bench_serving --backend sglang --host 127.0.0.1 --port "${PORT}" \
        --model model --tokenizer /tmp/tok \
        --dataset-name random --random-input-len 1024 --random-output-len 1024 \
        --random-range-ratio 0.2 --num-prompts 640 --max-concurrency 64
      kill "${server_pid}" 2>/dev/null || true
      wait "${server_pid}" 2>/dev/null || true
    else
      wait "${server_pid}"
    fi
  '

date -u
