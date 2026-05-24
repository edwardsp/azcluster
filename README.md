# azcluster

Fast Rust-based Slurm cluster deployer for Azure. Slurm + Pyxis + Enroot for containerised AI workloads on NDv5 H100. One CLI invocation, ~7-15 minutes wall-clock, no daemons on your laptop.

> **Status (v0.19.1)**: phases 0-3 + Slurm accounting + GPU pool + end-to-end DGXC Llama 3.1 8B BF16 live-validated. Llama 3.1 8B BF16 trains at **167,594 tok/s on 16 H100 (2 node)** and **83,737 tok/s on 8 H100 (1 node)** via `llmb-run submit` against the DGXC v25.11 toolchain — strong scaling 2.001×. Cross-node containerised NCCL all-reduce inside `nvcr.io/nvidia/nemo:25.07.02` (16-rank, 2 node × 8 H100, SHARP + GPUDirect RDMA) is live-validated end-to-end. **v0.19 ships deploy-UX**: `azcluster deploy --no-wait` submits the ARM deployment and returns immediately; run `azcluster resume --name <name>` afterwards to wait for ARM and run all post-deploy hooks (state file, timings JSON, Grafana dashboard import). Blocking deploys also write the pending marker first, so a terminal interrupt mid-ARM is recoverable via the same `azcluster resume` command. `azcluster status` surfaces ARM phase + operation roll-up + 8 s SSH bootstrap probe and nags about pending markers. `azcluster delete` works on pending-only clusters. Entra ID (`aad-login`) integration deferred to v0.19.2 / v0.20. Full DGXC workflow: [walkthrough-dgxc.md](walkthrough-dgxc.md). Health-check internals: [healthchecks.md](healthchecks.md). Next backlog: `aad-login` Entra integration, DCGM-backed NVLink/throttle checks, Slurm power-save autoscaling.

## Why azcluster

- **Single binary.** Rust CLI shells out to `az` and Bicep. No Python venv, no agent, no laptop-side controller.
- **AI-first defaults.** Default GPU pool is `Standard_ND96isr_H100_v5` with IB + NCCL tunings preconfigured. Pyxis + Enroot wired from boot: `srun --container-image=docker://...` works the moment a node registers.
- **Multi-pool, dynamic Slurm.** One VMSS Flex per pool. Nodes register via `slurmd --conf-server` and self-tag with `Feature=pool_<name>`; `slurm.conf` `NodeSet+PartitionName` maps them into partitions.
- **Managed observability out of the box.** Azure Monitor Workspace (Managed Prometheus) + Azure Managed Grafana, per-VM `prometheus` remote-writing via `azuread.managed_identity`. Four dashboards (node health, Slurm, GPU+IB, healthcheck) auto-imported post-deploy.
- **Observable provisioning.** Every deploy captures per-resource Azure Resource Manager timings to `~/.config/azcluster/deployments/<cluster>/`. `azcluster timings` prints a sorted table and a trend across runs.
- **Test mode that's actually fast.** `--shared-storage nfs-scheduler --no-monitoring --no-accounting` deploys a functional 1-CPU cluster in ~7 minutes (vs ~15 with ANF + AMW + AMG).

## Feature Matrix

Legend: ✅ implemented & live-validated · 🟡 implemented, not live-tested this release · 🟧 CLI surface only (backend not wired) · 🔜 roadmap

