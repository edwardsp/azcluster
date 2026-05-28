# azcluster

Fast Rust-based Slurm cluster deployer for Azure. Slurm + Pyxis + Enroot for containerised AI workloads on NDv5 H100. One CLI invocation, ~7-15 minutes wall-clock, no daemons on your laptop.

> **Status (v0.24.4)**: patch — Grafana dashboard hygiene. Dashboards now land in an `azcluster` folder (was: root). All dashboard queries filter by `nodename` (was: `instance`, which was identical across the fleet — `node.json` IB panels were live-broken and gpu_ib was indistinguishable across nodes). GPU+IB dashboard gained 8 new panels for v0.24.2's DCGM fields: tlimit / HBM3 temp / SM_ACTIVE / PIPE_TENSOR_ACTIVE / throttle violations / throttle reasons / NVLink errors / ECC. Dashboard-import retry no longer caps at 60 min — RBAC propagation can take longer in some Azure regions and giving up early created a bad UX.

> **Previous status (v0.24.3)**: patch — fixes `azcluster ssh|exec|scp --host <compute>` under `--bastion`. The two stacked `-o ProxyCommand=` calls collapsed to one (last-wins) and the connection landed on the login VM instead of the requested compute hostname. Fixed by building one composite ProxyCommand. Live-validated end-to-end on `v24walk2`.

> **Previous status (v0.24.2)**: patch — DCGM metrics expansion + `/shared` permission fix from v24walk2 live-validation. The counters CSV now includes thermal limits (`DCGM_FI_DEV_GPU_MAX_OP_TEMP` = 87°C "tlimit" on H100), ECC quartet (volatile + aggregate, single+double-bit), HBM3 row remap (retired pages + remap failure), and aggregate NVLink error counters (CRC FLIT/DATA, replay, recovery). `/shared` now mounts `chmod 1777` (sticky world-write) — fixes the regression where LDAP users couldn't `mkdir /shared/dgxc` to run the example DGXC sbatches.

> **Previous status (v0.24.1)**: patch — four fixes from the v0.24.0 walkthrough. (N-12) Default LDAP users `clusteradmin` + `clusteruser` now get sacctmgr account + per-partition associations during scheduler bootstrap, so `sbatch` works out-of-box. (N-13) `ArmClient` transparently refreshes the OAuth2 access token on 401 `ExpiredAuthenticationToken` during long polls — `azcluster deploy` no longer fails mid-poll after ~60 min. (N-14) Dashboard import status line detects non-TTY stderr and emits one line per 5 min instead of carriage-return refresh, keeping piped logs readable. (N-16) dcgm-exporter ships a custom counters CSV with `DCGM_FI_PROF_*` profile metrics (SM_ACTIVE, PIPE_TENSOR_ACTIVE, NVLINK_TX/RX_BYTES, etc.) bind-mounted into the enroot container — enables tensor-core utilization charts in Grafana.

> **Previous status (v0.24.0)**: minor — LDAP user model overhaul. Every deploy now provisions two default LDAP users alongside the local `azureuser` admin: `clusteradmin` (sudoer via `cn=cluster-admins` group + `/etc/sudoers.d/cluster-admins`) and `clusteruser` (regular). Both have their `sshPublicKey` seeded with the v0.22 admin pubkey so the deployer can `azcluster ssh <cluster> --user clusteradmin` immediately, no manual key setup. `azcluster user add` now (a) takes `--admin` to grant cluster-admin membership at creation, and (b) auto-generates a per-user ed25519 keypair stored locally at `~/.azcluster/keys/<cluster>-<username>` (mode 0600) — public key goes into LDAP `sshPublicKey` (multi-valued, alongside any `--ssh-key` files). Private keys are NEVER uploaded to Key Vault — only the laptop that runs `user add` has them. New `setadmin`/`unsetadmin` subcommands for promoting/demoting existing users. `azcluster user list` shows admin status.

