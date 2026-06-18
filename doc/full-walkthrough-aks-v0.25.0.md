# AKS target — full walkthrough (v0.25.0)

First clean end-to-end `azcluster deploy --target aks` walkthrough on the committed
v0.25.0 build, mirroring the Slurm walkthrough AKS-native. Captured on cluster
`aksm5`, **2× Standard_ND96isr_H200_v5 / mexicocentral** (Grafana in `eastus2`).

The AKS target is pod-native: there is no bare-metal job path, so each Slurm
walkthrough step maps to an `examples/aks/` manifest run with `kubectl`/`helm`.
The AKS analog of the Slurm walkthrough's two NCCL runs (plain-VM + container) is
the single containerised `examples/aks/nccl-allreduce-mpijob.yaml` MPIJob — there
is no plain-VM variant on AKS.

## 0. Deploy + monitoring

```bash
azcluster deploy --target aks --name aksm5 \
  --location mexicocentral --grafana-location eastus2 \
  --pool name=gpu,sku=Standard_ND96isr_H200_v5,count=2,default
```

- ARM: 30 resources / **959 s**, including the M5 managed-Prometheus chain
  (`dataCollectionEndpoints`/`dataCollectionRules`/`dataCollectionRuleAssociations
  ContainerInsightsMetricsExtension` via `bicep/modules/aks-prometheus.bicep`) +
  AMW (`amw-aksm5`, mexicocentral) + AMG (`amg-aksm5`, eastus2) + per-cluster Blob.
- Operator stages (AKS `runCommand`): cert-manager → NVIDIA Network Operator →
  NVIDIA GPU Operator → **Managed Prometheus DCGM ServiceMonitor (M5)** → Kueue →
  MPI Operator → Azure Container Storage (`local-csi`).
- `azcluster list` shows `TARGET = aks`.

## 1. Native operate client (no laptop `kubectl`)

`exec`/`logs`/`ssh` talk to the API server directly over client-cert TLS + WebSocket
(no `kube-rs`, MSRV 1.78):

```bash
azcluster exec aksm5 --host gpu-operator/<dcgm-pod> -- nvidia-smi -L   # lists 8× H200
azcluster logs aksm5 --component gpu-operator/<dcgm-pod> --tail 8       # streams pod logs
azcluster ssh  aksm5 --host aks-gpu-...-vmss000000                      # host-root chroot shell
```

- `exec` ran `nvidia-smi -L` in a pod → all **8 H200** GPUs; the exec error channel
  correctly surfaced exit codes (an OCI "executable not found" for a missing binary).
- `ssh --host <node>` creates a privileged hostPath-`/`-+-`chroot` debug pod
  (`uname -r` = `5.15.0-1111-azure`), exits cleanly, and auto-deletes the pod.
- `azcluster tunnel <aks>` is deferred (Kubernetes WS port-forward uses the
  `SPDY/3.1+portforward.k8s.io` tunneling subprotocol — a follow-up); it bails with
  the interim `kubectl port-forward` command.

## 2. NCCL validation (2-node, container)

```bash
export NODES=2
envsubst '${NODES}' < examples/aks/nccl-allreduce-mpijob.yaml | kubectl apply -f -
kubectl wait --for=condition=complete job/azcluster-nccl-validate-launcher --timeout=1800s
kubectl logs job/azcluster-nccl-validate-launcher
```

- 2-node MPIJob through Kueue, `all_reduce_perf_mpi -b 16G -e 16G -f 2 -g 1 -N 10`,
  16 ranks.
- **avg busbw 483.36 GB/s**, **8 IB/SHARP devices per node** (16 NICs across the job),
  no TCP fallback. Gate (≥400 GB/s + ≥8 IB/SHARP + no TCP) passed.

## 3. Training (DGXC Megatron-Bridge, Llama-3.1-8B)