| Area | Status | Default | Options | Notes |
|---|---|---|---|---|
| Control plane (scheduler VM + `azcluster-server:8443`) | ✅ | on | — | Co-located `slurmctld` + control daemon |
| Login VM | ✅ | no public IP | `--login-public-ip`, `--allowed-ssh-cidrs` | Egress via NAT Gateway |
| Compute pools (`--pool` repeatable) | ✅ | none | any VM SKU, count, optional `default` | One VMSS Flex per pool |
| Multi-pool partitions (CPU + GPU side by side) | ✅ | — | — | Dynamic `NodeSet+Feature` mapping in `slurm.conf` |
| Pyxis + Enroot containers | ✅ | on | — | `srun --container-image=docker://…` validated end-to-end |
| Image | ✅ | `microsoft-dsvm:ubuntu-hpc:2404` | `--ubuntu 2204` | DSVM HPC image (drivers, IB, MOFED) |
| Shared FS — ANF NFSv4.1 | ✅ | on (2 TiB Standard) | `--anf-size-tib`, `--anf-tier {Standard,Premium,Ultra}` | Mounted on `/shared` |
| Shared FS — NFS on scheduler (test mode) | ✅ | off | `--shared-storage nfs-scheduler` | SPOF, ~12 min faster, test only |
| AMLFS (Lustre) at `/amlfs` | 🟡 | off | `--amlfs-size-tib`, `--amlfs-sku`, `--amlfs-zone` | Provisioning path validated in earlier phases; not exercised since v0.10 |
| `azcluster scale <name> <pool> <current/target>` | ✅ | — | — | New in v0.14: CLI calls `az vmss scale --new-capacity` directly using the operator's `az` login. No tunnel, no scheduler-side daemon involvement. Operator needs `Microsoft.Compute/virtualMachineScaleSets/write` on the resource group. |
| `azcluster validate` (sinfo + srun + Pyxis) | ✅ | — | `--gpu`, `--no-container` | Run every release; v0.12.1 green |
| `azcluster ssh`, `tunnel`, `exec`, `logs`, `status`, `monitor`, `delete` | ✅ | — | — | Used daily during validation |
| `azcluster timings` (per-deploy ARM timings, JSON + trend.tsv) | ✅ | — | `--last N`, `--trend` | Live-validated v0.12.0/.1 (18 resources, 417s on mon6) |
| GPU pool — NCCL + IB + dcgm-exporter wiring | ✅ | auto-applied on H100 SKUs | — | Bootstrap configured (NCCL env vars in `/etc/profile.d/nccl-azcluster.sh`, IB topology file path, dcgm-exporter unit). Live-validated v0.13.4 on `Standard_ND96isr_H100_v5` x2. |
| Multi-node NCCL all-reduce (bare-metal, HPC-X) | ✅ | — | — | `/shared/examples/nccl-allreduce.sbatch` uses HPC-X (in-image, PMIx 4.x) + prebuilt `/opt/nccl-tests/build/all_reduce_perf`. Live-exercised v0.13.4 on 2× ND96isr_H100_v5 / 8× NDR400 IB. |
| Multi-node NCCL all-reduce (Pyxis container) | ✅ | — | — | New in v0.13.6. `/shared/examples/dgxc-nemo-multinode-smoke.sbatch` runs 2-node × 8-GPU = 16-rank NCCL all-reduce inside `nvcr.io/nvidia/nemo:25.07.02` via `srun --mpi=pmix --container-image=...`. Enabled by the CCWS-style runtime fix: slurmd exports `PMIX_MCA_ptl=^usock`, `PMIX_MCA_psec=none`, `PMIX_SYSTEM_TMPDIR=/var/empty`, `PMIX_MCA_gds=hash`, `HWLOC_COMPONENTS=-opencl`; upstream NVIDIA enroot hooks `50-slurm-pmi.sh` + `50-slurm-pytorch.sh` (pinned in-tree, Apache 2.0) propagate `PMIX_*`/`SLURM_*` env + bind-mount `$PMIX_SERVER_TMPDIR` into the container. All NGC containers ship HPC-X 2.20-2.26 → PMIx 4.2.x (matches host `mpi_pmix_v4.so`). |
| Monitoring — Managed Prometheus (AMW) + Managed Grafana (AMG) | ✅ | on | `--no-monitoring`, `--grafana-location` | Per-VM `prometheus` → AMW DCE via `azuread.managed_identity`. 3 dashboards (node, slurm, gpu+ib) auto-imported with retry on RBAC propagation. Live-validated v0.11.4. |
| Slurm accounting (Azure DB for MySQL Flex + slurmdbd) | 🟡 | `--accounting=true` | `--no-accounting` | New in v0.13.0. `Standard_B2ms` MySQL Flexible Server (MySQL 8.0.21, 50 GB autogrow, public access disabled, VNet-integrated on delegated `10.42.8.0/29`); CLI auto-generates the admin password and threads it as a secure Bicep param. Scheduler runs `slurmdbd` over TLS (DigiCert Global Root CA) on `localhost:6819`; `slurm.conf` has `AccountingStorageType=accounting_storage/slurmdbd` + `AccountingStorageEnforce=associations,limits,qos` + `JobAcctGatherType=jobacct_gather/cgroup`. Built and bicep-clean, but not yet end-to-end live-validated against a real cluster (next checklist item). |
| Autoscaling (Slurm power-save → VMSS resize) | 🔜 | — | — | Roadmap, not implemented. Use `azcluster scale` manually. |
| Spot pools | 🔜 (out of scope for now) | — | — | Not all target SKUs support Spot. |
| Distribution via GitHub Releases | ✅ | `edwardsp/azcluster@v0.12.1` | `--azcluster-version`, `--azcluster-repo` | CI builds x86_64-linux + aarch64-darwin on tag |