> **Previous status (v0.23.2)**: patch — six bug-fixes from the v0.23.1 end-to-end walkthrough. (1) `azcluster user {add,remove,sshkey}` is now Bastion-aware, routing direct to scheduler (the LDAP server) rather than failing with `no login public IP`. (2) `azcluster tunnel` also goes direct to scheduler under Bastion. (3) Grafana Admin RBAC propagation no longer fails deploys — single 60-min wait with a rolling status line and non-fatal timeout. (4) DCGM exporter now actually runs on compute nodes — replaced the broken `docker run` block (no docker in the ubuntu-hpc image) with an enroot-based systemd unit talking to nv-hostengine over TCP localhost:5555. GPU metrics flow to AMW. (5) Example sbatch templates no longer hardcode `--partition=cpu` (default azcluster pool is `gpu`, not `cpu`). (6) `azcluster user add` now auto-creates sacctmgr partition associations on every existing partition, so LDAP users can submit jobs without manual `sacctmgr modify user ... set Partition=...`.

> **Previous status (v0.23.1)**: patch — bug-fix release shipping nine fixes uncovered during the first v0.23 end-to-end walkthrough. `azcp` tarball is now extracted with `--strip-components=1` (the v0.4.5 archive nests the binary one dir deep). Storage URL composition gets its missing `/`. `ENROOT_TEMP_PATH` is on `/mnt/nvme/enroot-temp` (was tmpfs, which capped at ~340 GB shared and pushed `mksquashfs` scratch onto the 61 GB root disk → big container imports OOM-ed). `bootcmd:` disables `unattended-upgrades` before `package_update` to dodge the apt-lock race on first boot. Compute Prometheus config now has correctly-indented `dcgm_exporter` scrape block (the old `SCRAPE_GPU` shell-var inline append broke YAML on every restart). Example sbatch templates are `#!/bin/bash -l` so storage env loads. `azcp-cluster` distribute uses per-sqsh subdirs (prefix semantics). Grafana Admin RBAC retry budget bumped from 5 min to 20 min. `azcluster validate` auto-routes through Bastion when login has no public IP. `azcluster list` filters out Azure-managed sister RGs (`MA_*`, `MC_*`, …).

