# AKS examples

These are **examples you run with `kubectl`** against an AKS cluster deployed by
`azcluster deploy --target aks`. They are intentionally **not** compiled into the
`azcluster` binary — `azcluster` provisions the infrastructure (AKS, NVIDIA
operators, **Azure Container Storage / local-csi**, the per-cluster Blob account)
and you drive the data plane with these manifests.

The benchmark examples are matched to `../slurm/`: same container images, models,
node/GPU counts, and command-line parameters so Slurm-vs-AKS results are a
controlled comparison.

The storage pipeline mirrors the Slurm side, AKS-native:

| Phase | Tool | What |
|---|---|---|
| stage (once per model) | `azcp` | upload weights/data to the per-cluster Blob container |
| consume (per job) | **blobcache** | FUSE-mount the container read-only; hydrate pulls chunks onto **ACStor local NVMe**; peer misses are served **node-to-node over InfiniBand (RDMA/UCX)**, Blob only as last resort |

No `hostPath`: the cache lives on ACStor (`storageClassName: local-csi`, an
auto-RAID-0 of the node's local NVMe). blobcache pods are privileged, so they
reach `/dev/infiniband` directly and do **not** consume the exclusive `rdma/ib`
device resource — leaving all 8 HCAs free for a co-scheduled NCCL/training job.

## Prerequisites

```bash
azcluster deploy --target aks --name <cluster> \
  --location <region> \
  --pool name=gpu,sku=Standard_ND96isr_H200_v5,count=2,default
azcluster kubeconfig <cluster>
export KUBECONFIG=~/.azcluster/kube/<cluster>.config
```

Get the values the manifests need from one command:

```bash
azcluster status <cluster>
#   storage account:   stazc........
#   data container:    https://stazc........blob.core.windows.net/data
#   kubelet client id: ........-....-....-....-............
```

`envsubst` (from `gettext`) substitutes the `${...}` placeholders. Restrict it to
the named vars so the shell `${...}` inside pod scripts is left intact.

## NCCL all-reduce — [`nccl-allreduce-mpijob.yaml`](nccl-allreduce-mpijob.yaml)

Kubernetes MPIJob equivalent of `../slurm/nccl-allreduce.sbatch`:
`ghcr.io/azure/ai-infrastructure-on-azure/nccl-test:latest`, 2 nodes × 8 GPUs,
`all_reduce_perf_mpi -b 16G -e 16G -f 2 -g 1 -N 10`.

```bash
export NODES=2
envsubst '${NODES}' < nccl-allreduce-mpijob.yaml | kubectl apply -f -
kubectl wait --for=condition=complete job/azcluster-nccl-validate-launcher --timeout=1800s
kubectl logs job/azcluster-nccl-validate-launcher
```

Pass criteria: average bus bandwidth ≥ 400 GB/s, at least eight `mlx5_*`
IB/SHARP devices per node in NCCL logs, and no `NET/Socket` / `NET/IB : No device
found` fallback.

## Megatron training — operator + shared script + PyTorchJob

Install the trimmed PyTorch-only training operator, create a ConfigMap from the
shared launcher, then submit `training-megatron-pytorchjob.yaml`. This replaces
the former embedded training path with visible files.

```bash
kubectl apply -f training-operator.yaml
kubectl wait -n kubeflow --for=condition=available deploy/training-operator --timeout=300s

kubectl create configmap azcluster-llama-pretrain \
  --from-file=pretrain.py=../megatron-pretrain.py \
  --dry-run=client -o yaml | kubectl apply -f -

export WORKER_REPLICAS=1 TRAIN_ITERS=50 GBS=256 CP=2
envsubst '${WORKER_REPLICAS} ${TRAIN_ITERS} ${GBS} ${CP}' \
  < training-megatron-pytorchjob.yaml | kubectl apply -f -
kubectl logs -f -l training.kubeflow.org/job-name=azcluster-llama-train,training.kubeflow.org/replica-type=master
```

Matched payload: `nvcr.io/nvidia/nemo:26.04.00`, `../megatron-pretrain.py`,
TP=1 PP=1 CP=2 GBS=256 MBS=1 TRAIN_ITERS=50. Watch the master logs for
`MODEL_TFLOP/s/GPU`.

## 1. Stage data to Blob — [`azcp-upload.yaml`](azcp-upload.yaml)

```bash
export MI_CLIENT_ID=$(azcluster status <cluster> | awk '/kubelet client id:/{print $4}')
export DEST_URL=$(azcluster status <cluster> | awk '/data container:/{print $3}')/models/llama/
envsubst '${MI_CLIENT_ID} ${DEST_URL}' < azcp-upload.yaml | kubectl apply -f -
kubectl wait --for=condition=complete job/azcp-upload --timeout=1800s
kubectl logs job/azcp-upload     # ends with AZCP_UPLOAD_OK
```

## 2. Consume over InfiniBand — [`blobcache-rdma.yaml`](blobcache-rdma.yaml)

Two `blobcached` pods (one per GPU node) form a gossip cluster with the **RDMA
(UCX-over-InfiniBand)** peer transport, caching onto ACStor NVMe.

```bash
export STORAGE_ACCOUNT=$(azcluster status <cluster> | awk '/storage account:/{print $3}')
envsubst '${STORAGE_ACCOUNT} ${MI_CLIENT_ID}' < blobcache-rdma.yaml | kubectl apply -f -
kubectl rollout status statefulset/blobcache-rdma --timeout=300s

# confirm a 2-node cluster + RDMA transport
kubectl exec blobcache-rdma-0 -- curl -s 127.0.0.1:7773/peers
kubectl logs blobcache-rdma-0 | grep 'peer transport client initialised'   # kind=rdma

# hydrate (sharded origin fetch from Blob across both nodes)
kubectl exec blobcache-rdma-0 -- curl -s -XPOST 127.0.0.1:7773/hydrate \
  -H content-type:application/json -d '{"mount":"data","path":"","recursive":true}'

# read the data on node 1 — chunks it lacks are pulled FROM node 0 over IB
kubectl exec blobcache-rdma-1 -- cat /mnt/blobcache/data/<your/path> | wc -c

# verify the bytes came over the RDMA peer transport (not Blob origin):
kubectl exec blobcache-rdma-1 -- curl -s 127.0.0.1:7773/metrics \
  | grep blobcache_chunk_peer_fetch_seconds_count        # > 0
kubectl exec blobcache-rdma-0 -- curl -s 127.0.0.1:7773/metrics \
  | grep blobcache_peer_chunk_bytes_served_total         # > 0
```

Teardown the example (leaves the cluster up):

```bash
kubectl delete -f blobcache-rdma.yaml
kubectl delete job/azcp-upload
```

## Inference benchmark — [`inference-vllm.yaml`](inference-vllm.yaml)

Single-node vLLM serving Llama-3.1-8B-FP8 from a blobcache sidecar (model staged
to Blob, hydrated onto ACStor NVMe), then `vllm bench serve` at concurrency 128.
Live result on 1× ND H200: **9,840 tok/s output, 12.66 ms median TPOT, 67.97 ms
median TTFT** — matching the Slurm walkthrough baseline (~9,863 tok/s @ 12.4 ms).
Stage the model once (public, no HF token):

```bash
# stage the model to Blob first (ACStor scratch -> hf download -> azcp copy):
#   see stage-model.yaml — MODEL_REPO=neuralmagic/Meta-Llama-3.1-8B-Instruct-FP8 DEST_PREFIX=llama-3.1-8b-fp8
envsubst '${STORAGE_ACCOUNT} ${MI_CLIENT_ID}' < inference-vllm.yaml | kubectl apply -f -
kubectl logs -f job/inference-vllm -c vllm
```

## Multi-node inference (SGLang TP=16) — [`inference-sglang-multinode.yaml`](inference-sglang-multinode.yaml)

Two-node SGLang tensor-parallel (TP=16) serving across both GPU nodes, model
distributed by a blobcache RDMA sidecar (peer chunks over InfiniBand). Mirrors
run 6 of the Slurm walkthrough (DeepSeek-R1-0528 FP8 SGLang TP=16). `sglang-0` is
the TP head (`--dist-init-addr sglang-0.sglang:5000`); each pod's privileged
blobcache sidecar feeds the model in over IB while the sglang container holds all
8 GPUs + 8 `rdma/ib` for the cross-node TP NCCL. Set `MODEL_PREFIX` to the staged
model (e.g. `dsr1-fp8` for DeepSeek-R1, or `llama-3.1-8b-fp8` to smoke-test the
multi-node path). `sglang-0` runs `sglang.bench_serving` and prints tok/s + TPOT.

The cross-node TP=16 serving mechanism is live-validated on 2× ND H200 (server
forms across both nodes over IB and serves); the headline DeepSeek-R1 number
needs the ~640 GB model staged to Blob first.

## Using blobcache from a training job — [`training-blobcache.yaml`](training-blobcache.yaml)

For a benchmark, run blobcache as a **per-job sidecar** in the training pod: a
`PyTorchJob` whose pods each carry the blobcached RDMA sidecar (cache on an
ACStor ephemeral volume, FUSE shared with the trainer via `mountPropagation`)
and an azcp checkpoint-save sidecar. The trainer reads its model/dataset from
`/mnt/blobcache/...` and writes checkpoints to a shared ACStor `/scratch`, which
the azcp sidecar uploads to Blob on a `.DONE`/`.UPLOADED` handshake.

The trainer container in the manifest is a placeholder that proves the data path
(reads the staged bytes from blobcache, writes a checkpoint, runs the handshake);
replace it with your real launcher (e.g. the Megatron-Bridge pretrain used by
the Megatron training examples). The constituent mechanisms are each live-validated by the two
examples above and the checkpoint handshake; wire in your model + GPU launcher to
run the full benchmark.