### What "live-validated v0.12.1" actually means

The most recent end-to-end run (`mon6` on `southafricanorth`, `paul-azcluster-v6`, since deleted):

- `azcluster deploy --shared-storage nfs-scheduler --no-monitoring --no-accounting --pool name=cpu,sku=Standard_D8as_v5,count=1,default --login-public-ip` → succeeded in **417 s**.
- `azcluster validate mon6` → `sinfo` ✅, `srun -N1 hostname` ✅, `srun -N1 --container-image=docker://alpine:latest hostname` (Pyxis import + run) ✅.
- `azcluster timings mon6` → 18 resources captured, sorted table prints, JSON snapshot + `trend.tsv` appended.

**Not exercised in v0.12.1**: GPU pool of any kind, NCCL (single- or multi-node), AMLFS, full ANF path, monitoring/Grafana dashboards. Monitoring was validated in v0.11.4, ANF + Pyxis in v0.1.x–v0.2.x. **NCCL all-reduce has never been run end-to-end against this repo on real H100 hardware in any release**, though `/opt/nccl-tests/build/all_reduce_perf` is pre-built in the `microsoft-dsvm:ubuntu-hpc` image so the sample sbatch should run once a GPU pool is deployed. Validating a 2-node NDv5 H100 all-reduce is on the v0.13.x checklist.


## Architecture

```
            ┌──────────────── subscription / resource group ────────────────┐
            │                                                                │
  operator  │   ┌─────────────┐         ┌───────────────────────────────┐    │
   ── ssh ──┼──▶│  login VM   │         │  Azure Monitor Workspace      │    │
            │   │  (NIC + opt │         │  (Managed Prometheus)         │◀┐  │
            │   │   public IP)│         └───────────────┬───────────────┘ │  │
            │   └──────┬──────┘                         │ remote_write    │  │
            │          │ ssh (ProxyJump)                │ (managed-id     │  │
            │          ▼                                │  bearer token)  │  │
            │   ┌─────────────┐                         │                 │  │
            │   │ scheduler VM│  munge.key, slurm.conf  │                 │  │
            │   │  slurmctld  │◀──── NFSv4.1 ──────┐   ┌┴────────────┐    │  │
            │   │  azcluster- │       /shared      │   │   prometheus│    │  │
            │   │   server    │                    │   │   (on every │    │  │
            │   └──────┬──────┘                    │   │   VM, scrapes│   │  │
            │          │ --conf-server             │   │   local exps)│   │  │
            │          ▼                           │   └─┬──────────┬─┘   │  │
            │   ┌──────────────────────────┐       │     │          │     │  │
            │   │ VMSS Flex: pool=cpu      │───────┤     │          │     │  │
            │   │   slurmd --conf-server   │       │     │          │     │  │
            │   │   Feature=pool_cpu       │       │     │          │     │  │
            │   └──────────────────────────┘       │     │          │     │  │
            │   ┌──────────────────────────┐       │   ┌─┴───────┐ ┌┴──────┐│  │
            │   │ VMSS Flex: pool=gpu      │───────┤   │ node_exp│ │slurm_ ││  │
            │   │   slurmd + NCCL + IB     │       │   │ (all)   │ │exp    ││  │
            │   │   dcgm-exporter          │       │   │ dcgm_exp│ │(sched)││  │
            │   │   Feature=pool_gpu      │       │   │ (gpu)   │ │       ││  │
            │   └──────────┬───────────────┘       │   └─────────┘ └───────┘│  │
            │              │ optional               │                        │  │
            │              ▼                        │   ┌─────────────────┐  │  │
            │        ┌──────────┐                   │   │ Azure Managed   │◀─┘  │
            │        │  AMLFS   │  Lustre /amlfs    │   │ Grafana (AMG)   │     │
            │        │ (Lustre) │                   │   │ + 3 dashboards  │     │
            │        └──────────┘                   │   └─────────────────┘     │
            │                                       │                            │
            │   ┌────────── NAT Gateway ────────────┴───── egress ───┐          │
            │   │  scheduler subnet, login subnet, compute subnet     │          │
            │   └─────────────────────────────────────────────────────┘          │
            └────────────────────────────────────────────────────────────────────┘
```

**Network plan** (VNet `10.42.0.0/16`):