> **Previous status (v0.23.0)**: minor — per-cluster Azure Storage account (StorageV2, `allowSharedKeyAccess: false`, single container `data`) provisioned by default. Private Endpoint on by default (compute subnet, Private DNS zone linked to cluster VNet); opt out via `--storage-public-access`. Optional ADLS Gen2 via `--storage-hns` (adds a `dfs` PE sub-resource). Storage account name is deterministic `stazc<8-hex-blake3>` (override via `--storage-name`). The cluster UAI gets `Storage Blob Data Contributor` on the account; `azcp` (https://github.com/edwardsp/azcp) installed on login + compute (default `v0.4.5`, configurable via `--azcp-version`) authenticates via IMDS. User-scoped path convention `/data/users/<user>/` blob + `/mnt/nvme/users/<user>/` local; env vars exposed in user shells. Slurm prolog lazily creates per-user NVMe dirs with correct ownership before each job step. Three new example sbatch templates demonstrate the upload → build-sqsh → multi-node broadcast workflow via `azcp-cluster`.

> **Previous status (v0.22.7)**: patch — `azcluster ssh|exec|scp <cluster> --user <ldap-user>` no longer fails with `Permission denied (publickey)`. The v0.22 admin SSH key (KV-backed, `~/.azcluster/keys/<cluster>`) only authenticates `azureuser`; an LDAP user's `authorized_keys` (via SSSD `sshPublicKey`) contains whatever pubkey was enrolled with `azcluster user {add,sshkey add} --ssh-key <file>` — typically the operator's `~/.ssh/id_*`. New `resolve_identity_for_user` returns `None` when `connect_user != admin_user` and no explicit `--identity` is passed, letting ssh fall back to its default key discovery (agent / `~/.ssh/id_*`). Live-reproduced on `paul-eus-hb120-h100` against LDAP user `paul`; v0.22.7 verified fix.

> **Previous status (v0.22.6)**: patch — `azcluster user {add,remove,list,sshkey ...}` no longer fails with `Permission denied (publickey)` on operators whose ssh-agent doesn't already hold the v0.22 KV-backed admin key. The two ssh wrappers in `crates/azcluster-cli/src/user.rs` (`ssh_run` and `flush_login_sssd_cache`) had been missed during the v0.22.1 sweep that fixed the OpenSSH `-J` non-propagation bug at the other 8 jump sites — both now resolve the admin identity via `~/.azcluster/keys/<cluster>` and use the same explicit `-o ProxyCommand="ssh -W %h:%p -i <key> -o IdentitiesOnly=yes ..." <login>` pattern. Live-reproduced on `paul-eus-hb120-h100` before the fix; same cluster works after.

> **Previous status (v0.22.5)**: patch — `azcluster deploy` live TTY progress no longer shows duplicate rows. The v0.22.4 recursive walker emitted each resource once per ARM provisioning-state transition (Accepted → Running → Succeeded), and when a nested module deployment had multiple state-transition ops it recursed into the module twice — duplicating the entire descendant subtree. Both classes are now fixed by deduping ops by `targetResource.id` at the walker level (keep latest-seen entry per target, before recursing).

> **Previous status (v0.22.4)**: patch — `azcluster deploy` live TTY progress now shows the full nested resource tree instead of only the 2 top-level sub-scope ops. Each module deployment (`network`, `scheduler`, `login`, `compute-<pool>`, `keyvault`, `monitoring`, `anf`) and its leaf resources (NSGs, public IPs, NICs, VMs, VMSS, vaults, etc.) are rendered with indentation matching their depth in the deployment tree. Ops poll cadence relaxed from 5s to 10s to absorb the higher ARM call volume per tick. Live-validated on `v224b` / `southafricanorth`: 22 resources captured (RG + root nested + 5 module nested + 15 leaves), deploy completed in 238s. Also fixes a latent bug: ARM deployment-operation envelopes do NOT populate `targetResource.resourceGroup` — the recursive walker now parses the RG out of the ARM `id` path.

> **Previous status (v0.22.3)**: patch — adds `azcluster purge-kv` for permanent recovery of soft-deleted azcluster Key Vaults (bypasses the 7-day retention) so operators can re-use cluster names within the same week without falling back to `az` CLI or the Azure Portal. Native ARM REST (`Microsoft.KeyVault/locations/{loc}/deletedVaults/{name}/purge`); handles both 200-sync and 202-async response shapes via the existing `wait_for_async_operation` LRO helper. Safety: `kv-azc-` prefix hardcoded as a non-configurable filter (impossible to touch non-azcluster vaults); refuses `--all` without explicit opt-in; interactive `'yes'` gate unless `--yes`. Also fixes a latent ARM-frontend bug surfaced by the first live invocation: bodyless POST requests now set `Content-Length: 0` explicitly to satisfy IIS-flavoured `HTTP 411 Length Required`. Live-validated end-to-end on the two orphan vaults from v22a/v22b in `southafricanorth` (~6 min per purge LRO).

> **Previous status (v0.22.2)**: patch — re-tag of v0.22.1 to recover from the 2026-05-26 GitHub Actions outage that swallowed the original tag's release trigger. Identical code content.

> **Previous status (v0.22.1)**: patch — fixes two regressions found during v0.22.0 live-validation. (1) `finalize_deploy()` was overwriting the freshly-generated admin SSH keypair with empty strings before uploading `secrets-bundle` to Key Vault, leaving `azcluster ssh/exec/scp/tunnel` broken on fresh deploys. (2) OpenSSH `-J <jump>` does NOT propagate `-i <identity>` to the jump hop — `ssh --scheduler`, `exec --scheduler`, `exec --host <compute>`, `scp` to non-login targets, and the `status` bootstrap probe all silently failed with `Permission denied (publickey)` because v0.22's admin key lives in `~/.azcluster/keys/<name>` (not in the operator's ssh-agent). Fixed by emitting explicit `-o ProxyCommand="ssh -W %h:%p -i <key> -o IdentitiesOnly=yes ..." <jump>` at all 8 jump sites. Live-validated end-to-end on `v22b`/`southafricanorth`: `status` probe READY on both hops, `exec --scheduler` succeeds, `scp` bidirectional round-trip OK.

