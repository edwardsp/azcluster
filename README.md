# azcluster

Fast Rust-based Slurm cluster deployer for Azure. Slurm + Pyxis + Enroot for containerised AI workloads on NDv5 H100/H200. One CLI invocation, ~7-15 minutes wall-clock, no daemons on your laptop.

> **Status (v0.13.0)**: phases 0-3 + Slurm accounting (managed MySQL + slurmdbd) shipped. Live-validated on `southafricanorth`. Next milestone: validate the existing multi-node NCCL all-reduce sample sbatch on a live 2-node NDv5 H200 pool, then ship the v0.14+ usability backlog (no-tunnel scaling, health checks, LDAP/Entra, per-SKU NCCL defaults).

## Why azcluster

- **Single binary.** Rust CLI shells out to `az` and Bicep. No Python venv, no agent, no laptop-side controller.
- **AI-first defaults.** Default GPU pool is `Standard_ND96isr_H200_v5` with IB + NCCL tunings preconfigured. Pyxis + Enroot wired from boot: `srun --container-image=docker://...` works the moment a node registers.
- **Multi-pool, dynamic Slurm.** One VMSS Flex per pool. Nodes register via `slurmd --conf-server` and self-tag with `Feature=pool_<name>`; `slurm.conf` `NodeSet+PartitionName` maps them into partitions.
- **Managed observability out of the box.** Azure Monitor Workspace (Managed Prometheus) + Azure Managed Grafana, per-VM `prometheus` remote-writing via `azuread.managed_identity`. Three dashboards (node health, Slurm, GPU+IB) auto-imported post-deploy.
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
| `azcluster scale <name> <pool> <current/target>` | ✅ | — | — | Live-tested (v0.1.x). Requires `azcluster tunnel <name>` running in a second shell — operator → `localhost:8443` → scheduler `:8443` → `az vmss scale --new-capacity` |
| `azcluster validate` (sinfo + srun + Pyxis) | ✅ | — | `--gpu`, `--no-container` | Run every release; v0.12.1 green |
| `azcluster ssh`, `tunnel`, `exec`, `logs`, `status`, `monitor`, `delete` | ✅ | — | — | Used daily during validation |
| `azcluster timings` (per-deploy ARM timings, JSON + trend.tsv) | ✅ | — | `--last N`, `--trend` | Live-validated v0.12.0/.1 (18 resources, 417s on mon6) |
| GPU pool — NCCL + IB + dcgm-exporter wiring | 🟧 | auto-applied on H100/H200 SKUs | — | Bootstrap configured (NCCL env vars in `/etc/profile.d/nccl-azcluster.sh`, IB topology file path, dcgm-exporter unit). **Never live-tested on real H100/H200 in v0.11/v0.12.** All recent validations used `Standard_D8as_v5`. |
| Multi-node NCCL all-reduce | 🟡 | — | — | A sample sbatch is written to `/shared/examples/nccl-allreduce.sbatch` and invokes `/opt/nccl-tests/build/all_reduce_perf`, which is **pre-built into the `microsoft-dsvm:ubuntu-hpc` image** (no extra compile step). The sample has not been run end-to-end on a live H100/H200 pool against this codebase, but the prerequisites are in place. |
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

**Not exercised in v0.12.1**: GPU pool of any kind, NCCL (single- or multi-node), AMLFS, full ANF path, monitoring/Grafana dashboards. Monitoring was validated in v0.11.4, ANF + Pyxis in v0.1.x–v0.2.x. **NCCL all-reduce has never been run end-to-end against this repo on real H100/H200 hardware in any release**, though `/opt/nccl-tests/build/all_reduce_perf` is pre-built in the `microsoft-dsvm:ubuntu-hpc` image so the sample sbatch should run once a GPU pool is deployed. Validating a 2-node NDv5 H200 all-reduce is on the v0.13.x checklist.


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
VERSION=v0.13.1
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
  --pool name=gpu,sku=Standard_ND96isr_H200_v5,count=0 \
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
| `azcluster deploy …` | Provision the cluster (ARM sub deployment). |
| `azcluster status <name>` | Show pool capacities and resource summary. |
| `azcluster scale <name> <pool> <current/target>` | Resize a pool: e.g. `azcluster scale demo gpu 0/2`. |
| `azcluster ssh <name> [--scheduler]` | Interactive shell on login; `--scheduler` proxy-jumps to scheduler. |
| `azcluster exec <name> [--scheduler] -- <cmd>` | One-shot command. |
| `azcluster tunnel <name> <local:remote>` | Forward a local port through login. |
| `azcluster validate <name> [--gpu] [--no-container]` | sinfo + `srun hostname` + Pyxis `srun --container-image=docker://alpine` (+ optional `nvidia-smi`). |
| `azcluster logs <name> --component {scheduler\|login\|<node>} [--tail N\|--follow]` | Tail `/var/log/azcluster/install.log` or `journalctl` over SSH. |
| `azcluster monitor <name>` | Print the AMG Grafana URL for this cluster. |
| `azcluster timings <name> [--last N] [--trend]` | Per-resource deploy times; sorted table or trend TSV. |
| `azcluster delete <name>` | Delete the resource group (async). |

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

- **v0.13.x** — Live-validate the new Slurm accounting backend end-to-end on a 2-node H200 cluster; run the existing NCCL all-reduce sample sbatch and confirm `sacct` populates.
- **v0.14+** — backlog:
  - **Better scaling.** Drop the `azcluster tunnel` requirement. Either run a daemon on the scheduler that reconciles `--pool` capacity directly via the ARM/Compute REST APIs (no `az` shell-out), or wire Slurm's power-save plugin (`SuspendProgram`/`ResumeProgram`) to `az vmss scale` so Slurm itself sizes pools based on queued work.
  - **Health checks.** Port the patterns from [`edwardsp/azhealthcheck`](https://github.com/edwardsp/azhealthcheck) into a small Rust binary shipped with the release tarball. Compute nodes invoke it via Slurm `HealthCheckProgram`; failures drain the node automatically.
  - **User management.** Add a directory backend so user accounts aren't local-only. Options: stand up `slapd` on the scheduler (LDAP) and `sssd` on every node, or — preferred — federate against Microsoft Entra ID (Azure AD DS join or `aad-login` PAM module).
  - **Multi-node container validation.** Extend `azcluster validate` (or a new `validate-mpi` subcommand) to run a 2-node Pyxis + MPI smoke job, not just single-node `srun --container-image=…`.
  - **NCCL env vars per SKU.** The current `/etc/profile.d/nccl-azcluster.sh` hardcodes NDv5-flavoured settings (`NCCL_IB_HCA=mlx5`, `NCCL_TOPO_FILE=/opt/microsoft/ndv5-topo.xml`). These are wrong for GB-series (no static topo file needed) and arguably should be a user concern. Either dispatch on SKU family at boot, or drop the file and document the recommended exports per SKU.
  - **Spot pools** where SKU supports it; optional shared-home (ANF home volume).

## License

TBD.
