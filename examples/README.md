# azcluster workload examples

`azcluster` provisions the infrastructure. These files are the visible,
runnable workloads: copy/apply them yourself, edit parameters explicitly, and use
the same workloads on Slurm and AKS for controlled comparisons.

The Slurm and AKS examples intentionally use the same containers, models, GPU
counts, and benchmark parameters wherever both targets support the workload. The
goal is to make Slurm-vs-AKS numbers differ only by orchestration/runtime, not by
payload.

| Workload | Slurm | AKS | Matched payload |
|---|---|---|---|
| NCCL plain VM | `slurm/nccl-allreduce-vm.sbatch` | — | Slurm-only non-containerized HPC-X baseline, 2 nodes × 8 GPUs |
| NCCL container | `slurm/nccl-allreduce.sbatch` | `aks/nccl-allreduce-mpijob.yaml` | `ghcr.io/azure/ai-infrastructure-on-azure/nccl-test:latest`, `all_reduce_perf_mpi -b 16G -e 16G -f 2 -g 1 -N 10`, 16 ranks |
| Megatron training | `slurm/training-megatron.sbatch` | `aks/training-megatron-pytorchjob.yaml` | `nvcr.io/nvidia/nemo:26.04.00`, shared `megatron-pretrain.py`, TP=1 PP=1 CP=2 GBS=256 MBS=1 TRAIN_ITERS=50 |
| vLLM inference | `slurm/inference-vllm.sbatch` | `aks/inference-vllm.yaml` | `vllm/vllm-openai:latest`, `neuralmagic/Meta-Llama-3.1-8B-Instruct-FP8`, concurrency 128 |
| DeepSeek SGLang | `slurm/inference-sglang.sbatch` | `aks/inference-sglang-multinode.yaml` | `lmsysorg/sglang:v0.5.8-cu130`, `deepseek-ai/DeepSeek-R1-0528`, TP=16, 640 prompts, concurrency 64 |
| Storage stage | `slurm/stage-model.sbatch` | `aks/stage-model.yaml` | HuggingFace download to local NVMe, then `azcp copy` to the per-cluster Blob container |
| Storage distribute | `slurm/distribute-azcp-cluster.sbatch` | `aks/blobcache-rdma.yaml` / workload sidecars | Blob-backed model distribution to per-node NVMe/cache over InfiniBand |

## Layout

- `megatron-pretrain.py` — the single shared Megatron-Bridge launcher used by
  both Slurm and AKS training examples.
- `slurm/` — `sbatch` files for clusters deployed with `azcluster deploy`.
- `aks/` — Kubernetes manifests for clusters deployed with
  `azcluster deploy --target aks`.

Start with the README in the target-specific subdirectory, then run the matching
pair you care about.
