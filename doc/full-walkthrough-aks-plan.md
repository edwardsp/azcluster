# AKS target — canonical walkthrough plan (version-agnostic)

The reproducible end-to-end recipe for the `azcluster --target aks` deployment
target, mirroring [`full-walkthrough-slurm-plan.md`](full-walkthrough-slurm-plan.md)
AKS-native. Version-specific live captures live alongside (e.g.
[`full-walkthrough-aks-v0.25.0.md`](full-walkthrough-aks-v0.25.0.md)).

**AKS is pod-native.** There is no bare-metal job path, so each Slurm step maps to a
Kubernetes equivalent: an `azcluster` verb (`validate`/`train`) or an
[`examples/aks/`](../examples/aks/) manifest run with `kubectl`/`helm`. The Slurm
walkthrough's two NCCL runs (plain-VM + container) collapse to the single
containerised `azcluster validate` MPIJob — there is no plain-VM variant on AKS.

Convention: cluster name `aks<MAJOR><MINOR><PATCH>` (e.g. `aksm5`), 2× ND H200 in a
region with H200 capacity (`mexicocentral` as of v0.25.0). Managed Grafana is not in
every region — pin `--grafana-location` to a supported region (e.g. `eastus2`).

---

## 0. Login + deploy

```bash
azcluster login                                  # once; browser PKCE (or --device-code)

azcluster deploy --target aks --name <name> \
  --location mexicocentral --grafana-location eastus2 \
  --pool name=gpu,sku=Standard_ND96isr_H200_v5,count=2,default

azcluster kubeconfig <name>                      # admin kubeconfig -> ~/.azcluster/kube/<name>.config
export KUBECONFIG=~/.azcluster/kube/<name>.config
azcluster list                                   # TARGET column shows `aks`
azcluster status <name>                          # nodes Ready; storage account / data container / kubelet client id
```

Deploy provisions AKS + system + ND GPU pool, NVIDIA Network + GPU operators,
Kueue, MPI Operator, Azure Container Storage (`local-csi`), the per-cluster Blob
account, and — unless `--no-monitoring` — AMW + AMG + the managed-Prometheus
DCE/DCR/DCRA chain + the DCGM ServiceMonitor stage.

Capture the env the examples need:

```bash
export MI_CLIENT_ID=$(azcluster status <name> | awk '/kubelet client id:/{print $4}')
export STORAGE_ACCOUNT=$(azcluster status <name> | awk '/storage account:/{print $3}')
export DATA_URL=$(azcluster status <name> | awk '/data container:/{print $3}')
```

## 1. Native operate client (no laptop `kubectl`)

`exec`/`logs`/`ssh` speak to the API server directly (client-cert TLS + WebSocket;
no `kube-rs`):

```bash
azcluster exec <name> --host gpu-operator/<dcgm-pod> -- nvidia-smi -L   # 8x H200
azcluster logs <name> --component gpu-operator/<dcgm-pod> --tail 20
azcluster ssh  <name> --host <gpu-node>                                 # host-root chroot shell
```

`azcluster tunnel <name>` (AKS) is deferred — the Kubernetes WebSocket port-forward
uses the `SPDY/3.1+portforward.k8s.io` tunneling subprotocol (a SPDY framing layer);
it bails with the interim `kubectl port-forward` command.

## 2. NCCL validation (2-node, container)

```bash
azcluster validate <name>
```

Submits a 2-node MPIJob through Kueue (`all_reduce_perf_mpi -b 16G -e 16G -f 2 -g 1 -N 10`,
16 ranks). Gate: avg busbw ≥ 400 GB/s, ≥ 8 IB/SHARP devices per node, no TCP fallback.

## 3. Training (DGXC Megatron-Bridge, Llama-3.1-8B)

```bash
azcluster train <name> --wait
```

PyTorchJob, 16 GPUs / 2 nodes; reports steady-state `MODEL_TFLOP/s/GPU`.

## 4. Stage a model to Blob — [`examples/aks/stage-model.yaml`](../examples/aks/stage-model.yaml)

One-time per model (ACStor NVMe scratch → `hf download` → `azcp copy`):

```bash
kubectl create secret generic hf-token --from-file=token=hf_token.txt   # optional for public repos
export JOB_NAME=stage-llama MODEL_REPO=neuralmagic/Meta-Llama-3.1-8B-Instruct-FP8 \
       DEST_PREFIX=llama-3.1-8b-fp8 SCRATCH_SIZE=60Gi
envsubst '${JOB_NAME} ${MODEL_REPO} ${DEST_PREFIX} ${SCRATCH_SIZE} ${MI_CLIENT_ID} ${DATA_URL}' \
  < examples/aks/stage-model.yaml | kubectl apply -f -
kubectl wait --for=condition=complete job/${JOB_NAME} --timeout=3600s    # STAGE_OK
# DeepSeek-R1-0528 (~640 GB): MODEL_REPO=deepseek-ai/DeepSeek-R1-0528 DEST_PREFIX=dsr1-fp8 SCRATCH_SIZE=900Gi
```

## 5. Consume over InfiniBand — [`examples/aks/blobcache-rdma.yaml`](../examples/aks/blobcache-rdma.yaml)

2-node blobcache (UCX/RDMA peer transport, cache on ACStor NVMe). Hydrate shards the
origin fetch across both nodes; reading on node-1 pulls missing chunks from node-0
over IB.