| Subnet | CIDR | First usable | Workload |
|---|---|---|---|
| `scheduler` | `10.42.1.0/24` | `10.42.1.4` | scheduler VM + control plane (`8443`, `6817`) |
| `login` | `10.42.2.0/24` | `10.42.2.4` | login VM |
| `amlfs` | `10.42.3.0/24` | — | optional Lustre MGS/MDS/OST |
| `compute` | `10.42.4.0/22` | `10.42.4.4` | VMSS Flex compute nodes (all pools) |
| `anf` | `10.42.0.0/26` | — | ANF delegated subnet |
| `database` | `10.42.8.0/29` | — | MySQL Flexible Server delegated subnet (when `--accounting` on) |

**Identity & RBAC** (cluster scope):

- A `uai-<cluster>-scheduler` user-assigned identity is attached to compute VMSS (AzSecPack policies reject `SystemAssigned` on VMSS Flex).
- When monitoring is on, the same UAI gets `Monitoring Metrics Publisher` (GUID `3913510d-42f4-4e42-8a64-420c390055eb`) on the **AMW's default Data Collection Rule** in the Azure-managed sister RG `MA_<amwName>_<location>_managed`.
- The deployer principal gets `Grafana Admin` (`22926164-76b3-42b3-bc55-97df8dab3e41`) on AMG so the CLI can `POST /api/dashboards/db` after deploy with retry-on-RBAC-propagation.

**Distribution**: CI builds release artifacts on tag (`v*`): `azcluster-cli-{x86_64-linux,aarch64-darwin}`, `azcluster-server-x86_64-linux`, `spank_pyxis-vX.Y.Z-x86_64-linux.so`, versioned tarball, `SHA256SUMS`. Cloud-init on each node fetches the tarball from GitHub Releases, verifies SHA256, and starts the relevant systemd unit.

## Prerequisites

- `az` CLI logged in (`az login`)
- `jq`
- SSH key (`~/.ssh/id_ed25519.pub` or `~/.ssh/id_rsa.pub`)
- Permissions to create resource groups, role assignments, and Monitor/Grafana resources in the target subscription

## Install

Grab the prebuilt CLI from the latest release:

```bash
VERSION=v0.19.1
ARCH=x86_64-linux                       # or aarch64-darwin
curl -fsSL -o azcluster \
  https://github.com/edwardsp/azcluster/releases/download/${VERSION}/azcluster-cli-${ARCH}
chmod +x azcluster && sudo mv azcluster /usr/local/bin/
azcluster version
```

Or build from source: `cargo build --release --workspace` → `target/release/azcluster`.

## Usage

### Production-style deploy (ANF + monitoring on)

```bash
azcluster deploy \
  --name demo \
  --location southafricanorth \
  --grafana-location uksouth \
  --resource-group my-rg \
  --pool name=cpu,sku=Standard_D8as_v5,count=2,default \
  --pool name=gpu,sku=Standard_ND96isr_H100_v5,count=0 \
  --anf-size-tib 4 --anf-tier Premium \
  --login-public-ip
```

`--grafana-location` defaults to `--location`. Override when the cluster region does not host Azure Managed Grafana (e.g. `southafricanorth` → `uksouth`).

### Rapid-test deploy (~7 min)

```bash
azcluster deploy \
  --name demo \
  --location southafricanorth \
  --resource-group my-rg \
  --shared-storage nfs-scheduler \
  --no-monitoring --no-accounting \
  --pool name=cpu,sku=Standard_D8as_v5,count=1,default \
  --login-public-ip
```

`nfs-scheduler` exports `/shared` from the scheduler VM. SPOF, test only.

### Fire-and-forget deploy (`--no-wait` + `resume`)

Add `--no-wait` to any deploy command to submit the ARM deployment and return immediately. The CLI persists secrets + a pending marker at `~/.config/azcluster/clusters/<name>-pending.toml`, then exits. Run `azcluster resume --name <name>` afterwards (any time within 90 minutes) to wait for ARM and run post-deploy hooks (state file, timings JSON, Grafana dashboard import). Track progress meanwhile with `azcluster status <name>`, which prints ARM provisioning state, operation roll-up, and an 8 s SSH probe of `/var/log/azcluster/install.log` on login + scheduler.

Blocking deploys also write the pending marker before submitting ARM, so a terminal interrupt mid-deploy is recoverable via the same `azcluster resume` command.