```bash
kubectl apply -f examples/aks/training-operator.yaml
kubectl wait -n kubeflow --for=condition=available deploy/training-operator --timeout=300s
kubectl create configmap azcluster-llama-pretrain \
  --from-file=pretrain.py=examples/megatron-pretrain.py \
  --dry-run=client -o yaml | kubectl apply -f -
export WORKER_REPLICAS=1 TRAIN_ITERS=50 GBS=256 CP=2
envsubst '${WORKER_REPLICAS} ${TRAIN_ITERS} ${GBS} ${CP}' \
  < examples/aks/training-megatron-pytorchjob.yaml | kubectl apply -f -
kubectl logs -f -l training.kubeflow.org/job-name=azcluster-llama-train,training.kubeflow.org/replica-type=master
```

- PyTorchJob, 16 GPUs / 2 nodes, gbs=256, 50 iters.
- steady-state **506.4 MODEL_TFLOP/s/GPU**.

## 4. Storage — azcp stage + blobcache over InfiniBand

Stage a model to Blob (ACStor scratch → `hf download` → `azcp copy`), then the
2-node `blobcache-rdma` StatefulSet hydrates + serves peer chunks over IB:

```bash
# stage (Job): hf download neuralmagic/Meta-Llama-3.1-8B-Instruct-FP8 → azcp copy → Blob
envsubst '${STORAGE_ACCOUNT} ${MI_CLIENT_ID}' < examples/aks/blobcache-rdma.yaml | kubectl apply -f -
kubectl exec blobcache-rdma-0 -- curl -s -XPOST 127.0.0.1:7773/hydrate \
  -H content-type:application/json -d '{"mount":"data","path":"models/llama-3.1-8b-fp8","recursive":true}'
```

- azcp upload: 8.47 GiB in 4.6 s = **15.91 Gbps**.
- blobcache hydrate sharded 9.09 GB across both nodes; reading the model on node-1
  **peer-fetched 1083 chunks (4.54 GB) over RDMA** from node-0
  (`blobcache_chunk_peer_fetch_seconds_count`, `blobcache_peer_chunk_bytes_served_total`),
  `peer transport client initialised kind=rdma`. blobcache pods are privileged so they
  reach `/dev/infiniband` without taking the exclusive `rdma/ib` resource.

## 5. Inference — vLLM (Llama-3.1-8B-FP8)

```bash
envsubst '${STORAGE_ACCOUNT} ${MI_CLIENT_ID}' < examples/aks/inference-vllm.yaml | kubectl apply -f -
```

- 1280 requests, 0 failed. **output 9,912 tok/s, median TPOT 12.55 ms, median TTFT 64 ms**
  (matches the Slurm ~9,863 baseline).

## 6. Inference — DeepSeek-R1-0528 SGLang TP=16 (multi-node)

DeepSeek-R1-0528 (642 GB) staged to Blob, served TP=16 across both nodes; blobcache
distributes the weights over IB. See `examples/aks/inference-sglang-multinode.yaml`
for the exact bench prep (don't set `HF_HUB_OFFLINE=1` — the random sampler pulls
ShareGPT; strip the DeepSeek tokenizer `auto_map`).

- Hydrate: **688 GB over IB** (`peer transport`, 2296 MiB/s); all 163 safetensors
  shards loaded across the 16 GPUs; DeepGEMM first-run JIT warmup ~10-15 min.
- Bench (640 prompts, 1024/1024, concurrency 64): **output 1,258.84 tok/s,
  median TPOT 47.92 ms, median TTFT 184.56 ms, 640/640 requests**, 304.8 s.
  (H200 — not directly comparable to the Slurm H100 487.81 tok/s baseline.)

## 7. Observability (DCGM → managed Prometheus → AMW)

```bash
azcluster monitor aksm5   # prints the AMG Grafana URL
```

- DCGM ServiceMonitor (`azmonitoring.coreos.com/v1`, `app: nvidia-dcgm-exporter`,
  port `gpu-metrics`) applied by the M5 deploy stage; ama-metrics scrapes it into the AMW.
- `count(DCGM_FI_DEV_GPU_UTIL)` against the AMW Prometheus query endpoint = **16**
  (all 16 H200 GPUs). AMG is linked to the AMW (Monitoring Data Reader) so the metrics
  are queryable in Grafana.

## 8. Tear-down

```bash
azcluster delete aksm5 --yes
azcluster purge-kv --name aksm5 --location mexicocentral --yes
```