```bash
envsubst '${STORAGE_ACCOUNT} ${MI_CLIENT_ID}' < examples/aks/blobcache-rdma.yaml | kubectl apply -f -
kubectl rollout status statefulset/blobcache-rdma --timeout=300s
kubectl exec blobcache-rdma-0 -- curl -s -XPOST 127.0.0.1:7773/hydrate \
  -H content-type:application/json -d '{"mount":"data","path":"models/llama-3.1-8b-fp8","recursive":true}'
kubectl exec blobcache-rdma-1 -- curl -s 127.0.0.1:7773/metrics | grep blobcache_chunk_peer_fetch_seconds_count   # > 0
```

## 6. Inference — vLLM — [`examples/aks/inference-vllm.yaml`](../examples/aks/inference-vllm.yaml)

Single-node vLLM serving Llama-3.1-8B-FP8 from a per-job blobcache sidecar, then
`vllm bench serve` at concurrency 128:

```bash
envsubst '${STORAGE_ACCOUNT} ${MI_CLIENT_ID}' < examples/aks/inference-vllm.yaml | kubectl apply -f -
kubectl logs -f job/inference-vllm -c vllm
```

## 7. Inference — DeepSeek-R1 SGLang TP=16 — [`examples/aks/inference-sglang-multinode.yaml`](../examples/aks/inference-sglang-multinode.yaml)

2-node tensor-parallel (TP=16) serving; blobcache distributes the 640 GB model over
IB. Serve-and-stay, then bench against the warm `/health`-200 server. **Bench
gotchas:** do NOT set `HF_HUB_OFFLINE=1` (the random sampler pulls ShareGPT from the
Hub); strip the DeepSeek tokenizer `auto_map` (see the manifest header for the exact
prep). First-run DeepGEMM JIT is the 20-40 min long pole.

```bash
export MODEL_PREFIX=dsr1-fp8
envsubst '${STORAGE_ACCOUNT} ${MI_CLIENT_ID} ${MODEL_PREFIX}' < examples/aks/inference-sglang-multinode.yaml | kubectl apply -f -
kubectl rollout status statefulset/sglang --timeout=600s
# ...prep /tmp/tok (manifest header), then:
kubectl exec sglang-0 -c sglang -- python3 -m sglang.bench_serving --backend sglang \
  --host 127.0.0.1 --port 8888 --model model --tokenizer /tmp/tok \
  --dataset-name random --random-input-len 1024 --random-output-len 1024 \
  --random-range-ratio 0.2 --num-prompts 640 --max-concurrency 64
```

## 8. Observability

```bash
azcluster monitor <name>      # AMG Grafana URL
```

DCGM (`azmonitoring.coreos.com/v1` ServiceMonitor, `gpu-operator` ns) → ama-metrics
→ AMW. Verify: `count(DCGM_FI_DEV_GPU_UTIL)` against the AMW Prometheus query endpoint
equals the GPU count (16 on 2× ND H200). AMG is linked to the AMW (Monitoring Data
Reader) so the metrics are queryable in Grafana.

## 9. Tear-down

```bash
azcluster delete <name> --yes
azcluster purge-kv --name <name> --location <region> --yes
```

---

## Slurm vs AKS — per-test results (NOT a controlled comparison)

> ⚠️ **Read each column as an independent in-context result, not a head-to-head.**
> These captures differ in three confounded ways, so the deltas do **not** isolate
> "Slurm vs AKS":
> - **Hardware:** Slurm [`...-slurm-v0.24.20.md`](full-walkthrough-slurm-v0.24.20.md)
>   ran on **2× ND H100** (eastus); AKS [`...-aks-v0.25.0.md`](full-walkthrough-aks-v0.25.0.md)
>   on **2× ND H200** (mexicocentral). H200 is the *same compute die* as H100 — only
>   ~1.4× memory bandwidth + 1.76× capacity, so it does **not** explain a 2× throughput jump.
> - **Software:** different container/library versions (notably a much newer SGLang on
>   AKS, which alone accounts for most of the DeepSeek delta — TPOT 123→48 ms is far
>   more than memory bandwidth predicts), and H200's larger KV-cache capacity lets it
>   *sustain* high concurrency on the 671B model where H100 is capacity-throttled.
> - **Harness:** the **training** rows are not the same benchmark — Slurm used the DGXC
>   `dgxc-benchmarking` `llmb-run` harness; AKS used `azcluster train` (the CLI's own
>   embedded Megatron-Bridge pretrain). Same model/precision/gbs nominally, different harness.
>
> A clean apples-to-apples comparison would require the same GPU SKU, the same container
> image/library versions, and the same benchmark harness + bench parameters on both targets.

| Test | Slurm (H100) | AKS (H200) |
|---|---|---|
| NCCL all-reduce (2-node, 16 GiB) | 440.21 GB/s plain-VM · 451.08 GB/s container | 483.36 GB/s container |
| IB/SHARP | 8 NICs/node, no TCP fallback | 8 NICs/node, no TCP fallback |
| Training Llama-3.1-8B (16 GPU) — **different harness** | 541.81 TFLOP/s/GPU (DGXC llmb-run) | 506.4 TFLOP/s/GPU (`azcluster train`) |
| vLLM Llama-3.1-8B-FP8 | 9,863 tok/s @ 12.38 ms TPOT | 9,912 tok/s @ 12.55 ms TPOT |
| DeepSeek-R1 SGLang TP=16 (640 prompts, c64) — **diff. SGLang ver.** | 487.81 tok/s @ 123.34 ms TPOT | 1,258.84 tok/s @ 47.92 ms TPOT |
| Storage stage (azcp upload) | ~8.78 Gbps | 15.91 Gbps |
| Model distribution over IB | azcp-cluster MPI broadcast | blobcache RDMA peer-fetch |
| Observability | remote-write → AMW + Grafana | DCGM → managed Prometheus → AMW, `count=16` |