```bash
azcluster deploy --name demo --location southafricanorth --no-wait \
  --resource-group my-rg \
  --shared-storage nfs-scheduler --no-monitoring --no-accounting \
  --pool name=cpu,sku=Standard_D8as_v5,count=1,default --login-public-ip
# ... go for coffee ...
azcluster status demo                  # ARM phase + per-node cloud-init progress
azcluster resume --name demo         # waits for ARM, runs post-deploy hooks
```

### Add Lustre (AMLFS) on top of ANF

```bash
azcluster deploy ... \
  --amlfs-size-tib 4 \
  --amlfs-sku AMLFS-Durable-Premium-250 \
  --amlfs-zone 1
```

Mounted on login + compute at `/amlfs`.

### Lifecycle

| Command | Purpose |
|---|---|
| `azcluster deploy …` | Provision the cluster (ARM sub deployment). Add `--no-wait` to return immediately after submission; run `azcluster resume` later. |
| `azcluster resume --name <name>` | Wait for a `--no-wait` (or interrupted) ARM deployment to reach a terminal state and run post-deploy hooks (state file, timings JSON, Grafana dashboard import). |
| `azcluster status <name>` | Show pool capacities and resource summary. Nags about pending markers if any. |
| `azcluster scale <name> <pool> <current/target>` | Resize a pool: e.g. `azcluster scale demo gpu 0/2`. |
| `azcluster ssh <name> [--scheduler]` | Interactive shell on login; `--scheduler` proxy-jumps to scheduler. |
| `azcluster exec <name> [--scheduler] -- <cmd>` | One-shot command. |
| `azcluster tunnel <name> <local:remote>` | Forward a local port through login. |
| `azcluster validate <name> [--gpu] [--no-container] [--multi-node] [--partition <p>]` | sinfo + `srun hostname` + Pyxis `srun --container-image=docker://alpine` (+ optional `nvidia-smi`). With `--multi-node`: cross-node `srun -N2`, cross-node Pyxis launch, and (with `--gpu`) a bounded 2-node NCCL all-reduce via HPC-X (NDv5-tuned). |
| `azcluster logs <name> --component {scheduler\|login\|<node>} [--tail N\|--follow]` | Tail `/var/log/azcluster/install.log` or `journalctl` over SSH. |
| `azcluster monitor <name>` | Print the AMG Grafana URL for this cluster. |
| `azcluster timings <name> [--last N] [--trend]` | Per-resource deploy times; sorted table or trend TSV. |
| `azcluster delete <name>` | Delete the resource group (async). |
| `azcluster user add <name> --username <u> [--ssh-key <path>]` | Create an LDAP user (auto-allocated UID, default gid 20000, home `/shared/home/<u>`). |
| `azcluster user remove <name> --username <u>` | Delete an LDAP user. |
| `azcluster user list <name>` | List LDAP users. |
| `azcluster user sshkey {add,remove,list} <name> --username <u> [--key-file <path>]` | Manage `sshPublicKey` LDAP attribute used by `sss_ssh_authorizedkeys`. |

### Submitting jobs

```bash
azcluster ssh demo
sinfo
srun -N1 --partition=cpu hostname
srun -N1 --partition=gpu --container-image=docker://nvcr.io/nvidia/pytorch:24.05-py3 nvidia-smi
sbatch /shared/examples/nccl-allreduce.sbatch     # multi-node NCCL all-reduce template
```

## Repo Layout

```
crates/
  azcluster-core/       domain model (Cluster, NodePool, NodeSku, …)
  azcluster-server/     control-plane daemon (axum) on scheduler
  azcluster-cli/        management CLI (clap) + timings module
bicep/
  main.bicep            subscription-scope entry, creates RG
  cluster.bicep         orchestrates modules
  modules/
    network.bicep       VNet, subnets, NSGs, NAT Gateway
    scheduler.bicep     scheduler VM + UAI
    login.bicep         login VM (+ optional public IP)
    compute.bicep       VMSS Flex per pool
    anf.bicep             Azure NetApp Files (account + capacity pool + volume)
    amlfs.bicep           Azure Managed Lustre (optional)
    accounting.bicep      Azure Database for MySQL Flexible Server + slurm_acct_db
    monitoring.bicep      AMW + AMG + RBAC + DCE
    ingestion-endpoint.bicep   AMW data collection endpoint
cloud-init/
  scheduler.yaml.tmpl   slurmctld, munge, NFS exports (test mode), prometheus
  login.yaml.tmpl       mounts /shared, /amlfs; slurm client + Pyxis spank
  compute.yaml.tmpl     slurmd, Pyxis, Enroot, NCCL+IB tunings, dcgm-exporter
grafana/dashboards/
  node.json             node_exporter health
  slurm.json            slurm scheduler metrics
  gpu_ib.json           dcgm + InfiniBand counters
  health.json           azhealthcheck per-node/per-check severity
.github/workflows/      ci.yml + release.yml
research/               local reference checkouts (gitignored)
.sisyphus/              planning artifacts (gitignored)
CHANGELOG.md            every user-visible change, per release
AGENTS.md               operating manual for AI agents working on this repo
```

