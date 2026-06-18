# Slurm examples

These `sbatch` files run on an azcluster Slurm deployment and are matched to the
AKS manifests in `../aks/` wherever possible.

## Run pattern

```bash
azcluster scp <cluster> examples/slurm/<file>.sbatch :/shared/home/clusteradmin/
azcluster exec <cluster> --user clusteradmin -- "sbatch <file>.sbatch"
```

For training, copy the shared launcher too:

```bash
azcluster scp <cluster> examples/megatron-pretrain.py :/shared/home/clusteradmin/megatron-pretrain.py
```

## Matched examples

| Slurm file | AKS equivalent | Notes |
|---|---|---|
| `nccl-allreduce-vm.sbatch` | — | Slurm-only plain-VM HPC-X baseline. |
| `nccl-allreduce.sbatch` | `../aks/nccl-allreduce-mpijob.yaml` | Same `nccl-test:latest` image and `all_reduce_perf_mpi -b 16G -e 16G -f 2 -g 1 -N 10`. |
| `training-megatron.sbatch` | `../aks/training-megatron-pytorchjob.yaml` | Same NeMo 26.04 container, shared `../megatron-pretrain.py`, TP=1 PP=1 CP=2 GBS=256 MBS=1 TRAIN_ITERS=50. |
| `stage-model.sbatch` | `../aks/stage-model.yaml` | Download HuggingFace model to NVMe, upload to Blob with `azcp`. |
| `distribute-azcp-cluster.sbatch` | `../aks/blobcache-rdma.yaml` and workload sidecars | Slurm copies Blob → per-node NVMe via `azcp-cluster`; AKS consumes via blobcache RDMA sidecars. |
| `inference-vllm.sbatch` | `../aks/inference-vllm.yaml` | Same vLLM image, Llama-3.1-8B-FP8 model, random benchmark, concurrency 128. |
| `inference-sglang.sbatch` | `../aks/inference-sglang-multinode.yaml` | Same SGLang image, DeepSeek-R1-0528, TP=16, 640 prompts, concurrency 64. |

## Typical flow

```bash
# NCCL container comparison
sbatch nccl-allreduce.sbatch

# Training comparison
sbatch training-megatron.sbatch

# Small inference model: stage once, distribute each run, serve/bench
sbatch stage-model.sbatch neuralmagic/Meta-Llama-3.1-8B-Instruct-FP8 llama-3.1-8b-fp8
sbatch distribute-azcp-cluster.sbatch llama-3.1-8b-fp8
sbatch inference-vllm.sbatch

# Large inference model
sbatch stage-model.sbatch deepseek-ai/DeepSeek-R1-0528 dsr1-fp8
sbatch distribute-azcp-cluster.sbatch dsr1-fp8
sbatch inference-sglang.sbatch
```

The storage scripts rely on cluster-provided environment from
`/etc/profile.d/azcluster-storage.sh`: `AZCLUSTER_USER_BLOB_URL`,
`AZCLUSTER_USER_NVME`, and `AZURE_CLIENT_ID`.