> **Previous status (v0.22.0)**: minor — per-cluster Azure Key Vault becomes the source of truth for the cluster manifest + secrets (LDAP + MySQL admin passwords + freshly-generated admin SSH ed25519 keypair). Cluster RGs get five `azcluster:*` tags so the CLI can rediscover any cluster from the subscription alone. Every command (`ssh`/`exec`/`scp`/`tunnel`/`status`/`delete`/`scale`/`logs`/`monitor`/`timings`/`validate`/`resume`/`user`) is now stateless — any operator with KV RBAC runs them from a fresh laptop after only `azcluster login`. Admin private key materialises lazily to `~/.azcluster/keys/<cluster>` (`0600`). New subcommands `azcluster list` (RG-tag discovery) and `azcluster purge-cache`. Global `--no-cache` flag. Live TTY deploy progress; `azcluster timings` preserved. **Clean break — pre-v0.22 clusters are not discoverable, no migration path.**

> **Previous status (v0.21.4)**: minor — exposes `--scheduler-sku` and `--login-sku` on `azcluster deploy` so operators can override scheduler/login VM SKUs without editing Bicep. Defaults unchanged (`Standard_D8as_v5` / `Standard_D4as_v5`). Carries forward v0.21.3 LDAP-user UX (`--host` + `--user/-u` on `ssh`/`exec`/`scp`, `-A` on `exec`).