## Development

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
for f in bicep/main.bicep bicep/cluster.bicep bicep/modules/*.bicep; do
  az bicep build --file "$f" --stdout > /dev/null
done
```

Live-test region used for v0.x validation: `southafricanorth`. Capacity is tight, so tear deploys down (`azcluster delete <name>`) as soon as validation completes.

## Releasing

Tag-triggered. `CHANGELOG.md` follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). To release: move `Unreleased` content under a new `[X.Y.Z] - YYYY-MM-DD` heading, bump `Cargo.toml` versions and the `--azcluster-version` CLI default, commit, then `git tag vX.Y.Z && git push --tags`. CI publishes the release.

## Roadmap

- **v0.13.x** — ✅ Slurm accounting live-validated. ✅ 2-node NDv5 H100 NCCL all-reduce live-exercised (bare-metal HPC-X path). ✅ v0.13.6 cross-node containerised PMIx world live-validated. ✅ v0.13.7 udev rule opens `/dev/infiniband/uverbs*` to `0666` so `ENROOT_REMAP_ROOT yes` no longer blocks uverbs from container userspace. ✅ v0.13.8 cross-node containerised NCCL uses InfiniBand end-to-end. ✅ v0.13.9 end-to-end DGXC training live-validated: Llama 3.1 8B BF16 reaches 167,594 tok/s on 16 GPU (2 node) / 83,737 tok/s on 8 GPU (1 node) — 2.001× strong scaling. Fixes cross-node PyTorch/Gloo rendezvous (compute `/etc/hosts` now maps the hostname to eth0 IPv4, not `127.0.1.1`) and Slurm conf perms (cloud-init now `chmod 0644` on every `/etc/slurm/*.conf`).
- **v0.14** — ✅ `azcluster scale` no longer requires a separate `azcluster tunnel` shell. CLI invokes `az vmss scale --new-capacity` directly using the operator's existing `az` login, identical to how every other ARM op (deploy/delete/status) already works. Scheduler-side `azcluster-server` keeps `/v1/healthz` as a future hook point; the `/v1/pools/:name/scale` endpoint is removed.
- **v0.14+** — backlog:
  - **Better scaling.** Wire Slurm's power-save plugin (`SuspendProgram`/`ResumeProgram`) to `az vmss scale` so Slurm itself sizes pools based on queued work.
  - **Health checks.** ✅ Shipped in v0.16: `azhealthcheck` Rust binary in the release tarball (`/usr/local/bin/azhealthcheck`) + wrapper at `/usr/local/sbin/azcluster-healthcheck`. Slurm `HealthCheckProgram` runs it every 5 min on every compute node; non-zero exit drains. 5 checks: `gpu_count`, `gpu_xid` (catastrophic + soft XID classification), `network` (eth + IB operstate/carrier + flap), `kmsg` (kernel critical), `systemd` (`slurmd,prometheus,node_exporter` + `dcgm-exporter` on GPU). DCGM-backed checks (NVLink CRC, throttle) deferred to v0.17.
  - **User management.** Add a directory backend so user accounts aren't local-only. Options: stand up `slapd` on the scheduler (LDAP) and `sssd` on every node, or — preferred — federate against Microsoft Entra ID (Azure AD DS join or `aad-login` PAM module).
  - **Multi-node container validation.** ✅ Shipped in v0.15: `azcluster validate --multi-node` (with `--gpu` for the NCCL all-reduce path). NDv5-tuned NCCL env; cross-SKU is part of "NCCL env vars per SKU" below.
  - **NCCL env vars per SKU.** The current `/etc/profile.d/nccl-azcluster.sh` hardcodes NDv5-flavoured settings (`NCCL_IB_HCA=mlx5`, `NCCL_TOPO_FILE=/opt/microsoft/ndv5-topo.xml`). These are wrong for GB-series (no static topo file needed) and arguably should be a user concern. Either dispatch on SKU family at boot, or drop the file and document the recommended exports per SKU.
  - **Spot pools** where SKU supports it; optional shared-home (ANF home volume).

## License

TBD.
