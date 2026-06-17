# AKS storage examples

These are **examples you run with `kubectl`** against an AKS cluster deployed by
`azcluster deploy --target aks`. They are intentionally **not** compiled into the
`azcluster` binary — `azcluster` provisions the infrastructure (AKS, NVIDIA
operators, **Azure Container Storage / local-csi**, the per-cluster Blob account)
and you drive the data plane with these manifests.

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

## Using blobcache from a training job

For a benchmark, run blobcache as a **per-job sidecar** in the training pod
(same privileged + IPC_LOCK + RDMA config, cache on an ACStor ephemeral volume,
FUSE shared with the trainer via `mountPropagation`). The training container
reads its model/dataset from the shared `/mnt/blobcache/...` path. See the
end-to-end walkthrough in [`../../doc/`](../../doc/) for the full benchmark
recipe and how checkpoints are written back to Blob with `azcp`.