> **Previous status (v0.21.1)**: adds `azcluster scp` (bastion-aware scp wrapper with first-class node selection — `login` default, `scheduler`, or any compute hostname) and a fast-path on `azcluster login --subscription <id>` that rebinds the cached principal to a new subscription in ~6 ms without re-auth (workaround for Microsoft tenants where Conditional Access blocks device-code flow). Carries forward v0.21.0 Azure Bastion no-plugin support live-validated on `paul-azcluster` / `southafricanorth` (auto-route through Bastion for `ssh`/`exec`/`tunnel` + new `scp`; hidden `bastion-proxy` stdio bridge as ssh `ProxyCommand`; hand-rolled WS framing on `tokio-rustls` because Azure Bastion's non-RFC WS upgrade breaks `tokio-tungstenite`). Carries forward v0.20.0 native ARM REST + OAuth2.

> **Previous status (v0.21.0)**: native Azure Bastion (no plugin). Provisions Bastion Standard SKU + `enableTunneling: true` into `AzureBastionSubnet = 10.42.0.64/26` when `--bastion` is passed. CLI auto-routes `azcluster ssh/exec/tunnel` through Bastion when login has no public IP. Live-validated on `paul-azcluster`/`southafricanorth` (`v21a`): login + scheduler reachable via Bastion, `--no-bastion` cleanly surfaces the legacy error.

> **Previous status (v0.19.4)**: phases 0-3 + Slurm accounting + GPU pool + end-to-end DGXC Llama 3.1 8B BF16 + container `_mpi` NCCL live-validated. v0.19.4 fixed a latent showstopper for containerised MPI (removed `UCX_TLS=tcp` from `/etc/enroot/environ.d/50-nccl.env` and `/etc/profile.d/nccl-azcluster.sh`). Llama 3.1 8B BF16 baselines: 167,594 tok/s on 16 H100 (2 node) / 83,737 tok/s on 8 H100 (1 node). Full DGXC workflow: [walkthrough-dgxc.md](walkthrough-dgxc.md). Next backlog: bastion SSH tunneling (vgamayunov-style, no plugin), DCGM-backed NVLink/throttle checks, Slurm power-save autoscaling.

## Why azcluster

- **Single binary.** Pure-Rust CLI. Authenticates to Azure directly via OAuth2 (PKCE / device code); calls ARM REST natively. No `az` CLI, no Python venv, no agent, no laptop-side controller.
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
| Login VM | ✅ | no public IP | `--login-public-ip`, `--allowed-ssh-cidrs`, `--bastion` | Egress via NAT Gateway; `--bastion` enables Azure-native SSH tunneling without a public IP |
| Azure Bastion (no plugin) | ✅ | off | `--bastion` (deploy), `--no-bastion` (ssh/exec/tunnel opt-out) | Standard SKU + `enableTunneling`. `azcluster ssh/exec/tunnel` auto-route through Bastion when login has no public IP. Hidden `azcluster bastion-proxy` is used as ssh `ProxyCommand` (stdio WS bridge). Hand-rolled WS framing on `tokio-rustls` — Bastion's non-RFC WS upgrade breaks `tokio-tungstenite`. |
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

- Azure account with permissions to create resource groups, role assignments, and Monitor/Grafana resources in the target subscription
- SSH key (`~/.ssh/id_ed25519.pub` or `~/.ssh/id_rsa.pub`)
- `jq`

The CLI authenticates against Azure directly via OAuth2 (PKCE in a browser, or `--device-code` for headless). No `az` CLI install required.

```bash
azcluster login                              # interactive browser PKCE
azcluster login --device-code                # headless / SSH session
azcluster login --tenant <id> --subscription <id>
```

Tokens cache at `~/.azure/azcli_tokens.json` (mode 0600). Subscriptions enumerated via ARM REST; selected one persists alongside the token cache.

> Contributors editing `bicep/*.bicep` MUST regenerate `bicep/main.json` (`az bicep build --file bicep/main.bicep --outfile bicep/main.json`) before committing — CI fails the build on drift. The CLI embeds `main.json` at compile time; end users never need bicep.

## Install

Grab the prebuilt CLI from the latest release:

```bash
VERSION=v0.24.4
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

### Deploy without a public IP (Azure Bastion)

Drop `--login-public-ip` and add `--bastion` to provision Azure Bastion (Standard SKU + `enableTunneling`) alongside the cluster. `azcluster ssh/exec/tunnel` will auto-route through Bastion via a hidden `bastion-proxy` stdio bridge (used as ssh `ProxyCommand`). No browser, no Python plugin, no `az network bastion`.

```bash
azcluster deploy --name demo --location southafricanorth --resource-group my-rg \
  --pool name=cpu,sku=Standard_D8as_v5,count=1,default \
  --bastion              # adds ~3-5 min to deploy

azcluster ssh demo                  # auto-routes through Bastion
azcluster ssh demo --scheduler      # tunnels directly to scheduler VM (no -J)
azcluster exec demo -- hostname     # one-shot exec via Bastion
azcluster tunnel demo               # local 8443 -> scheduler:8443 via Bastion
# add --no-bastion to any of the above to force the legacy "no public IP" error.
```

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
| `azcluster scp <name> <SRC>... <DST>` | Bastion-aware scp wrapper. Remote paths use `[node]:path` (e.g. `:/shared/x`, `scheduler:/etc/x`, `vmss-<cluster>-<pool>NNNNNN:/x`); empty node = `login`. Flags: `-r`, `-p`, `-i <key>`, `--no-bastion`. |
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

### First user (LDAP) — the canonical multi-user path

The cluster admin account (`azureuser`) is the operator's foothold. Real users come from `azcluster user add`, which writes them to the on-cluster LDAP, SSSD on login + every compute node resolves them, and `pam_mkhomedir` creates `/shared/home/<user>` on first login.

```bash
# 1. Add an LDAP user with your local pubkey
azcluster user add demo --username alice --ssh-key ~/.ssh/id_rsa.pub

# 2. SSH straight in as that user (no agent forwarding needed)
azcluster ssh demo --user alice                  # → login VM, home /shared/home/alice
azcluster ssh demo --host demo-cpu-0001 --user alice    # → compute via ProxyJump

# 3. Exec one-shots and scp as the LDAP user (works for any compute hostname)
azcluster exec demo --host demo-cpu-0001 --user alice -- "id; hostname"
azcluster scp  demo --user alice ./local.txt demo-cpu-0001:/shared/home/alice/

# 4. Job submission as alice (writes to her own /shared/home/alice/)
azcluster ssh demo --user alice -- sbatch /shared/examples/nccl-allreduce.sbatch
```

Notes:
- `--host <hostname>` and `--scheduler` are mutually exclusive. `--host` is the generic compute-targeting flag and works for any in-VNet hostname the login VM can resolve.
- `--user <name>` (short `-u`) is honored at every SSH hop so the same identity authenticates ProxyJump and final destination.
- `--scheduler --user <ldap-user>` does NOT work — the scheduler hosts the LDAP server itself and runs no SSSD client. Use the admin user for scheduler shell access; job submission happens from login.
- `azcluster exec --forward-agent` / `-A` opts into SSH agent forwarding when you want nested ssh from the remote shell.

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
