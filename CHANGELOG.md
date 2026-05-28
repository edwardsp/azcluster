# Changelog

All notable changes to azcluster are documented here.
Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Versioning: [SemVer](https://semver.org/).

## [Unreleased]

## [0.24.4] - 2026-05-28

### Added
- **GPU + InfiniBand Grafana dashboard expanded** with 8 new panels for the v0.24.2 DCGM fields: GPU die temp vs `DCGM_FI_DEV_GPU_MAX_OP_TEMP` (in-band "tlimit", 87°C on H100), HBM3 memory temp, `DCGM_FI_PROF_SM_ACTIVE`, `DCGM_FI_PROF_PIPE_TENSOR_ACTIVE`, throttle violation rate (thermal + power), clock throttle reasons (decoded bitmask), NVLink errors (CRC_FLIT + CRC_DATA + REPLAY + RECOVERY), and ECC errors (SBE/DBE × volatile/aggregate). Total panels: 16 (was 8).

### Changed
- **Grafana dashboards land in `azcluster` folder, not at root** (N-20). The CLI's dashboard-import path now creates a folder titled "azcluster" with UID `azc-azcluster` (idempotent — `409 Conflict` and `412 Precondition Failed` are treated as success), then sets `folderUid: "azc-azcluster"` on every dashboard import payload. Drops the prior `folderId: 0` (which dumped them at root). Pre-v0.24.4 deploys can retire the orphan top-level dashboards manually via Grafana UI; new deploys land cleanly.
- **All dashboards now filter by `nodename`, not `instance`** (N-25). Grafana template variables and panel exprs across `gpu_ib.json` and `node.json` switched from `instance=~"$instance"` to `nodename=~"$nodename"`. Reason: every compute node scrapes its own colocated prometheus at `127.0.0.1:9100`, so `instance` is identically `127.0.0.1:9100` across the fleet — useless for disambiguation. `nodename` is set via Prometheus `external_labels.nodename = $(hostname)` in `cloud-init/{login,compute}.yaml.tmpl` and carries the real VMSS instance hostname into AMW. `node.json` IB panels were live-broken (showing "No data") because of this; now fixed.
- **Dashboard import retry is now unbounded** (N-19). Previous 60-min cap caused `azcluster deploy` on fresh AMG instances to give up before the Grafana Admin role assignment had propagated to the Grafana data plane (Azure-side eventual consistency, observed taking >60 min in some regions). Rationale: the cluster is fully usable while waiting, dashboards are cosmetic, and there is no scenario where giving up early is the correct behavior. Loop continues with 30 s polls + non-TTY 5-min status emission. Only bails on non-retryable HTTP errors (non-401/403 + non-`NoRoleAssignedException`) or operator interrupt.
- `--azcluster-version` CLI default bumped from `v0.24.3` to `v0.24.4`.

## [0.24.3] - 2026-05-27

### Fixed
- **N-17c `azcluster ssh|exec|scp --host <compute>` lands on login under `--bastion`** (live-reproduced + fixed on v24walk2). OpenSSH `ProxyCommand` cannot be chained on a single invocation — when both `-o ProxyCommand=bastion-proxy ...` and a `-o ProxyCommand=ssh -W ...` jump were set on the SAME ssh command, the LAST one wins, silently dropping the bastion routing and forcing the connection through what would-be the local 127.0.0.1 (which isn't reachable). Result: every `--host <compute>` invocation under `--bastion` ended up on the login VM because that's where the final destination resolved to. Fixed by introducing a new helper `bastion_compute_proxy_command()` that builds ONE composite ProxyCommand of the form `ssh -W %h:%p -o ProxyCommand='<bastion-proxy> --target login' azureuser@127.0.0.1`, with the inner ssh using the admin key + `IdentitiesOnly=yes` for the login hop. Applied to `ssh`, `exec`, and `scp`. Live-validated end-to-end: `exec --host v24walk2-gpu-0002 -- "hostname; nproc; nvidia-smi -L"` now returns the compute node (96 CPUs, 8× H100). scp roundtrip also works.

### Changed
- `--azcluster-version` CLI default bumped from `v0.24.2` to `v0.24.3`.

## [0.24.2] - 2026-05-27

### Added
- **DCGM thermal-limit + ECC + HBM3 row-remap + NVLink-error metrics** (live-validated v24walk2). Counters CSV now ships 15 additional fields all confirmed accepted by dcgm-exporter 4.4.0 talking to DCGM 4.5.2 on `microsoft-dsvm:ubuntu-hpc:2404`:
  - Thermal: `DCGM_FI_DEV_GPU_MAX_OP_TEMP` (= 87°C constant on H100 SXM5, matches the "tlimit" name used by Azure host-side telemetry), `DCGM_FI_DEV_GPU_TEMP_LIMIT` (dynamic margin), `DCGM_FI_DEV_SLOWDOWN_TEMP`, `DCGM_FI_DEV_SHUTDOWN_TEMP` — last three are best-effort and may emit empty on some H100 firmwares.
  - ECC: `DCGM_FI_DEV_ECC_{SBE,DBE}_{VOL,AGG}_TOTAL` quartet (volatile + aggregate single/double-bit).
  - HBM3 row remap: `DCGM_FI_DEV_RETIRED_{SBE,DBE,PENDING}` + `DCGM_FI_DEV_{CORRECTABLE,UNCORRECTABLE}_REMAPPED_ROWS` + `DCGM_FI_DEV_ROW_REMAP_FAILURE`.
  - NVLink errors: `DCGM_FI_DEV_NVLINK_{CRC_FLIT,CRC_DATA,REPLAY,RECOVERY}_ERROR_COUNT_TOTAL` quartet (aggregate; per-link variants deferred to a future release due to cardinality).
  - `DCGM_FI_DEV_FB_RESERVED` (per-process VRAM accounting).

### Changed
- `--azcluster-version` CLI default bumped from `v0.24.1` to `v0.24.2`.

### Removed
- DCGM counters `DCGM_FI_DEV_ENC_UTIL` + `DCGM_FI_DEV_DEC_UTIL` (NVENC/NVDEC video codec utilisation — always ~0 on H100 SXM5 training clusters; pure noise).

### Fixed
- **N-17a `/shared` is now `chmod 1777`** in scheduler bootstrap, not `0755`. The v0.18.1 fix that set `/shared` to 0755 (to let LDAP users TRAVERSE the dir to reach their home) accidentally prevented them from creating sibling dirs like `/shared/dgxc`, breaking every DGXC example sbatch with `mkdir: cannot create directory '/shared/dgxc': Permission denied`. Live-reproduced today on `v24walk2`. The new layout: `/shared` 1777 (sticky world-write, like `/tmp`), `/shared/home` 0755 (root-managed), pre-created `/shared/dgxc` + `/shared/jobs` 1777 for examples. Individual home dirs under `/shared/home/<user>` keep their 0700 perms from `pam_mkhomedir` (v0.18.2 umask fix).

## [0.24.1] - 2026-05-27

### Fixed
- **N-12** scheduler bootstrap now provisions sacctmgr account + per-partition associations for default LDAP users (`clusteradmin`, `clusteruser`) so `sbatch` works out-of-box without the operator manually running `azcluster user add`.
- **N-13** `ArmClient` now transparently refreshes the OAuth2 access token on HTTP 401 `ExpiredAuthenticationToken`/`InvalidAuthenticationToken` and retries the GET once. Long-poll loops (deployment completion, async LRO) no longer fail after the ~60 min token lifetime; previously `azcluster deploy` aborted mid-poll with a 401 even though the deployment itself was still progressing. Implementation: `ArmClient.access_token: RwLock<String>` + `with_refresh_callback(get_access_token)` injected in `arm_client()` factory.
- **N-14** Grafana dashboard import status line: detects non-TTY stderr (e.g. when CLI output is piped to `tee` or redirected to a log file) and emits one full status line every 5 minutes instead of carriage-return-refreshing every 30s. Interactive TTY behaviour unchanged.
- **N-16** dcgm-exporter on compute nodes now ships a custom `/etc/dcgm-exporter/counters.csv` (bind-mounted into the enroot container) that adds DCGM profile-mode metrics: `DCGM_FI_PROF_GR_ENGINE_ACTIVE`, `SM_ACTIVE`, `SM_OCCUPANCY`, `PIPE_TENSOR_ACTIVE`, `DRAM_ACTIVE`, `PIPE_FP64/FP32/FP16_ACTIVE`, `PCIE_TX/RX_BYTES`, `NVLINK_TX/RX_BYTES`. Enables tensor-core utilization charts in Grafana without rebuilding the container image.

### Changed
- `--azcluster-version` CLI default bumped from `v0.24.0` to `v0.24.1`.

## [0.24.0] - 2026-05-27

### Added
- **Default LDAP users**: every fresh deploy now provisions two LDAP users in addition to the local `azureuser` admin foothold:
  - `clusteradmin` (uid 20001, primary group `cluster-admins` gid 20100, sudo via `/etc/sudoers.d/cluster-admins`)
  - `clusteruser` (uid 20002, primary group `azusers` gid 20000)

  Both have `sshPublicKey` seeded to the v0.22 admin pubkey, so the deployer's `~/.azcluster/keys/<cluster>` private key authenticates them out of the box: `azcluster ssh <cluster> --user clusteradmin` works immediately. `pam_mkhomedir` creates `/shared/home/clusteradmin` and `/shared/home/clusteruser` on first login.

- **LDAP admin role + sudo**: new `cn=cluster-admins,ou=groups` LDAP group. Members get sudo via `/etc/sudoers.d/cluster-admins` (`%cluster-admins ALL=(ALL) NOPASSWD: ALL`) on login + compute. `clusteradmin` is in this group by default; promote/demote other users via `azcluster user setadmin` / `unsetadmin`.

- **`azcluster user add --admin`** flag: provisions the user AND adds them to `cn=cluster-admins` in one call. Equivalent to `add` followed by `setadmin`.

- **Per-user auto-generated keypairs**: `azcluster user add` now generates a fresh ed25519 keypair on the operator's laptop. Public key goes into the user's LDAP `sshPublicKey` (alongside any `--ssh-key` files), private key is written to `~/.azcluster/keys/<cluster>-<username>` (mode 0600). The private key is **never** uploaded to Key Vault — only on the operator's laptop. `azcluster ssh --user <name>` from that laptop now Just Works without `--identity`. Use `--no-generate-keypair` to skip if you supplied all keys via `--ssh-key`.

- **`azcluster user list`** now shows admin status (per `cn=cluster-admins` membership).

- **`azcluster user setadmin` / `unsetadmin`**: promote or demote an existing LDAP user.

### Changed
- `--azcluster-version` CLI default bumped from `v0.23.2` to `v0.24.0`.
- `sshPublicKey` Bicep param on scheduler module is no longer `@secure()` (it's a public key — needed for ldap-base.ldif `__ADMIN_SSH_PUBLIC_KEY__` substitution). No security impact; pubkey is on every VM `osProfile`.
- `bicep/main.json` regenerated.

## [0.23.2] - 2026-05-27

### Fixed
- **`azcluster user {add,remove,sshkey ...}` now Bastion-aware.** Previously these commands shelled out via login's public IP and failed with `cluster X has no login public IP` on `--bastion` deploys. Now they auto-route through `bastion-proxy --target scheduler` (direct, skipping login since the LDAP server is on scheduler). Same pattern as `ssh`/`exec`/`scp`/`tunnel`/`validate` already use.
- **`azcluster tunnel <cluster> <local>:8443` now goes direct via Bastion** (`--target scheduler`) instead of through login. Saves a hop.
- **Grafana Admin RBAC propagation: never fails deploy.** Replaced the 20-attempt × 60s = 20 min hard-fail loop with a 60 min soft-cap that prints a single rolling status line (`waiting for Grafana Admin role propagation: 12m34s elapsed (cap 60m), last response: 401 Unauthorized`). On timeout, logs a clean "cluster IS fully usable; re-run `azcluster monitor` later" message and returns success. Deploy no longer errors-out on slow Azure RBAC propagation.
- **DCGM exporter actually works on compute nodes.** Pre-v0.23.2 used `docker run` which silently no-op'd because the `microsoft-dsvm:ubuntu-hpc` image ships pyxis+enroot but NOT docker. v0.23.2 imports the `nvcr.io/nvidia/k8s/dcgm-exporter` container via `enroot import` and runs it as a systemd unit talking to nv-hostengine over TCP `localhost:5555`. GPU metrics (`DCGM_FI_DEV_GPU_UTIL`, `DCGM_FI_PROF_GR_ENGINE_ACTIVE`, etc.) now flow to AMW.
- **Example sbatch templates no longer hardcode `--partition=cpu`.** A default azcluster deploy has only a `gpu` pool; the cpu specifier made every example sbatch fail with `Invalid partition specified: cpu` until manually patched. Removed the partition spec so Slurm picks the default partition.
- **`azcluster user add` now creates partition associations for every existing partition.** Slurm with `AccountingStorageEnforce=associations,limits,qos` (our default) requires explicit per-partition association in sacctmgr, otherwise sbatch returns `Invalid account or account/partition combination`. v0.23.2's user-add loop iterates `sinfo -h -o '%R'` and runs `sacctmgr add user <u> Account=<u> Partition=<p>` for each.

### Changed
- `--azcluster-version` CLI default bumped from `v0.23.1` to `v0.23.2`.
- `bicep/main.json` regenerated.

## [0.23.1] - 2026-05-27

### Fixed
- **azcp tarball extraction**: `cloud-init/{login,compute}.yaml.tmpl` now uses `tar -xzf … --strip-components=1 -C /usr/local/bin/ azcp-x86_64-unknown-linux-gnu/azcp` because the v0.4.5 azcp release archive nests the binary one directory deep. Previously aborted install-{login,compute}.sh under `set -e` because tar exited non-zero — silently skipped every step after azcp install (including `touch /var/log/azcluster/ready`).
- **Storage URL composition missing slash**: `/etc/profile.d/azcluster-storage.sh` had `AZCLUSTER_USER_BLOB_URL="${AZCLUSTER_STORAGE_URL}users/${USER}"` (no `/`). Fixed to `${AZCLUSTER_STORAGE_URL}/users/${USER}` so `azcp copy` paths resolve correctly.
- **Enroot temp path on tmpfs**: `ENROOT_TEMP_PATH /run/enroot` (tmpfs, RAM-backed) caused large container imports (NeMo ~16 GiB, plus mksquashfs scratch on `/tmp` which is on the 61 GB root disk) to fail with "No space left on device". Fixed to `ENROOT_TEMP_PATH ${ENROOT_BASE}/enroot-temp` (so `/mnt/nvme/enroot-temp` when NVMe RAID is present, ~28 TB capacity).
- **apt-daily race on first boot**: cloud-init's `package_update: true` raced with `unattended-upgrades` on first boot and hit `Could not get lock /var/lib/dpkg/lock-frontend`, killing the bootstrap before our runcmd could disable unattended-upgrades. Now disabled in `bootcmd:` (which runs before `package_update`) on all 3 templates.
- **Prometheus compute config malformed YAML**: the `SCRAPE_GPU` shell variable that appended a `dcgm_exporter` block inline into `static_configs:` produced wrong indentation (`- job_name: dcgm_exporter` at column 8 under `- targets:` at column 6, instead of at scrape_configs level). Compute Prometheus restarted 1000+ times with `parsing YAML file: yaml: did not find expected key`. Replaced with a separate `cat >>` block after the main config; indent is now correct.
- **Example sbatch templates lost storage env**: `#!/bin/bash` + `set -u` non-login shell didn't source `/etc/profile.d/azcluster-storage.sh`, so `$AZCLUSTER_USER_BLOB_URL` and `$AZCLUSTER_USER_NVME` were unbound. Templates now use `#!/bin/bash -l` (login shell).
- **azcp-cluster distribute prefix vs single-file**: azcp-cluster treats the source URL as a prefix; a single-file source ending in `.sqsh` matched as a directory marker and transferred 0 bytes. Example template now uploads to `users/<u>/sqsh/<name>/<name>.sqsh` (per-sqsh subdir) and `azcp-cluster` source points to `users/<u>/sqsh/<name>/`. `srun --export=` now propagates `AZCLUSTER_USER_BLOB_URL` and `AZCLUSTER_USER_NVME` into the pyxis container so the inner azcp-cluster has them.
- **Grafana Admin RBAC propagation retry budget too short**: 10 × 30 s = 5 min was tight. Bumped to 20 × 60 s = 20 min.
- **`azcluster validate` did not auto-route via Bastion**: when login has no public IP, validate now uses the same `bastion-proxy` ProxyCommand as `ssh`/`exec`/`scp`.
- **`azcluster list` included Azure-managed sister RGs** (the `MA_<amwName>_<location>_managed` group that Azure Monitor auto-creates). These get tagged with our `azcluster:*` tags because ARM propagates tags from the parent deployment. `list` now filters out RGs whose name matches Azure-managed prefixes (`MA_`, `MC_`, `AzureBackupRG_`, `NetworkWatcherRG`, `databricks-rg-`).

### Changed
- `--azcluster-version` CLI default bumped from `v0.23.0` to `v0.23.1`.
- `bicep/main.json` regenerated to reflect the Bicep template-hash bump from the storage module finalisation.

## [0.23.0] - 2026-05-26

### Added
- Per-cluster Azure Storage account with a single container `data`, provisioned by default. Disable via `--no-storage`. `StorageV2` SKU configurable via `--storage-sku` (default `Standard_LRS`); access tier configurable via `--storage-tier` (default `Hot`). `allowSharedKeyAccess: false` is hardcoded — all data-plane auth is AAD via the cluster UAI (which gets Storage Blob Data Contributor on the account). Soft-delete: 7-day blob + container retention by default.
- Storage account name is deterministic: `stazc<8-hex-blake3(subscription_id|cluster_name|location)>` (13 chars, lowercase alphanumeric, well under the Azure 24-char limit). Override via `--storage-name`; validates against the Azure naming grammar (3-24 chars, lowercase ASCII letters + digits).
- Storage Private Endpoint (PE) on by default — PE NIC lives in the cluster's compute subnet (`10.42.4.0/22`), Private DNS zone `privatelink.blob.core.windows.net` linked to the cluster VNet. When `--storage-hns` is set, a second PE + DNS zone is provisioned for the `dfs` sub-resource (ADLS Gen2 hierarchical operations). Disable via `--storage-public-access` to skip PE/DNS provisioning entirely (exposes the account to operator laptop).
- `azcp` (https://github.com/edwardsp/azcp) installed on login + compute via cloud-init, version pinned by `--azcp-version` (default `v0.4.5`). Binary at `/usr/local/bin/azcp`; authenticates via IMDS using the cluster UAI (`AZURE_CLIENT_ID` set in `/etc/profile.d/azcluster-storage.sh`).
- User-scoped storage path convention: `/data/users/<user>/` under the cluster blob container, mirrored by `/mnt/nvme/users/<user>/` on each compute node. Login + compute shells get env vars (`AZCLUSTER_STORAGE_URL`, `AZCLUSTER_USER_BLOB_URL`, `AZCLUSTER_USER_NVME`, `AZCLUSTER_SHARED_BLOB_URL`) via `/etc/profile.d/azcluster-storage.sh`. A common `/data/shared/` area is available by convention (Contributor RBAC is cluster-wide, not per-path).
- Slurm prolog `/etc/slurm/prolog.d/10-azcluster-user-nvme.sh` (mode 0755, written by compute cloud-init) lazily creates `/mnt/nvme/users/${SLURM_JOB_USER}` with `${SLURM_JOB_UID}:${SLURM_JOB_GID} 0700` before any job step starts. Wired into scheduler `slurm.conf` via `Prolog=...` + `PrologFlags=Alloc,Contain`. Skips silently when `/mnt/nvme` doesn't exist (non-NVMe-RAID SKUs).
- Example sbatch templates dropped in `/shared/examples/` when storage is enabled: `azcp-upload-user-data.sbatch` (single-node upload from `/shared/home/${USER}/` to user blob path), `azcp-build-and-publish-sqsh.sbatch` (build sqsh on compute, publish to user blob), `azcp-cluster-distribute-sqsh.sbatch` (multi-node `azcp-cluster` broadcast of a user sqsh to per-node NVMe via pyxis `--container-image=docker://ghcr.io/edwardsp/azcp/azcp-cluster:<azcp-version>`).
- 7 new clap flags on `azcluster deploy`: `--storage` / `--no-storage`, `--storage-name`, `--storage-hns`, `--storage-public-access`, `--storage-sku`, `--storage-tier`, `--azcp-version`. All persist into `ClusterState` (Key Vault manifest) + `PendingDeploy` (resume marker). All have `#[serde(default)]` on the state fields so pre-v0.23 clusters deserialize cleanly.
- The cluster UAI (`uai-<cluster>-scheduler`) is now attached to compute + login VMs/VMSS in addition to scheduler, so `azcp` on any node authenticates as the principal that holds Storage Blob Data Contributor. Pre-v0.23, compute + login only had the monitoring UAI (when monitoring was on) or no UAI at all.

### Changed
- `--azcluster-version` CLI default bumped from `v0.22.7` to `v0.23.0`.
- `bicep/main.json` regenerated to reflect storage module, 4 new substitution placeholders threaded through compute/login/scheduler modules, and the dual-UAI attachment on compute + login.

## [0.22.7] - 2026-05-26

### Fixed
- `azcluster ssh|exec|scp <cluster> --user <ldap-user>` no longer fails with `Permission denied (publickey)`. The v0.22 admin SSH key (KV-backed, `~/.azcluster/keys/<cluster>`) only authenticates `azureuser`; an LDAP user's `authorized_keys` (via SSSD `sshPublicKey`) contains whatever pubkey was enrolled with `azcluster user {add,sshkey add} --ssh-key <file>` — typically the operator's `~/.ssh/id_*`. The CLI was force-passing `-i <admin-key>` regardless of `--user`, so every LDAP-user invocation was rejected. New `resolve_identity_for_user` returns `None` when `connect_user != admin_user` and no explicit `--identity` is passed, letting ssh fall back to its default key discovery (agent / `~/.ssh/id_*`). New `add_ssh_jump` wrapper emits bare `-J <jump>` in that case (correct because the inner ssh uses the same default discovery). Applied to `ssh`, `exec`, `scp`. Admin-only paths (`tunnel`, `logs`, `validate`, `status` bootstrap probe, `user.rs` ssh wrappers) keep using `resolve_identity` + `add_ssh_jump_with_identity` unchanged. Live-reproduced on `paul-eus-hb120-h100` against LDAP user `paul`.

## [0.22.6] - 2026-05-26

### Fixed
- `azcluster user {add,remove,list,sshkey {add,remove,list}}` no longer fails with `Permission denied (publickey)` on operators whose ssh-agent does not already hold the v0.22 KV-backed admin key. The two ssh wrappers in `crates/azcluster-cli/src/user.rs` (`ssh_run` and `flush_login_sssd_cache`) had been missed by the v0.22.1 sweep that fixed the OpenSSH `-J` non-propagation bug at the other 8 jump sites. They now resolve the admin identity via `~/.azcluster/keys/<cluster>` (materialised lazily from Key Vault) and use the same explicit `-o ProxyCommand="ssh -W %h:%p -i <key> -o IdentitiesOnly=yes ..." <login>` pattern. `resolve_identity` and `add_ssh_jump_with_identity` are now `pub(crate)` so `user.rs` can call them. Live-reproduced on `paul-eus-hb120-h100`; v0.22.6 build verified fix.

## [0.22.5] - 2026-05-26

### Fixed
- `azcluster deploy` live TTY progress no longer renders duplicate rows. Two distinct dup sources fixed at the walker level (`arm/client.rs`): (1) ARM returns one operation per provisioning-state transition (Accepted → Running → Succeeded) per target — the walker now dedupes by `targetResource.id` keeping the latest-seen entry, so each resource appears exactly once with its current state; (2) when a nested module deployment had multiple state-transition ops, the walker recursed into it twice, duplicating its entire descendant subtree — dedup before recursion now ensures each nested module is walked exactly once. Two regression tests added (`dedup_ops_by_target_keeps_latest_state_per_id_preserves_first_seen_order`, `dedup_ops_by_target_drops_ops_without_target_id`).

## [0.22.4] - 2026-05-26

### Changed
- `azcluster deploy` live TTY progress now recursively walks the deployment tree and indents each module's child resources beneath its parent (`cluster-<name>` → `network` / `scheduler` / `login` / `compute-<pool>` / `keyvault` / `monitoring` / `anf` → individual leaf resources like `vnet-*`, `vm-*-scheduler`, `vmss-*-cpu`, `kv-azc-*`, etc.). Previously only the 2 sub-scope ops were visible (the root nested-deployment row and the RG row), giving the misleading `1/2 ops` summary even mid-deploy. Ops poll cadence relaxed from 5s to 10s to absorb the ~10-15× higher ARM call volume per tick (still well under read-throttle limits). Live-validated end-to-end on `v224b` / `southafricanorth`: 22 resources captured (RG + root nested + 5 module nested + 15 leaves) all rendered with correct indentation, deploy completed in 238s.

### Fixed
- ARM deployment-operation envelopes return `targetResource` with only `id` / `resourceType` / `resourceName`; the `resourceGroup` field is NOT populated. The new recursive walker fell back to sub-scope endpoint queries for RG-scoped nested deployments (which returned no operations), silently truncating the tree at depth 1. Fixed by parsing the resource group out of the ARM `id` path when the structured `resourceGroup` field is absent (same pattern as `timings::parse_target_id`). Three regression tests added pinning the ARM-shape behavior (`nested_module_target_falls_back_to_id_when_resource_group_field_absent`, `_sub_scope_has_empty_rg`, `_non_deployment_returns_none`).

## [0.22.3] - 2026-05-26

### Added
- `azcluster purge-kv` permanently removes soft-deleted azcluster Key Vaults (bypasses the 7-day soft-delete retention). Flags: `--name <cluster> --location <loc>` to target a specific vault (derives `kv-azc-<hash(sub|name|location)>`), `--all` for every `kv-azc-*` in the subscription, `--location <loc>` to scope, `--dry-run` to list candidates without acting, `--yes` to skip the interactive `'yes'` prompt. Calls the ARM `Microsoft.KeyVault/locations/{loc}/deletedVaults/{name}/purge` endpoint natively (no `az` shell-out); handles both 200-sync and 202-async response shapes via the existing `wait_for_async_operation` LRO helper. Live-validated against both v22a + v22b orphan vaults in `southafricanorth` (purge LRO observed ~6 min per vault).

### Fixed
- ARM POST requests with no body now set an explicit `Content-Length: 0` header. Without it, Azure's ARM frontend returns `HTTP 411 Length Required` (HTML response). Surfaced by the first `azcluster purge-kv` live invocation. Affects `purge_deleted_vault` and any future bodyless POST on the ARM client.

### Added
- `azcluster purge-kv` permanently removes soft-deleted azcluster Key Vaults (bypasses the 7-day soft-delete retention). Flags: `--name <cluster> --location <loc>` to target a specific vault (derives `kv-azc-<hash(sub|name|location)>`), `--all` for every `kv-azc-*` in the subscription, `--location <loc>` to scope, `--dry-run` to list candidates without acting, `--yes` to skip the interactive `'yes'` prompt. Calls the ARM `Microsoft.KeyVault/locations/{loc}/deletedVaults/{name}/purge` endpoint natively (no `az` shell-out); handles both 200-sync and 202-async response shapes via the existing `wait_for_async_operation` LRO helper.

### Fixed
- ARM POST requests with no body now set an explicit `Content-Length: 0` header. Without it, Azure's ARM frontend returns `HTTP 411 Length Required`. Surfaced by the first `azcluster purge-kv` live invocation. Affects `purge_deleted_vault` and any future bodyless POST on the ARM client.

## [0.22.2] - 2026-05-26

Identical content to v0.22.1; v0.22.1 tag did not trigger GitHub Actions (delete+re-push race with the Actions trigger debouncer left no release published). Re-tagged as v0.22.2 to force a clean trigger.

### Fixed
(same as v0.22.1 below)

## [0.22.1] - 2026-05-26

### Fixed
- **`finalize_deploy()` was overwriting the freshly-generated admin SSH keypair on disk with empty strings, then uploading the empty keypair to Key Vault.** Root cause: `finalize_deploy()` rebuilt the `ClusterSecrets` struct from `existing_secrets` (captured at the start of `deploy()` — `None` on a fresh deploy), so `admin_ssh_{public,private}_key` defaulted to empty strings. The subsequent `secrets.save()` clobbered the keypair that `deploy()` had written at line 1003, and the KV upload then pushed empty keys. Effect: a fresh `azcluster deploy` succeeded, the cluster reached ARM `Succeeded`, but `azcluster ssh/exec/scp/tunnel` then errored with `secrets-bundle in vault '<kv>' has no admin_ssh_private_key`. Re-running deploy then generated a *fresh* keypair (because local secrets file was now empty), but ARM rejected the update with `PropertyChangeNotAllowed` since `osProfile.linuxConfiguration.ssh.publicKeys` is immutable on existing VMs. Fix: `finalize_deploy()` now re-loads `ClusterSecrets::load_optional(&args.name)` from disk first to pick up the keypair that `deploy()` just persisted, and only falls back to `existing_secrets` if the on-disk copy is missing. Live-reproduced on `v22a`/`southafricanorth` (v0.22.0) before the fix; live-validated end-to-end on `v22b`/`southafricanorth` after the fix.
- **OpenSSH `-J <jump>` does NOT propagate `-i <identity>` to the jump hop.** Every `azcluster ssh/exec/scp` invocation that traverses the login VM (i.e. `--scheduler` or `--host <compute>`, or any scp to a non-login target) was failing with `Permission denied (publickey)` on the jump hop because the inner ssh that handles `-J` falls back to the agent / `~/.ssh/id_*` instead of inheriting the outer `-i`. In v0.21.x this happened to work because the admin key was a copy of the operator's `~/.ssh/id_ed25519.pub` and therefore already in the agent; v0.22 generates the admin keypair fresh per deploy and stores it only in Key Vault + `~/.azcluster/keys/<cluster>`, breaking the implicit agent assumption. Fix: introduced `add_ssh_jump_with_identity()` helper that emits an explicit `-o ProxyCommand="ssh -W %h:%p -i <key> -o IdentitiesOnly=yes ..." <jump>` instead of `-J <jump>`. Applied to all 8 jump sites in `crates/azcluster-cli/src/main.rs` (3 in `ssh()`, 3 in `exec()`, 1 in `scp()`, 1 in `bootstrap_probe()`). Live-validated: `status` probe now reports `login: READY` + `scheduler: READY`; `exec --scheduler` reaches scheduler and reports `slurmctld active`; `scp` round-trip (laptop ↔ login) succeeds.
- **`azcluster status` bootstrap probe was not using the admin identity** — it invoked `ssh` without `-i`, relying on the operator's ssh-agent. Now resolves the admin identity via `resolve_identity(None, &state.name)` (lazily fetching from KV if needed) and injects `-i <key> -o IdentitiesOnly=yes` into the probe ssh. Silently prints `SKIP (no admin key: <reason>)` if the identity can't be resolved, so `status` remains best-effort.

## [0.22.0] - 2026-05-26

### Added
- **Per-cluster Azure Key Vault as source of truth for cluster manifests + secrets.** Every `azcluster deploy` now provisions a per-cluster Key Vault (`kv-azc-<8-hex-blake3(sub|name|location)>`, ≤24 chars) in the cluster RG with RBAC enabled, and writes two secrets at finalize-time: `cluster-manifest` (full `ClusterState` as JSON) and `secrets-bundle` (LDAP admin password + MySQL admin password if accounting + admin SSH ed25519 keypair). The cluster RG gets five `azcluster:*` tags (`managed=true`, `name=<cluster>`, `kv=<vault>`, `version=<crate-version>`, `deployed-at=<ISO-8601>`) so the CLI can rediscover any cluster from the subscription alone.
- **Stateless CLI.** `azcluster {ssh,exec,scp,tunnel,status,delete,scale,logs,monitor,timings,validate,resume,user}` all resolve cluster state via `cluster_resolver::Resolver` — local cache hit (`~/.config/azcluster/clusters/<name>.toml`, 24h TTL) → RG tag lookup → Key Vault fetch → cache write. Any operator with KV RBAC can run every command from a fresh laptop after `azcluster login`. Pre-v0.22 clusters (no `azcluster:managed` tag) are not discoverable; clean break, no migration path.
- **Admin SSH keypair generated fresh per deploy and stored in Key Vault.** `azcluster deploy` now mints a brand-new ed25519 keypair via `ssh-key` 0.6 (no read of `~/.ssh/*`) and stamps it into the `secrets-bundle` secret alongside other secrets. The private key materialises lazily to `~/.azcluster/keys/<cluster>` (file `0600`, parent dir `0700`) on first `azcluster ssh/exec/scp/tunnel` invocation, fetched directly from Key Vault. The `--ssh-key` flag has been removed from `azcluster deploy`.
- **`azcluster list`** subcommand: enumerates all `azcluster:managed=true` resource groups in the current subscription via ARM REST `GET /subscriptions/{}/resourcegroups?$filter=tagName eq 'azcluster:managed' and tagValue eq 'true'`. Plain-text table by default, JSON via `--json`. Header includes the resolved subscription id for clarity.
- **`azcluster purge-cache [--name <cluster>]`** subcommand: clears entries under `~/.config/azcluster/clusters/`. With `--name`, only that cluster's `<name>.toml` is removed. `-pending` and `-secrets` markers are preserved.
- **Global `--no-cache` flag.** `azcluster --no-cache <subcommand>` bypasses the local cache and forces a Key Vault round-trip on every cluster resolution. Useful when the cache is suspected stale or when validating that KV is the authoritative source.
- **Live TTY deploy progress.** `azcluster deploy` (without `--no-wait`) now renders an in-place ANSI table of ARM resource provisioning states (Creating/Running/Succeeded/Failed) that updates every poll cycle. Non-TTY environments (CI, redirected stdout) automatically fall back to plain-line streaming so logs remain greppable. `azcluster timings <name>` is unaffected and continues to work.

### Changed
- All 16 `ClusterState::load(...)` call sites in `azcluster-cli/src/main.rs` swapped to `resolve_cluster(...)`.
- All 6 `if let Some(id) = &args.identity { -i ... }` blocks (ssh, tunnel, exec, scp, logs, validate) replaced with unconditional `resolve_identity(args.identity.as_deref(), &args.name)?` + `-i <path>` injection. User-supplied `--identity` still wins; default is the lazily-materialised admin key.
- `ClusterSecrets` schema extended with `admin_ssh_public_key: Option<String>` + `admin_ssh_private_key: Option<String>` (both `#[serde(default)]`, OpenSSH PEM-encoded). Round-trip serialisation test updated.
- `finalize_deploy()` now (a) re-persists `ClusterSecrets` with the admin keypair, (b) uploads `cluster-manifest` + `secrets-bundle` to the per-cluster Key Vault via `KeyVaultClient::set_secret`, (c) calls `ArmClient::patch_resource_group_tags()` to stamp the five `azcluster:*` tags onto the RG. All three side-effects warn-not-fail so a transient KV/ARM blip doesn't strand a freshly-deployed cluster.
- `ArmClient` gains `patch_resource_group_tags(rg, tags)` (RG `PATCH` is 200-sync per ARM contract) and a new `get_vault_token()` helper that mints a `https://vault.azure.net/.default` access token from the cached auth principal.
- ARM deployment parameters now include `keyVaultName` (computed CLI-side via `derive_kv_name()`) and `deployerPrincipalId` + `deployerPrincipalType` (previously optional; now unconditional so the Bicep `keyvault.bicep` module can grant the deployer Key Vault Secrets Officer at create time).
- Bicep `cluster.bicep` now provisions `bicep/modules/keyvault.bicep` and outputs `keyVaultName`, `keyVaultUri`, `keyVaultId`.

### Removed
- `azcluster deploy --ssh-key <path>` (kept v0.21.x invariant of "use the operator's local pubkey" broken; admin key is now always generated fresh server-side and lives in Key Vault).

### Notes
- **Clean break — no migration.** Pre-v0.22 clusters lack the `azcluster:managed` RG tag and the Key Vault. They can still be torn down via `azcluster delete <name>` (which falls back to the local state file when KV resolution returns 0 RGs), but every other subcommand requires a v0.22+ deploy.
- **Auth audience.** Vault tokens are cached separately from ARM management tokens (different audience: `https://vault.azure.net/.default` vs `https://management.azure.com/.default`). Both are minted from the same cached auth principal via the existing OAuth2 PKCE/device-code flow.
- **Dependencies added.** `blake3 = "1"` (KV name derivation), `ssh-key = { version = "0.6", features = ["ed25519", "alloc"] }` (admin keypair generation + OpenSSH PEM encoding).
- **Test discipline.** All 114 workspace tests pass under `cargo test --workspace -- --test-threads=1`. The `arm::config::tests::test_api_version_*_env` parallel-env-var race documented in earlier releases still applies; AGENTS.md mandates `--test-threads=1`.
- **Live-validation.** Gates green at commit time (`cargo build --workspace`, `cargo test --workspace -- --test-threads=1` → 114 pass, `cargo clippy --workspace --all-targets -- -D warnings`). End-to-end live-validation against `southafricanorth` deploy + fresh-laptop scenario follows tagging.

## [0.21.4] - 2026-05-25

### Added
- **`azcluster deploy --scheduler-sku <sku>` and `--login-sku <sku>`** — operator-facing CLI flags to override the VM SKU of the scheduler and login VMs. Defaults remain `Standard_D8as_v5` (scheduler) and `Standard_D4as_v5` (login), so existing invocations are unaffected. The underlying `schedulerSku` / `loginSku` Bicep parameters have existed since the initial template (`bicep/main.bicep`); v0.21.4 just exposes them. Useful for shrinking scheduler/login down to `Standard_D2as_v5` for rapid-test deploys, or scaling them up when the cluster reaches a few hundred compute nodes and slurmctld + control-plane overhead grows.

### Changed
- `DeployArgs` gains `scheduler_sku: String` and `login_sku: String` (both with sensible defaults). ARM parameter envelope adds `schedulerSku` and `loginSku` entries.

## [0.21.3] - 2026-05-25

### Added
- **`azcluster ssh`/`exec`/`scp` `--host <hostname>`** — hop through login to an arbitrary in-VNet hostname (typically a compute VMSS instance like `<cluster>-cpu-0001`). Mutually exclusive with `--scheduler`. Works under both no-bastion (operator → login → host via `-J`) and bastion (operator → bastion-proxy → login → host via `-J`) routing. The VNet's auto-registered DNS resolves the compute hostname from the login VM, so no manual IP lookups are needed.
- **`azcluster ssh`/`exec`/`scp` `--user <name>` (short `-u`)** — connect as a non-admin user, intended for LDAP users created via `azcluster user add`. The same user identity is used at BOTH hops (ProxyJump and final destination), because LDAP users have their pubkey distributed via SSSD on login + compute, but admin's authorized_keys may not contain the connecting operator's key. Defaults to `state.admin_username` (azureuser) when omitted, preserving v0.21.2 behavior.
- **`azcluster exec --forward-agent` / `-A`** — opt-in SSH agent forwarding for nested ssh from the remote shell. Off by default to keep the auditable behavior of v0.21.2.

### Changed
- `ConnectArgs` (ssh), `ExecArgs` (exec), and `ScpArgs` (scp) gain `--host` + `--user` flags; `ExecArgs` additionally gains `-A/--forward-agent`. `--scheduler` is marked `conflicts_with = "host"` via clap.
- `ssh()`, `exec()`, and `scp()` internally use a unified `connect_user = args.user.unwrap_or(admin_username)` and `jump_user = connect_user` so a single identity authenticates at every hop.

### Notes
- **Known limitation:** `--scheduler --user <ldap-user>` does NOT work. The scheduler hosts the slapd LDAP server itself and runs no SSSD client, so LDAP users have no local presence there and pubkey lookup fails. Job submission happens from login (where SSSD resolves LDAP users), not from scheduler. If you need shell access to scheduler, use the admin user (`azcluster ssh <name> --scheduler`).

## [0.21.2] - 2026-05-25

### Fixed
- **`/etc/slurm/*.conf` permissions regression** — when `install-scheduler.sh` (or login/compute equivalents) aborts mid-bootstrap (e.g. transient `curl 404` fetching a not-yet-published release tarball during a deploy that races CI), cloud-init's `runcmd` 0077 umask leaves slurm.conf / plugstack.conf at 0600. Non-root `srun`/`sinfo` then fails with `Permission denied`. Defense-in-depth: install scripts now `trap 'chmod 0644 /etc/slurm/*.conf' EXIT` immediately after `set -euo pipefail`, so the chmod always runs regardless of where the script aborts. Live-reproduced on `v211b` scheduler.
- **`azcluster timings` no longer reports only the root deployment.** `timings::az_op_to_timing` now falls back to parsing `targetResource.id` for `resourceGroup`/`resourceType`/`resourceName` when ARM REST omits the explicit fields (the v0.20.0 native-ARM-REST switch surfaced this — sub-scope nested `Microsoft.Resources/deployments` entries have an `id` but no top-level `resourceGroup`, breaking the recursion gate). `collect_sub_operations` now recurses into sub-scope nested deployments via `list_subscription_deployment_operations` when the parsed rg is empty. v211b previously captured 1 op + `total -0s`; now captures all ~18 module ops + a non-negative total. Float-normalization on the sum prevents `-0.0` display.
- **`azcluster status` bootstrap probe** — (a) prefers the `/var/log/azcluster/ready` marker over the raw `install.log` tail (now prints `READY` instead of a misleading curl 404 left in the log buffer); (b) honors bastion routing via `should_use_bastion` + the existing `bastion-proxy` ssh `ProxyCommand`, so the probe works for clusters deployed with `--bastion` and no public IP; (c) probe enabled whenever login is reachable (public IP OR bastion).

### Changed
- `cloud-init/{scheduler,login,compute}.yaml.tmpl` install scripts gain an `EXIT` trap as defense-in-depth for the chmod 0644 step.
- `crates/azcluster-cli/src/timings.rs` adds `parse_target_id` helper + 3 unit tests covering rg-scoped, sub-scope nested deployment, and the id-fallback path.

## [0.21.1] - 2026-05-25

### Added
- **`azcluster scp <name> <SRC>... <DST>`** — `scp` wrapper that mirrors real scp syntax: paths with `<node>:<path>` are remote, bare paths are local. `<node>` defaults to `login` when empty (`:/shared/foo`); other accepted values are `scheduler` and any compute hostname (e.g. `vmss-<cluster>-<pool>NNNNNN`). Flags: `-r` recursive, `-p` preserve mtime/mode, `-i <key>`, `--no-bastion` opt-out. Bastion-aware: auto-injects `-o ProxyCommand="azcluster bastion-proxy ..."` when the cluster has no public IP, and `-o ProxyJump=azureuser@<login>` for scheduler/compute targets without bastion. A single invocation can only touch one remote node (no remote-to-remote); enforced at parse time. Compute paths always traverse login (bastion-proxy targets login + ProxyJump to compute hostname); scheduler paths under bastion go direct to the scheduler VM (no jump).

### Changed
- **`azcluster login --subscription <id>` fast-path.** When the operator's token cache already contains a valid (or refreshable) account for the requesting principal, `login` now rebinds the cache entry under the target subscription id in ~6 ms instead of triggering a fresh interactive OAuth2 flow. Management-scope access tokens are principal-scoped (audience `https://management.azure.com/`), not subscription-scoped, so rebinding is safe. Workaround for Microsoft tenants that block device-code flow (Conditional Access error AADSTS53003): `azcluster login` once interactively (PKCE in browser), then re-run with `--subscription <id>` from any shell to switch the bound subscription with zero re-auth. `try_rebind_cached(target_sub_id, tenant_filter)` in `auth/token_provider.rs` is the new helper; calls `TokenProvider::refresh_with_token` only when the access token is within 5 min of expiry, otherwise reuses verbatim.

### Fixed
- Replace `Option::is_none_or` with explicit `match` in `auth/token_provider::try_rebind_cached` to satisfy workspace MSRV clippy gate (`clippy::incompatible_msrv`).

## [0.21.0] - 2026-05-25

### Added
- **`--bastion` deploy flag**. Provisions Azure Bastion (Standard SKU + Standard Static public IP + `enableTunneling: true`) into a new `AzureBastionSubnet` (`10.42.0.64/26`) carved out of the cluster VNet. Adds ~3-5 min to deploy time. Use when the cluster is deployed without `--login-public-ip` (the recommended secure default) so the operator can still `ssh`/`exec`/`tunnel` into login + scheduler via Azure-native tunneling.
- **`azcluster ssh`/`exec`/`tunnel` auto-route through Bastion** when `state.bastion_enabled && state.login_public_ip.is_none()`. The CLI sets `ProxyCommand="azcluster bastion-proxy --cluster <name> --target {login|scheduler}"` on the spawned `ssh`. With `--scheduler`, the proxy connects straight to the scheduler VM's resource ID (Bastion can tunnel to any VM in the VNet) — no `-J` jump needed. Opt-out via `--no-bastion` on any of the three commands to surface the legacy "no public IP" error.
- **Hidden `azcluster bastion-proxy` subcommand** (`--cluster <name> --target {login|scheduler} [--port N]`). stdio bridge: fetches a Bastion tunnel token (`POST https://<bastion-fqdn>/api/tokens` form-encoded with the operator's AAD bearer), opens `wss://<endpoint>/webtunnelv2/<token>?X-Node-Id=<node>` via hand-rolled rustls + manual WS framing (Azure Bastion's WS upgrade response is non-RFC-strict — `tokio-tungstenite` rejects it), and pipes the binary WS frames to/from stdio. Designed exclusively as an `ssh ProxyCommand` consumer; `--hide`d from `--help`. Deletes the tunnel token on shutdown.
- New `crates/azcluster-cli/src/bastion/` module: `client.rs` (Bastion token API, async `reqwest`), `tunnel.rs` (manual WS framing — opcodes 0x01/0x02/0x08/0x09/0x0A, client→server masking, 2/8-byte extended payload length, `tokio-rustls` 0.26 TLS).
- `ClusterState` gains `bastion_enabled`, `bastion_name`, `bastion_dns_name`, `bastion_resource_id` (all `#[serde(default)]` for backward compat). `PendingDeploy` gains `bastion_enabled` for `--no-wait` → `azcluster resume` round-trips.

### Changed
- `bicep/modules/network.bicep` now accepts `enableBastion bool=false`; when true, appends `AzureBastionSubnet = cidrSubnet(vnetAddressPrefix, 26, 1)` (10.42.0.64/26) to `subnets` and exposes `bastionSubnetId`.
- `bicep/cluster.bicep` + `bicep/main.bicep` thread `enableBastion`; cluster.bicep conditionally instantiates `module bastion 'modules/bastion.bicep' = if (enableBastion)` and surfaces `bastionId`/`bastionDnsName`/`bastionName` outputs (BCP318 warning on conditional output access matches the existing `monitoring!.outputs.*` pattern).
- `bicep/main.json` regenerated (185306 bytes, up from 177730).
- Workspace `Cargo.toml` adds `rustls = "0.23"`, `tokio-rustls = "0.26"`, `webpki-roots = "0.26"`, plus extra `tokio` features (`io-util`, `io-std`, `net`, `sync`, `time`); `crates/azcluster-cli/Cargo.toml` consumes them.

## [0.20.0] - 2026-05-25

### Removed
- **`az` CLI dependency**. The CLI is now a fully self-contained Rust binary that authenticates to Azure via OAuth2 directly and calls ARM REST natively. All 17 prior `az` shell-out call sites have been replaced across 8 vertical slices on `refactor/native-azure-sdk`. End users no longer need `az`, `az login`, or `az bicep` on their workstation. `ensure_az()` and every `Command::new("az")` invocation have been deleted from `crates/azcluster-cli/src/main.rs`.

### Added
- **`azcluster login`** — native Azure OAuth2. Interactive PKCE flow in a browser by default; `--device-code` for headless / SSH sessions; `--tenant <id>` / `--subscription <id>` flags for non-interactive selection. Uses the well-known Azure CLI public client ID `04b07795-8ddb-461a-bbee-02f9e1bf7b46` and the `https://management.azure.com/.default offline_access` scope. Tokens (access + refresh) cache to `~/.azure/azcli_tokens.json` (mode `0600`). Token refresh is automatic on every ARM call when the access token is within 5 minutes of expiry. **NOT compatible with the Python `az` CLI's MSAL token cache** — this is intentional (different cache schema, no shared lock contract).
- **`ArmClient`** (`crates/azcluster-cli/src/arm/client.rs`) — a typed, retrying ARM REST client. Surface: subscription enumeration, resource-group CRUD, subscription-scope deployment LRO (`PUT`/`POST what-if`/`GET` poll until terminal), deployment operation listing (subscription + RG), VMSS get/scale + async-operation polling, Grafana endpoint resolution + dashboard import (`POST /api/dashboards/db` with ARM bearer; 10×30s retry on 401/403/NoRoleAssignedException). All endpoints version-pinned in `ApiVersions` (rg=2024-03-01, deployment=2024-03-01, compute=2024-07-01, network=2023-11-01, storage=2023-05-01, grafana=2023-09-01).
- **`bicep/main.json` is now committed and embedded into the binary** via `include_str!("../../../bicep/main.json")`. End users never need bicep tooling. Contributors editing `bicep/*.bicep` MUST regenerate `bicep/main.json` (`az bicep build --file bicep/main.bicep --outfile bicep/main.json`); CI's new `bicep` drift-check job (`.github/workflows/ci.yml`) fails the build if the committed JSON drifts from a fresh transpile.
- **Release pipeline** (`.github/workflows/release.yml`) now installs the standalone `bicep` v0.30.23 binary, rebuilds `bicep/main.json` on every tag, ships it in the versioned tarball, AND uploads `azcluster-main-${VERSION}.json` as a separate release asset for operators who want to inspect the ARM template.
- **`--what-if` native LRO**. `azcluster deploy --what-if` now hits `POST /providers/Microsoft.Resources/deployments/{}/whatIf` (202 + `Location` header), polls the async-operation URL until 200, and prints the result JSON. No longer deferred to `az deployment sub what-if`.

### Changed
- **Deployment parameters are now typed `serde_json::Value`** (bool / int / array preserved), not coerced to strings. Each value is wrapped `{"value": v}` per ARM REST contract. Previously the `az deployment sub create --parameters key=value` path stringified everything; this caused subtle Bicep `bool('true')` coercion failures in edge cases.
- **`resolve_template()`** now returns `serde_json::Value`. Default (no `--template`) returns the embedded `main.json`. `--template <path>` must point to a `.json` file; passing `.bicep` returns a clear error directing the user to run `az bicep build` (intentional — the CLI deliberately does not bundle a bicep transpiler).
- `Cargo.toml` workspace version `0.19.4` → `0.20.0`. CLI default `--azcluster-version` bumped to `v0.20.0`.
- `README.md` Prerequisites no longer lists `az` CLI; documents `azcluster login` instead. Status block rewritten for the v0.20.0 refactor.

### Fixed
- Slice-4 commit log notes the previously-broken `create_deployment()` helper in `arm/client.rs` (wrong ARM body shape: missing `location` at body root, `properties` flattened) — replaced with `create_subscription_deployment()` which sends the correct shape (`{ location, properties: { template, parameters, mode } }`).

### Live-validated
- **NOT live-validated against a real Azure deployment.** Tests: 86/86 passing; `cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo fmt --all` clean; release build green; CLI `--help` renders for every subcommand. The v0.19.4 functional baseline (container `_mpi` NCCL on 2× ND96isr_H100_v5 = avg 277 GB/s busbw) carries forward; this release does not change cluster runtime behaviour. Operator-AFK rollout: live-validate `azcluster login` → `azcluster deploy` → `azcluster scale` → `azcluster status` → `azcluster delete` on the next deploy cycle and document results in the v0.20.1 CHANGELOG.

## [0.19.4] - 2026-05-25

### Fixed
- **`UCX_TLS=tcp` in `/etc/enroot/environ.d/50-nccl.env` broke every container that ran an MPI workload over HPC-X/UCX** (e.g. `/usr/local/bin/all_reduce_perf_mpi` shipped in `nvcr.io/nvidia/nemo:25.07.02`, `nvcr.io/nvidia/pytorch:*`, and other NGC containers). UCX is a GLOBAL transport policy: `UCX_TLS=tcp` told HPC-X's UCX layer to use TCP transport on every interface in `UCX_NET_DEVICES`, including `mlx5_ib0..7` which don't expose TCP sockets. Every rank logged `ucp_context.c:1582 UCX ERROR no usable transports/devices (asked tcp on network:mlx5_ib0:1,...)`, OMPI then fell back to its TCP BTL and hung indefinitely in `mca_btl_tcp_endpoint_recv_connect_ack: received unexpected process identifier`. Removed the line from `cloud-init/compute.yaml.tmpl` (both `/etc/profile.d/nccl-azcluster.sh` and `/etc/enroot/environ.d/50-nccl.env`). NCCL itself doesn't use UCX (it has its own `IBext_v10` plugin), so the removal does NOT affect the bare NCCL path; UCX auto-detects `rc,ud,sm,self` for IB MPI workloads. `UCX_NET_DEVICES` is kept so HPC-X picks the 8 NDR rails by name when it auto-detects transports. Live-validated on `paul-azcluster-v194a` (2× ND96isr_H100_v5, `nvcr.io/nvidia/nemo:25.07.02`): `srun --mpi=pmix --container-image=... /usr/local/bin/all_reduce_perf_mpi -b 8M -e 1G -f 2 -g 1` across 16 ranks reaches **avg busbw 277.005 GB/s, peak 439.38 GB/s at 1 GiB**, NCCL using `NET/IBext_v10/N/GDRDMA` on all 8 rails (matches the v0.13.8 bare-metal HPC-X baseline of 434 GB/s — confirming zero container overhead on the IB+SHARP+GPUDirect RDMA path).

### Changed
- `Cargo.toml` workspace version `0.19.3` → `0.19.4`. CLI default `--azcluster-version` bumped to `v0.19.4`.

### Live-validated
- 2-node 16-rank NCCL all-reduce via Slurm + Pyxis + PMIx + HPC-X **`_mpi` binary** on `nvcr.io/nvidia/nemo:25.07.02`. NeMo container ships HPC-X 2.x → `libpmix.so.2.2.35` (NOT PMIx 4.2.x as previously documented — see AGENTS.md correction). PMIx 2↔4 wire compatibility is sufficient for `srun --mpi=pmix` rendezvous. Peak in-place busbw 439.38 GB/s; avg 277.005 GB/s across 8 MB → 1 GiB sweep.

## [0.19.3] - 2026-05-25

### Added
- **`azcluster user add` now auto-registers with Slurm accounting** when the cluster was deployed with `--accounting` (the default). Creates a per-user Slurm account (`sacctmgr -i add account <user> Organization=azcluster`) and adds the user with `DefaultAccount=<user>`, so `sreport user top` can break down compute usage per LDAP user. Closes the v0.19.2 footgun where `azcluster user add paul` would create the POSIX identity but `sbatch` from `paul` returned `Invalid account or account/partition combination specified` until the operator manually ran `sudo sacctmgr -i add user paul DefaultAccount=default` on the scheduler.
- **`azcluster user remove` symmetrically deregisters** the user + per-user account from `slurm_acct_db`, in user-then-account order to respect the FK constraint. Both registration and deregistration are best-effort (non-fatal warnings on failure) so a transient `slurmdbd`/MySQL hiccup never blocks LDAP changes; the warning includes the exact copy-paste recovery command.
- **`ClusterState` now persists `accounting_enabled: bool`** (with `#[serde(default)]` for backward compat) so the CLI can tell, after deploy, whether to run `sacctmgr` from `user add/remove`. Existing `<cluster>.toml` state files load with `accounting_enabled = false`; existing clusters keep working but won't get sacctmgr auto-registration until next deploy.

### Changed
- **`azcluster user list` now prints a formatted table** (`USERNAME`, `UID`, `GID`, `SHELL`, `GECOS`) sorted by uidNumber, instead of dumping raw LDIF. Removes the v0.19.2 "env-dump" UX where operators had to grep through `dn:` / `uidNumber:` / `gecos:` lines themselves. LDIF parser is pure-Rust (~30 LOC), no `ldap3` crate dep added.
- `Cargo.toml` workspace version `0.19.2` → `0.19.3`. CLI default `--azcluster-version` bumped to `v0.19.3`.

### Live-validated
- Deploy on `southafricanorth` / `rg-azcluster-v193a`, 2× ND96isr_H100_v5, `--shared-storage nfs-scheduler --login-public-ip` (accounting on, monitoring off). ARM Succeeded in 1672 s; 24 resources; cluster operational with `h100*` UP idle on `v193a-h100-[0001-0002]`; `accounting_enabled=true` persisted in `~/.config/azcluster/clusters/v193a.toml`. `azcluster user list v193a` renders formatted table (USERNAME/UID/GID/SHELL/GECOS, sorted by uid) ✓. End-to-end round-trip `user remove paul && user add paul && sbatch as paul` reaches `COMPLETED` with `Account=paul` after a SINGLE CLI invocation, no manual `systemctl restart slurmctld` needed. Verified via `scontrol show assoc_mgr account=paul`: cache reflects the new uid (e.g. 20009) immediately after `user add` returns, matching `getent passwd paul` on the scheduler.

### Fixed
- **`azcluster user add/remove` left the scheduler's `slurmctld` assoc_mgr cache holding the previous uid for the same username**, so the freshly-added user couldn't `sbatch` (`Invalid account or account/partition combination`) and required an out-of-band `sudo systemctl restart slurmctld` on the scheduler. Two-layer cache caused this: (1) scheduler-side SSSD (added in v0.19.2) cached the old username→uid mapping; (2) `slurmctld`'s `assoc_mgr` resolved username→uid once via `getpwnam` at startup or first reference and held it for process lifetime — `scontrol reconfigure` does NOT invalidate the assoc_mgr cache. Fix in `user.rs`: after `sacctmgr` add/remove on the scheduler, the CLI now runs `sudo -n sss_cache -u '<user>' && sudo -n sss_cache -E` (clears scheduler SSSD) THEN `sudo -n systemctl restart slurmctld` (forces assoc_mgr to re-resolve via getpwnam against the now-fresh SSSD). 3 s post-restart sleep so the CLI returns only once slurmctld has finished loading. Live-validated: uid 20007 → 20009 in a single CLI call, immediate `sbatch` success.
- **`sacctmgr` add/delete commands were swallowing real failures via blanket `|| true`** in the v0.19.3 first cut. Replaced with an in-shell `sacctmgr_run` retry helper that (a) preserves exit codes via `set +e`/`set -e` framing, (b) treats both `"already exists"` and Slurm's actual `"Already existing"` (capital A) duplicate-account stderr as idempotent, (c) retries up to 12 × 5 s on transient `"Connection refused"` / `"cluster has not been added"` / `"is not registered"` strings (slurmdbd bootstrap windows), (d) surfaces every other non-zero as a fatal error with the stderr captured. The trailing `sss_cache -u`, `sss_cache -E`, and `systemctl restart slurmctld` calls are intentionally `|| true` (best-effort) — these are cache-invalidation conveniences, not state-changing operations.

## [0.19.2] - 2026-05-24

### Added
- **`azcluster deploy --extra-package <pkg>` (repeatable)**: installs arbitrary apt packages on every node (scheduler / login / compute) right after the cluster.env / install.env sources during cloud-init, before any service starts. Validated live on `southafricanorth` with `--extra-package git-lfs --extra-package python3.12-venv` — both packages report `ii` on scheduler, login, and compute after deploy. Empty default; package names are validated CLI-side against `^[a-z0-9][a-z0-9+.:-]*$` (regex-free Rust validator). Persisted in `ClusterState` + `PendingDeploy` (`extra_packages: Vec<String>` with `#[serde(default)]` for backward compat) and threaded through Bicep (`extraPackages` param on `main.bicep` → `cluster.bicep` → `modules/{scheduler,login,compute}.bicep`).
- **Scheduler-side SSSD** so `getent passwd <ldap-user>` and `id <ldap-user>` resolve on the scheduler too (previously only login + compute did). Wired against the local `slapd` via `ldap://127.0.0.1` (NOT `ldapi:///` — see Fixed below). `services = nss, pam` only; no `ssh` provider needed (operators don't SSH as LDAP users to the scheduler). Followed by `systemctl restart slurmctld` to clear any cached uid lookups from before SSSD came up.
- **Per-user enroot CACHE path** on every compute node: `/etc/enroot/enroot.conf` now sets `ENROOT_CACHE_PATH /var/lib/enroot-data/cache/user-$(id -u)`. Parent dir `/var/lib/enroot-data/cache/` is already mounted on `/mnt/nvme` (the NVMe RAID-0) via the existing v0.13.5 symlink, and is 1777-sticky so each user's subdir is created on first import without collision. Matches the per-user pattern used by CCWS (cyclecloud-slurm-workspace).
- **Idempotent `azcluster deploy`**: re-running `deploy` after a crash or with a stale `<name>-secrets.toml` now reuses the persisted `ldap_admin_password` + `mysql_admin_password` instead of generating fresh ones (which ARM would reject as different from the LDAP/MySQL admin password already in the cluster). `ClusterSecrets::load_optional` is called first; if absent, fresh secrets are generated and saved before ARM submission. Surfaced by v0.17 live-validation (wrapper-CLI crash mid-deploy left ARM-Succeeded but post-deploy hooks unrun; v0.18.3 captured the symptom, v0.19.2 fixes the root cause).

### Changed
- `Cargo.toml` workspace version `0.19.1` → `0.19.2`. CLI default `--azcluster-version` bumped to `v0.19.2`.
- `walkthrough-dgxc.md` Tier 2 rewritten as a **per-user** flow rooted at `$HOME/dgxc` (under the LDAP user's `/shared/home/<user>` shared home), not `/shared/dgxc` owned by root. Operators run `azcluster user add` once per user; users install DGXC + register their own NGC + HF tokens + `llmb-install express` themselves. Removes the implicit "operator installs DGXC globally for everyone" assumption. Notes that `python3.12-venv` is required (use `--extra-package python3.12-venv` at deploy time) and explains the per-user enroot cache.

### Fixed
- **`EXTRA_PACKAGES` env var was unquoted in install.env / cluster.env**, so when the value contained whitespace (e.g. `EXTRA_PACKAGES=git-lfs python3.12-venv`), bash parsed the line as a scoped assignment + command execution under `set -e`, exited 127 (`python3.12-venv: command not found`), and aborted `install-scheduler.sh` at line 1. Caught by live-validation on deploy `#1`. Fix: emit `EXTRA_PACKAGES="{{EXTRA_PACKAGES}}"` (quoted) in all three templates.
- **Scheduler `az login --identity` was unconditionally non-defensive**, so under `--no-monitoring --no-accounting` test mode (where the UAI has no subscription-level RBAC) it returned non-zero and `set -e` killed `install-scheduler.sh` mid-bootstrap. The scheduler doesn't actually invoke any `az` subcommands later, so the login was vestigial defensive code that became fatal. Fix: append `|| true 2>&1`. Pre-existing bug surfaced by deploy `#2`.

### Live-validated
- Deploy on `southafricanorth` / `rg-azcluster-v192a`, 2× ND96isr_H100_v5, `--shared-storage nfs-scheduler --no-monitoring --no-accounting --login-public-ip --extra-package git-lfs --extra-package python3.12-venv`. ARM Succeeded in 494 s. Scheduler: `dpkg -l git-lfs python3.12-venv` → both `ii`, `sssd active`, `slurmctld active`, `slapd active`, `sinfo` shows `h100*` UP idle with `v192a-h100-[0001-0002]`, `ldapsearch -H ldapi:///` works, `/var/log/azcluster/ready` present. Login: same packages installed, `sssd active`, `sackd active`. Compute (via `srun -p h100 -N1`): `ENROOT_CACHE_PATH /var/lib/enroot-data/cache/user-$(id -u)` in `/etc/enroot/enroot.conf`, both packages `ii`, `/mnt/nvme drwxrwxrwt`. LDAP user `paul` created via `azcluster user add`, `id paul` resolves on scheduler (after SSSD `ldap_uri` fix), `ssh paul@<login>` works with `/shared/home/paul` shared across nodes. Bare-metal `srun --mpi=pmix bash -c '...PMIX_RANK...'` and `srun --mpi=pmi2 bash -c '...PMI_RANK...'` both form a 4-rank world across 2 nodes; `pmi2` returns size cleanly (`size=4`), `pmix` does not populate `PMIX_SIZE` env (but does propagate `PMIX_RANK`).

### Known limitations
- **Containerised `--mpi=pmix` cross-node** still produces incomplete rank propagation when launched via Pyxis (`--container-image` per srun). The container import itself takes ~20 min on first run even with `/mnt/nvme` RAID-0 (NGC pull dominates), and re-using a per-node `--container-name` across multiple srun steps fails because pyxis container names are per-node, not global. Documented; no in-product fix in v0.19.2. Workaround: use `--mpi=pmi2` if the container's MPI stack supports it, or use the `srun --container-image=... torchrun` pattern (one container per srun, torchrun handles rendezvous over TCP) that the v0.19.1 `walkthrough.md` documents.



## [0.19.1] - 2026-05-24

### Added
- **`azcluster resume --name <name>`** is the new explicit verb to run post-deploy hooks (state file, timings JSON, Grafana dashboard import) after a `--no-wait` deploy. Reads the persisted `PendingDeploy` marker, polls ARM until terminal (Succeeded / Failed / Canceled), loads the cluster secrets, runs `finalize_deploy()`, and deletes the marker. Replaces the v0.19.0 "re-run `azcluster deploy` with the same args to finalize" UX — that overload was confusing because "deploy" reads as "deploy again".
- **`PendingDeploy` marker is now written for blocking deploys too**, BEFORE `az deployment sub create` is invoked. If the operator's shell dies mid-ARM (terminal disconnect, OOM kill, hibernate), the deploy is now recoverable via `azcluster resume --name <name>` — same path as `--no-wait`. On the blocking happy path the marker is deleted at the end of `deploy()`, so steady-state operation is unchanged.
- **`azcluster status <name>` always nags about pending markers.** When a pending marker exists the status block now prints `-> run azcluster resume --name <name> once ARM state is Succeeded`. The "no cluster state yet" footer points at `resume` (not at re-running `deploy`).

### Changed
- **`azcluster deploy` is now strictly linear and single-purpose**: it submits ARM and (without `--no-wait`) runs `finalize_deploy()` inline. The v0.19.0 "detect pending marker, switch to resume mode" magic at the top of `deploy()` is gone. The only resume path is `azcluster resume`.
- `resume_deploy()` deleted; new `resume()` (driven by `ResumeArgs`) absorbs its body. `resume()` builds a synthetic `DeployArgs` from the pending marker + an `az group show --query location` lookup (PendingDeploy does not persist `location`). `finalize_deploy()` retains its name as the internal helper that runs post-deploy hooks (state save, secrets persist, timings capture, dashboard import).
- `--no-wait` deploy completion message now points at `azcluster resume --name <name>`, not "re-run azcluster deploy".
- Workspace version `0.19.0` → `0.19.1`. CLI default `--azcluster-version` bumped to `v0.19.1`.


## [0.19.0] - 2026-05-24

### Added
- **`azcluster deploy --no-wait`** submits the ARM deployment with `--no-wait` and exits ~immediately after persisting cluster secrets and a `PendingDeploy` marker at `~/.config/azcluster/clusters/<name>-pending.toml`. Removes the previous requirement that the operator keep their shell/CLI alive for 7-15 minutes during provisioning. Re-running `azcluster deploy --name <name>` (with the same args) detects the pending marker, polls `az deployment sub show` every 30 s (cap 90 min), and on `Succeeded` runs all post-deploy hooks (state file, timings capture, Grafana dashboard import) before deleting the pending marker. If the deployment ended in `Failed`/`Canceled`, the resume aborts with a one-line recovery hint pointing at `azcluster delete` + manual pending-file removal.
- **`PendingDeploy` state file** (`{cluster, deployment_name, resource_group, started_at, monitoring_enabled, accounting_enabled, shared_storage, grafana_location}`) with `save/load_optional/delete`. Round-trip unit test in `cluster_state.rs`.
- **`azcluster status <name>` now surfaces ARM phase + cloud-init progress.** If a `PendingDeploy` marker exists it prints the ARM provisioning state and operation roll-up (`N total: X succeeded, Y running, Z failed`). If the cluster state file already exists AND the login VM has a public IP, the command also runs a short SSH probe (`ConnectTimeout=8`, `BatchMode=yes`) against `login` and (via ProxyJump) `scheduler`, printing the last line of `/var/log/azcluster/install.log` from each. Both probes are best-effort — an SSH failure prints `ERR (...)` and the command continues. Status also now works against a `--no-wait` deploy that hasn't finalized yet (no state file required).
- **`azcluster delete <name>` works without a state file** when only a pending marker exists, so an aborted `--no-wait` deploy can still be torn down via the CLI. Always removes both the state file and the pending marker if present.

### Changed
- `deploy()` refactored: `finalize_deploy()` helper extracts all post-ARM work (state save, secrets persist, timings, dashboard import); `resume_deploy()` handles the pending-marker path; `poll_deployment_until_terminal()` shared by resume. `ClusterSecrets` are now persisted BEFORE `az deployment sub create` returns so a `--no-wait` deploy can resume after operator logout or laptop loss (ARM secure parameters are not retrievable from `properties.outputs`).
- Workspace version `0.18.3` → `0.19.0`. CLI default `--azcluster-version` bumped to `v0.19.0`.

### Fixed
- **`bootstrap_probe` in `azcluster status` was silently returning empty.** The probe invoked `ssh ... "--" "bash" "-lc" "tail -n1 /var/log/..."` via `Command::new("ssh").args(...)`. OpenSSH whitespace-joins all positional remote args before exec on the remote side, so the remote login shell received `bash -lc tail -n1 /var/log/...` — bash parsed that as `-c 'tail'` and ignored the rest (tail with no args reads stdin, returning nothing immediately). Live-validated on `v19uxtest`: both `login` and `scheduler` probes printed empty after a successful deploy. Fix: pass the remote command as a single string (`tail -n1 /var/log/azcluster/install.log 2>/dev/null || echo '<no log yet>'`); ssh's join-then-exec then yields the correct shell command. After the fix the probe correctly prints `Executing: /usr/lib/systemd/systemd-sysv-install enable sssd` (login) and `... enable slapd` (scheduler) — the cloud-init checkpoints.

### Deferred
- **Entra ID (`aad-login`) integration** deferred to v0.19.1 / v0.20. Plan is Momus-approved (`.sisyphus/plans/v0.19-aad-login.md`), blocked only by the interactive device-code flow being explicitly excluded from automated testing.


## [0.18.3] - 2026-05-24

### Added
- **`azcluster user`/`sshkey` mutations now push SSSD cache invalidation on the login VM** immediately after the LDAP write returns. New `flush_login_sssd_cache(state, user)` opens a direct (non-jump) ssh to `<login-public-ip>` and runs `sudo -n sss_cache -u <user>` + `sudo -n sss_cache -E`. Best-effort — any failure (no public IP, ssh rejected, sudo absent) logs a warning and falls through to the v0.18.2 60 s `entry_cache_timeout` floor; the LDAP write itself is already durable in slapd. Wired into `user_add`, `user_remove`, `sshkey_add`, `sshkey_remove`. After v0.18.2 live-validation showed sshkey propagation at 47 s (within the new 60 s TTL but still operator-perceptible), this lands it at a couple of seconds for the SSH-as-LDAP-user path while keeping compute's longer-tail propagation bounded by the same 60 s TTL. 2 new unit tests cover the flush command shape (`sudo -n sss_cache -u 'user'`, `|| true` for best-effort, username quoting).
- **`ClusterSecrets::load_optional(name)`** returns `Ok(None)` when the secrets file is absent (vs. `load()` which errors). Used by the deploy flow to detect re-invocations.

### Fixed
- **`azcluster deploy` is now re-invocable against an existing cluster.** v0.17 live-validation surfaced that re-running deploy against a cluster whose ARM deployment had already succeeded failed with `Missing input parameters: ldapAdminPassword` because the CLI generated a fresh password every invocation and ARM rejected the second deploy when the value differed from what slapd had already been provisioned with. v0.18.0 + .1 + .2 inherited the same gap (also affected `mysqlAdminPassword` when accounting was on). Fix: deploy now calls `ClusterSecrets::load_optional` first; if a secrets file exists, it reuses both `ldap_admin_password` and (when `--accounting`) `mysql_admin_password` and prints `==> reusing persisted secrets for cluster '<name>' (re-invocation safe)`. First-deploy behaviour unchanged (fresh generation + persistence). Re-invocation now succeeds and re-runs the post-deploy hooks (dashboard import, timings JSON, state file refresh) without ARM drift.

### Changed
- **`ClusterSecrets` schema**: added `mysql_admin_password: Option<String>` (defaults to `None`, `#[serde(default)]`). Backward-compatible with v0.18.x secrets files that only carry `ldap_admin_password`. 2 new unit tests cover the round-trip and the v0.18.x → v0.18.3 read path.
- Workspace version `0.18.2` → `0.18.3`. CLI default `--azcluster-version` bumped to `v0.18.3`.

### Deferred
- **Entra ID (`aad-login`) integration** deferred again, now to v0.19. Unchanged blocker: interactive device-code flow explicitly excluded from automated testing.

## [0.18.2] - 2026-05-24

### Changed
- **SSSD attribute cache shortened to 60 s** on both login and compute. `entry_cache_timeout = 60` + `entry_cache_user_timeout = 60` in `/etc/sssd/sssd.conf` (was the upstream default of 5400 s / 90 min). v0.18.1 live-validation surfaced that after `azcluster user sshkey add/remove` the new `sshPublicKey` LDAP attribute was correct in slapd but `sss_ssh_authorizedkeys` on the login node served the cached value for up to 90 min, forcing operators to `sudo sss_cache -u <user>` to force a refresh. The new TTL bounds the worst-case propagation lag to 1 minute on a 1-2 RPS LDAP load that the scheduler `slapd` easily absorbs. No regression to base auth latency: SSSD still satisfies repeat lookups from cache; the only difference is when the cache is considered stale.
- **`pam_mkhomedir umask=0077`** on login and compute so per-user home directories created on first login are `drwx------` (`0700`) instead of the prior `drwxr-x---` (`0750`) from the default `umask=0022`. Tightens hygiene without breaking anything (group `azusers` had only `r-x` before; nothing relies on that). Applied by `sed`-rewriting the `session optional pam_mkhomedir.so …` line that `pam-auth-update --enable mkhomedir` drops into `/etc/pam.d/common-session`.
- Workspace version `0.18.1` → `0.18.2`. CLI default `--azcluster-version` bumped to `v0.18.2`.

### Deferred
- **Entra ID (`aad-login`) integration** deferred again, now to v0.19. Same blocker as v0.18.0/v0.18.1: requires Azure AD app registration + UAI token-exchange + an interactive device-code flow that is explicitly excluded from automated testing. v0.18.x ships the fully-automatable LDAP path.

## [0.18.1] - 2026-05-24

### Fixed
- **`cn=uidNext` LDIF rejected by slapd** (regression in v0.18.0). The entry declared only `objectClass: extensibleObject` (AUXILIARY). OpenLDAP 2.6 requires at least one STRUCTURAL objectClass per entry; slapd refused the add with `Object class violation (65) — no structural object class provided`. Combined with `set -euo pipefail` in `install-scheduler.sh`, this aborted the entire scheduler bootstrap before reaching `install -d /shared/home` and the `touch /var/log/azcluster/ready` marker — so the cluster came up half-provisioned, the UID counter was absent, and `azcluster user add` could not allocate UIDs. Fix: add `objectClass: device` (structural; requires only `cn`, which we already provide) alongside `extensibleObject` for the `uidNumber` attribute. Live-reproduced on `paul-azcluster v18a` (southafricanorth) and verified the fix loads the LDIF cleanly.
- **openssh-lpk schema add was racy and idempotency-broken** (regression in v0.18.0). The bootstrap wrapped the `ldapadd` in `if ! ldapsearch ... | grep -q openssh-lpk; then`, which (a) is sensitive to slapd's `cn=schema,cn=config` taking a moment to settle after `apt install slapd` and (b) gives no recovery path on the second run because re-adding a present schema returns exit 68 (`LDAP_ALREADY_EXISTS`) which `set -e` propagates. Same problem on the base LDIF guard (`if ! ldapsearch ... grep -q '^dn: ou=people'`) — first-run failures left a partial DIT with no way to converge. Fix: drop the guards; run `ldapadd` unconditionally with `-c` (continue on per-entry errors); explicitly tolerate exit 68 (everything-already-exists) and fail loudly on any other non-zero status.
- **`/shared` came up `drwxrwx--- nobody:nogroup` from ANF** so LDAP-resolved users (`uid 20001`, `gid 20000(azusers)`) could not traverse it to reach their home directories under `/shared/home/<user>`. `pam_mkhomedir` (running as root inside PAM) successfully created `/shared/home/testuser` owned `testuser:azusers` 750, but the user himself got `Permission denied` on `chdir /shared/home/testuser` because the parent `/shared` denied traversal. Fix: scheduler bootstrap now `chmod 0755 /shared` immediately before creating `/shared/home`, making the share world-traversable while keeping individual home dirs 750.
- **`azcluster user` CLI could not authenticate to slapd** (regression in v0.18.0). The CLI sent the LDAP admin password over the scheduler ssh-jump's stdin pipe and relied on `bash -lc 'LDAP_PW=$(cat); …'` to read it. Under OpenSSH ProxyJump + no-PTY + `bash -lc`, the command-substitution subshell did not inherit the ssh channel's pipe stdin and `$(cat)` returned **0 bytes** — every LDAP operation therefore performed an unauthenticated bind and was rejected by slapd with `ldap_bind: Server is unwilling to perform (53) — unauthenticated bind (DN with no password) disallowed`, so `user add` / `user remove` / `user list` / `sshkey *` / UID allocation all failed end-to-end. Fix: drop stdin entirely. The CLI now base64-encodes both the password and the LDIF, inlines them directly in the remote command line (`PW=$(printf %s '<b64>' | base64 -d); LDIF=$(printf %s '<b64>' | base64 -d); printf %s "$LDIF" | ldapadd -x -D '…' -w "$PW" -H ldap://127.0.0.1 -c`), and uses `Command::output()` (no `Stdio::piped()` stdin). Adds a 20-LOC `b64_encode` helper (RFC 4648 alphabet, with `=` padding) — no new external dependencies. 3 new unit tests cover the base64 encoder + command construction.

### Changed
- Workspace version `0.18.0` → `0.18.1`. CLI default `--azcluster-version` bumped to `v0.18.1`.
- Workspace test count: 39 → 42 (added `b64_matches_rfc4648_vectors`, `build_write_cmd_inlines_b64_and_no_stdin`, `build_search_cmd_inlines_password_b64` in `user.rs`).

## [0.18.0] - 2026-05-23

### Added
- **LDAP-backed user management.** The scheduler now runs `slapd` (configured non-interactively via `debconf-set-selections` during cloud-init) with base DN `dc=azcluster,dc=local`, admin DN `cn=admin,dc=azcluster,dc=local`. The OpenSSH-LPK schema is loaded so user entries can carry `sshPublicKey` attributes. Base structure (`ou=people`, `ou=groups`, default group `cn=azusers,…` gid 20000, `cn=uidNext` counter starting at 20001) is seeded on first boot.
- **SSSD on login + compute** (`sssd`, `sssd-ldap`, `libnss-sss`, `libpam-sss`, `oddjob-mkhomedir`). Config: `services = nss, pam, ssh`, `id_provider = ldap`, `ldap_uri = ldap://<scheduler-ip>`, `ldap_user_extra_attrs = sshPublicKey:sshPublicKey`, `ldap_user_ssh_public_key = sshPublicKey`, `override_homedir = /shared/home/%u`. `pam-auth-update --enable mkhomedir` creates home directories on first login. sshd `AuthorizedKeysCommand /usr/bin/sss_ssh_authorizedkeys` resolves authorized keys from LDAP at SSH-connect time.
- **`azcluster user` CLI subcommand** with `add`, `remove`, `list`, and `sshkey {add, remove, list}`. UID auto-allocated from the `cn=uidNext` counter (operator can override with `--uid`); default gid 20000. `--ssh-key <file>` (repeatable on `add`) seeds initial `sshPublicKey` attributes. All ops run on the scheduler via `ssh -J <login> <scheduler>`; the LDAP admin password is sent over stdin (never argv) and the scheduler's local `ldapadd`/`ldapmodify`/`ldapsearch` talks to `ldap://127.0.0.1`.
- **CLI-side secret store.** Deploys auto-generate a 36-char LDAP admin password (reusing the existing `gen_mysql_password` helper) and persist it to `~/.config/azcluster/clusters/<name>-secrets.toml` (mode `0600`). Sibling file pattern keeps the secret out of `<name>.toml` (which is read by `azcluster status`).
- **Bicep parameter threading.** New `@secure() ldapAdminPassword` param flows `main.bicep` → `cluster.bicep` → `scheduler.bicep` and into `cloud-init/scheduler.yaml.tmpl` via the existing `replace(...)` chain, mirroring the MySQL accounting password pattern. Compute and login use the existing `SCHEDULER_IP` substitution; no new bicep params needed (LDAP traffic is intra-VNet on port 389, already allowed by the `internalNsg`'s `allow-vnet-inbound` rule, and on login by the default NSG `AllowVnetInBound`).
- **8 new unit tests** in `crates/azcluster-cli/src/user.rs` covering LDIF rendering (add user, delete user, add/remove ssh key, uid bump) and username validation (`[a-z][a-z0-9_-]{0,31}`). Total CLI test count: 6 → 14. Workspace total: 31 → 39.

### Changed
- Workspace version `0.17.0` → `0.18.0`.
- CLI default `--azcluster-version` bumped to `v0.18.0`.
- `cloud-init/scheduler.yaml.tmpl` now installs `slapd`, `ldap-utils`, and `debconf-utils`; creates `/shared/home` (0755 root:root) so SSSD's `oddjob-mkhomedir` can create per-user subdirectories at first login.

### Deferred
- **Entra ID (`aad-login`) integration** deferred to v0.18.1. Adding it requires an Azure AD app registration + UAI token-exchange wiring that is not CI-testable end-to-end without an interactive device-code flow, and the user explicitly excluded the device-code flow from automated testing. v0.18.0 ships the LDAP + SSH-key authentication path which is fully automatable and live-validatable on the standard test cluster.


## [0.17.0] - 2026-05-23

### Added
- **Prometheus textfile metrics from `azhealthcheck` + Grafana `Node Health Checks` dashboard.** New `--metrics-dir <path>` flag on `azhealthcheck` writes a Prometheus exposition file (`azhealthcheck.prom`) atomically via `tmp + rename(2)` with mode `0644` so the unprivileged `node_exporter` user can scrape it. The compute cloud-init wrapper (`/usr/local/sbin/azcluster-healthcheck`) now passes `--metrics-dir /var/lib/node_exporter/textfile_collector`, and `node_exporter.service` is started with `--collector.textfile --collector.textfile.directory=/var/lib/node_exporter/textfile_collector`. The directory is pre-created `node_exporter:node_exporter 0755` so the service starts cleanly even before the first healthcheck run.
- **Metrics emitted** (labelled by `check` and `host`; `host` defaults to `/etc/hostname` and can be overridden with `--metrics-host`):
  - `azcluster_healthcheck_severity{check,host}` — `0`/`1`/`2` per check.
  - `azcluster_healthcheck_findings_total{check,host}` — number of findings emitted by each check on this run.
  - `azcluster_healthcheck_worst_severity{host}` — max severity across all checks.
  - `azcluster_healthcheck_last_run_timestamp_seconds{host}` — unix time of the most recent run; the dashboard alerts when this falls more than 10 min behind `time()`.
- **`grafana/dashboards/health.json`** — new auto-imported dashboard (`uid: azcluster-health`): per-node worst-severity stat tiles (green/yellow/red), per-check severity heatmap, cluster-wide findings-by-check timeseries, "seconds since last healthcheck run" tile (thresholds: 10 min warn / 30 min crit), node counts in WARN/ERROR, and a sortable per-node/per-check table with value mappings. Templating vars: `$host`, `$check`. Wired into `crates/azcluster-cli/src/main.rs` via the existing `DASHBOARDS` `include_str!` array; the CLI imports it post-deploy alongside `node.json`/`slurm.json`/`gpu_ib.json`.
- **5 new unit tests** in `crates/azhealthcheck/src/metrics.rs` covering exposition format, severity mapping, label escaping (`"`, `\`, `\n`), empty-outcome edge case, atomic write with `0644` mode, no-temp-file leakage on overwrite, and parent-dir auto-creation. Test count: 14 -> 19.

### Changed
- Workspace version `0.16.1` -> `0.17.0`.
- CLI default `--azcluster-version` bumped to `v0.17.0`.


## [0.16.1] - 2026-05-23

### Fixed
- **`azhealthcheck` never actually ran on v0.16 nodes — every CPU node self-drained every 5 min.** `cloud-init/compute.yaml.tmpl` contained two blocks writing `/usr/local/sbin/azcluster-healthcheck`: the v0.16 wrapper that delegates to `/usr/local/bin/azhealthcheck`, and a legacy inline-shell wrapper from a pre-v0.16 prototype. The legacy block executed after the v0.16 block and overwrote it on every boot, so the Rust binary installed by v0.16 was never invoked. The legacy script also hit the exact gotcha `AGENTS.md` warns about — `command -v nvidia-smi` is true on the `microsoft-dsvm:ubuntu-hpc` image even on CPU SKUs, so `nvidia-smi -L` failed on every CPU node and the script drained itself with `Reason=azcluster-healthcheck: nvidia-smi -L failed` every `HealthCheckInterval=300` (5 min). Removed the legacy block entirely; v0.16's wrapper at line 239 is now the sole writer and uses the AGENTS.md-approved gate `nvidia-smi -L 2>/dev/null | grep -qE '^GPU [0-9]+:'`. Live-validated in `paul-azcluster`/`southafricanorth` on 2× `Standard_D8as_v5` — v0.16.0 deploy reproduced the regression (node1 drained at +5 min and +10 min on schedule with the legacy script's reason); v0.16.1 fix applied to the source tree, awaiting next live deploy for full end-to-end re-confirmation.

### Changed
- Workspace version `0.16.0` -> `0.16.1`.
- CLI default `--azcluster-version` bumped to `v0.16.1`.


## [0.16.0] - 2026-05-23

### Added
- **`azhealthcheck` — node health-check binary for Slurm `HealthCheckProgram`.** New crate `crates/azhealthcheck/` (Rust, MIT). Ships as a release artifact (`azhealthcheck-vX.Y.Z-x86_64-linux.tar.gz`) and is installed by `cloud-init/compute.yaml.tmpl` on every compute node at `/usr/local/bin/azhealthcheck`, with a small wrapper at `/usr/local/sbin/azcluster-healthcheck` that supplies the default service list (`slurmd,prometheus,node_exporter` + `dcgm-exporter` on GPU nodes). The Slurm scheduler config (`slurm.conf`) already pointed at this wrapper path (`HealthCheckProgram=/usr/local/sbin/azcluster-healthcheck`, `HealthCheckInterval=300`, `HealthCheckNodeState=ANY,CYCLE`); v0.16 makes that pointer real. Exit codes: `0` (Ok), `1` (Warning), `2` (Error); Slurm drains the node on any non-zero exit.
- **Checks shipped in v0.16** (5; ported from patterns in [`edwardsp/azhealthcheck`](https://github.com/edwardsp/azhealthcheck), MIT):
  - `gpu_count` — sysfs PCI scan (NVIDIA vendor `0x10de`, class `0x0300|0x0302`) vs. `/dev/nvidia[0-9]+` count. Mismatch → Error. Returns Ok on CPU nodes (no GPUs).
  - `gpu_xid` — scans `dmesg` for `NVRM: Xid` events. Fatal XIDs (48/61/62/63/64/74/79/94/95) and uncategorised → Error; soft XIDs (43/45) → Warning.
  - `network` — sysfs scan of `/sys/class/net/*` Ethernet (`type=1`) and InfiniBand (`type=32`) interfaces. `operstate != up` or `carrier != 1` → Error; `carrier_down_count > 0` while up → Warning (link flapped).
  - `kmsg` — `dmesg --level=emerg,alert,crit --since "1 hour ago"`. Any line → Error.
  - `systemd` — `systemctl is-active <svc>` for each configured service. Any `failed` → Error; `inactive`/`activating` → Warning; missing units are silently skipped (lets the GPU-only `dcgm-exporter` slot be absent on CPU nodes).
- **Flags**: `--checks gpu_count,gpu_xid,network,kmsg,systemd` (default: all), `--services <list>` (for the `systemd` check), `--json` (machine-readable output for human debugging), `--sys-root`/`--dev-root` (for unit testing). Unit tests inject fake `dmesg`/`systemctl` output via a `Runner` trait; 14 tests live alongside the implementation.
- Release pipeline (`.github/workflows/release.yml`) now builds `azhealthcheck` on the linux job and uploads `azhealthcheck-vX.Y.Z-x86_64-linux.tar.gz` alongside `azcluster-cli`/`azcluster-server`/`spank_pyxis.so`.

### Changed
- Workspace version `0.15.0` -> `0.16.0`.
- CLI default `--azcluster-version` bumped to `v0.16.0`.

### Deferred to v0.17+
- DCGM-backed GPU checks (`gpu_dcgm`, `gpu_nvlink`) — need either `libdcgm` Rust bindings or a `nvidia-smi -q` shim. The 5 dep-free checks above cover the most common drain triggers (catastrophic XIDs, link-down, kernel critical, failed services, missing GPU device nodes).
- Intrusive active diagnostics (`gpu_diag`) — not appropriate for periodic `HealthCheckProgram` invocation.
- Azure GHR (GPU Health Reporting) integration — start with exit-code-based draining first.


## [0.15.0] - 2026-05-23

### Added
- **`azcluster validate --multi-node`** runs cross-node smoke checks before users hit them: a 2-node `srun -N2 hostname`, a 2-node Pyxis container launch (`srun -N2 --container-image=docker://alpine:latest`), and (when combined with `--gpu`) a bounded 2-node NCCL all-reduce via HPC-X + `/opt/nccl-tests/build/all_reduce_perf` over message sizes 8M..64M (~30 s). The NCCL check is tuned for ND H100 v5 (`NCCL_IB_HCA=mlx5_ib`, `NCCL_TOPO_FILE=/opt/microsoft/ndv5-topo.xml`, all 8 `mlx5_ib*` HCAs in `UCX_NET_DEVICES`) and would catch regressions in the IB-fabric-in-container / PMIx-multi-node class (e.g. v0.13.6 → v0.13.8) at deploy time. Requires ≥2 idle nodes in the target partition.
- **`azcluster validate --partition <name>`** targets a specific Slurm partition for every check (defaults to the cluster default partition).

### Changed
- Workspace version `0.14.0` -> `0.15.0`.
- CLI default `--azcluster-version` bumped to `v0.15.0`.


## [0.14.0] - 2026-05-23

### Changed
- **`azcluster scale` no longer requires `azcluster tunnel`.** The CLI now invokes `az vmss scale --resource-group <rg> --name vmss-<cluster>-<pool> --new-capacity <n>` directly using the operator's existing `az` login, identical to how `deploy`, `delete`, `status`, and `timings` already work. Removes the previous architecture (CLI → reqwest POST → localhost:8443 → ssh local-forward → scheduler:8443 → `azcluster-server` → `az vmss scale`) that required the operator to keep `azcluster tunnel <name>` running in a second shell for the duration of every scale call. The scheduler-side `azcluster-server` daemon still ships and runs (kept as a future hook point for `/v1/healthz` and for the eventual Slurm power-save autoscaling integration); the `/v1/pools/:name/scale` route is removed. Operator now needs `Microsoft.Compute/virtualMachineScaleSets/write` on the resource group (already required for `deploy`/`delete`).
- `azcluster scale` now validates the pool name against `compute_vmss_names` in cluster state and bails with the list of known pools if the pool is unknown, instead of failing at HTTP time.
- Workspace version `0.13.10` -> `0.14.0`.
- CLI default `--azcluster-version` bumped to `v0.14.0`.

### Removed
- `reqwest` dependency from `azcluster-cli` (the scale HTTP POST was its only consumer).
- `ScaleRequest`/`ScaleResponse`/`ErrorBody` types + `scale_pool` handler + `/v1/pools/:name/scale` route from `azcluster-server`.


## [0.13.10] - 2026-05-23

### Documentation
- Strip unqualified bare-metal NCCL bandwidth claims (peak/avg busbw numbers from a single `all_reduce_perf` run) and MFU-vs-theoretical-peak claims (`~54% MFU vs 989 TFLOPS H100 BF16 peak`, `100.07% efficiency`) from forward-facing docs: `README.md` status block + feature matrix row + v0.13.x roadmap bullet, `walkthrough.md` §4 "What good looks like" + §5 container summary, `walkthrough-dgxc.md` Tier-2 results table. azcluster does not currently run a qualified bandwidth-acceptance baseline; treat `NCCL_DEBUG=INFO` signals (`NET/IB ... mlx5_ib*:1/IB/SHARP`, `NVLS multicast support is available`, `NCCL RDMA Plugin v11`) as the pass/fail criterion. Measured DGXC training throughput (167,594 tok/s on 16 GPU / 83,737 tok/s on 8 GPU, 2.001× strong scaling) and the v0.13.8 container all-reduce summary (`avg busbw=434.40 GB/s` on a single run) are retained as observed values without comparison to a theoretical peak. Historical AGENTS.md and CHANGELOG entries left unchanged for the internal debugging record.

### Changed
- Workspace version `0.13.9` -> `0.13.10`.
- CLI default `--azcluster-version` bumped to `v0.13.10`.


## [0.13.9] - 2026-05-23

### Fixed
- **Cross-node `torch.distributed`/Gloo rendezvous now works on multi-node Pyxis jobs.** v0.13.8 fixed NCCL-over-IB inside containers, but `torch.distributed.new_group(backend="gloo")` (used by Megatron-Bridge for CP groups during init) was still failing with `Gloo connectFullMesh ... timed out connecting: SO_ERROR: Connection refused, remote=[127.0.1.1]:20901`. Root cause: the Ubuntu cloud-image default `127.0.1.1 <hostname>` line in `/etc/hosts`. PyTorch/Gloo calls `gethostbyname(hostname)` for the rendezvous advertised address; every remote rank then dials its own loopback. `cloud-init/compute.yaml.tmpl` now writes the eth0 IPv4 (not `127.0.1.1`) for the renamed compute hostname, so cross-node Gloo connectFullMesh resolves to the correct VNIC IP. Live-validated on `paul-azcluster-h100d`: Llama 3.1 8B BF16 trains end-to-end at 16 GPU (2 node, 167,594 tok/s, ~538 MODEL_TFLOP/s/GPU) and 8 GPU (1 node, 83,737 tok/s, ~537 MODEL_TFLOP/s/GPU); strong scaling 8→16 = **2.001× → 100.07% efficiency**.
- **Slurm conf files now have correct permissions out of cloud-init.** `cloud-init`'s `runcmd` stage inherits umask 0077, so any `cat > /etc/slurm/foo.conf <<EOF ... EOF` heredoc produced a 0600 file (only readable by root). `srun`/`sinfo` then run as the submitting non-root user, parse `/etc/slurm/slurm.conf` + `/etc/slurm/plugstack.conf` locally on the submit host (Pyxis spank plugins load at submit time, not at exec time), and bail with `error: s_p_parse_file: unable to read "/etc/slurm/slurm.conf": Permission denied`. `cloud-init/{scheduler,login,compute}.yaml.tmpl` now `chmod 0644` each Slurm conf file (slurm.conf, plugstack.conf, cgroup.conf, gres.conf) immediately after the heredoc.

### Documentation
- `walkthrough-dgxc.md` Tier-2 rewritten around the v25.11 `llmb-install --play` + `llmb-run submit` flow (replaces the previous `./launch.sh` env-var approach); Tier-2 results table added with the live 8 GPU / 16 GPU Llama 3.1 8B BF16 numbers from `paul-azcluster-h100d`. New "Storage sizing" callout warns that `--shared-storage nfs-scheduler` is too small for NeMo `nvcr.io#nvidia/nemo:26.04.00` (~17 GiB squashfs) and recommends ANF (default) or an attached data disk.
- AGENTS.md gains two gotchas: "Slurm conf files cloud-init perms" and "Compute `/etc/hosts` 127.0.1.1 breaks cross-node Gloo/PyTorch rendezvous".

### Changed
- Workspace version `0.13.8` -> `0.13.9`.
- CLI default `--azcluster-version` bumped to `v0.13.9`.


## [0.13.8] - 2026-05-23

### Fixed
- **Cross-node containerised NCCL now uses InfiniBand end-to-end on NDv5 H100.** v0.13.7 opened `/dev/infiniband/*` perms to `0666` (still needed and retained) but was insufficient on its own: enroot's default `/dev` handling does NOT bind-mount `/dev/infiniband` into containers, so NCCL inside Pyxis still logged `NET/IB : No device found.` and fell back to OOB ethernet. v0.13.8 sets `MELLANOX_VISIBLE_DEVICES=all` in `/etc/enroot/environ.d/50-nccl.env`, which triggers enroot's shipped `/etc/enroot/hooks.d/99-mellanox.sh` hook to bind-mount `/dev/infiniband/{uverbs,umad,issm}*` + `/dev/infiniband/rdma_cm` (and matching `/sys/class/infiniband*` entries) into every Pyxis container. The hook discovers all `mlx?_core` HCAs on the host and binds them based on `MELLANOX_VISIBLE_DEVICES` (parity with `NVIDIA_VISIBLE_DEVICES`). Live-validated on `paul-azcluster-h100d` (2× ND96isr_H100_v5): the multinode NeMo all-reduce smoke (16 ranks, `nvcr.io/nvidia/nemo:25.07.02`, `srun --mpi=pmix`) now logs `NET/IB : Using [0]mlx5_ib0:1/IB/SHARP ... [7]mlx5_ib7:1/IB/SHARP` on every rank and reaches `avg busbw=434.40 GB/s` at 1 GiB (SHARP path), up from TCP-fallback levels in v0.13.7.

### Changed
- Workspace version `0.13.7` -> `0.13.8`.
- CLI default `--azcluster-version` bumped to `v0.13.8`.
- Updated the `cloud-init/compute.yaml.tmpl` comment block above the udev rule to clarify it complements (rather than replaces) the enroot mellanox hook: the hook does the bind-mount, the udev rule keeps the in-container UID (mapped through `ENROOT_REMAP_ROOT`) able to open the bound devices.
- AGENTS.md IB-visibility gotcha updated: the operative fix is `MELLANOX_VISIBLE_DEVICES=all` + the enroot `99-mellanox.sh` hook; the v0.13.7 udev rule is necessary but not sufficient.


## [0.13.7] - 2026-05-23

### Fixed
- **NCCL inside Pyxis containers now uses InfiniBand on NDv5 H100.** Cloud-init on every GPU compute node writes `/etc/udev/rules.d/91-azcluster-ib-perms.rules` setting `MODE="0666"` on `uverbs*`, `rdma_cm`, `ucm*`, `umad*`, `issm*`, and immediately `chmod 0666`s the existing device nodes plus runs `udevadm trigger`. The earlier default of `0660 root:root` interacted with `ENROOT_REMAP_ROOT yes` (added in v0.13.5 for DGXC compat): the in-container "root" maps to a host non-root uid and could not open `/dev/infiniband/uverbs*`, so NCCL logged `NET/IB : No device found.` and silently fell back to `OOB eth0:10.42.4.x` (single-NIC TCP) instead of the 8× NDR400 IB fabric. With permissive device modes, NCCL inside Pyxis containers picks up `mlx5_ib0..7` and uses the IB fabric directly.


## [0.13.6] - 2026-05-22

### Added
- **Cross-node containerised MPI now works (CCWS-style runtime fix).** Slurm 25.11 + Pyxis + Enroot can now launch a single MPI world across multiple Pyxis containers with `srun --mpi=pmix --container-image=...`. Two cooperating pieces, both shipped in `cloud-init/compute.yaml.tmpl`:
  - **slurmd `EnvironmentFile`** (`/etc/default/slurmd`) now exports `PMIX_MCA_ptl=^usock`, `PMIX_MCA_psec=none`, `PMIX_SYSTEM_TMPDIR=/var/empty`, `PMIX_MCA_gds=hash`, `HWLOC_COMPONENTS=-opencl`. Pins the PMIx server transport / security / GDS modules so all ranks negotiate the same channel regardless of host autodetect.
  - **Enroot PMI hooks** at `/etc/enroot/hooks.d/50-slurm-pmi.sh` and `50-slurm-pytorch.sh` (upstream NVIDIA Enroot, Apache 2.0, pinned in-tree). `50-slurm-pmi.sh` copies all `PMIX_*` and `SLURM_*` env into the container's `${ENROOT_ENVIRON}` and bind-mounts `$PMIX_SERVER_TMPDIR` into the container via `${ENROOT_MOUNTS}`. `50-slurm-pytorch.sh` derives `MASTER_ADDR` / `MASTER_PORT` / `RANK` / `LOCAL_RANK` / `WORLD_SIZE` from `SLURM_*` for any container exposing `PYTORCH_VERSION` (NeMo, NGC PyTorch, Megatron, etc.).
- **`/shared/examples/dgxc-nemo-multinode-smoke.sbatch`** — 2-node × 8-GPU = 16-rank NCCL all-reduce inside `nvcr.io/nvidia/nemo:25.07.02`, exercising the v0.13.6 cross-node containerised path end-to-end.

### Changed
- Workspace version `0.13.5` -> `0.13.6`.
- CLI default `--azcluster-version` bumped to `v0.13.6`.
- Removed the "cross-node Pyxis container = broken" caveats from `nccl-allreduce.sbatch` and `dgxc-nemo-container-smoke.sbatch` comments now that the multi-node container path is supported.
- AGENTS.md "PMIx 4 vs 5 ABI" gotcha replaced with the corrected "Cross-node containerised MPI via Pyxis needs slurmd PMIx env + enroot PMI hooks" entry. The earlier ABI-incompatibility framing was a misdiagnosis: NGC PyTorch/NeMo containers ship HPC-X 2.20-2.21 → PMIx 4.2.x (matching the host's `mpi_pmix_v4.so`). The actual failure mode was missing `PMIX_MCA_*` env on slurmd and missing PMI propagation into containers — both of which the CCWS pattern fixes without rebuilding any package.

### Verified
- **NGC container PMIx version audit (v0.13.6 decision).** Comprehensive research across NVIDIA NGC training containers (PyTorch 24.10-25.05, NeMo 25.07, TensorFlow 24.05) and HPC-X versions 2.18-2.26 confirms: all major NGC training containers from 2024-2025 ship HPC-X 2.20-2.25, all bundling PMIx 4.2.x. The `ghcr.io/azure/ai-infrastructure-on-azure/nccl-test:latest` image uses HPC-X 2.26, which also bundles PMIx 4.2.9. No PMIx 5 found in production NGC containers as of May 2026. Conclusion: azcluster v0.13.6 ships only `mpi_pmix_v4.so` (no PMIx 5 rebuild required). Evidence: NVIDIA HPC-X release notes, Azure HPC image specifications, NGC container release notes. See AGENTS.md "Cross-node containerised MPI via Pyxis" section for implementation details.


## [0.13.5] - 2026-05-22

### Added
- **Automatic NVMe RAID-0 ephemeral scratch on SKUs with `Microsoft NVMe Direct Disk(s)`.** ND96isr_H100_v5 ships 8× ~3.5 TB raw NVMe (28 TB total) which the marketplace image leaves unpartitioned. Compute bootstrap now detects them via `lsblk -d -n -p -o NAME,MODEL` (regex `Microsoft NVMe Direct Disk( v2)?`, case-insensitive), wipes filesystem signatures, builds `/dev/md/azcluster_nvme` as RAID-0 (chunk=128, metadata=1.2, ext4 with label `azcluster_nvme`), mounts at `/mnt/nvme` with `nofail,x-systemd.device-timeout=10`, and persists via `/etc/mdadm/mdadm.conf` + `/etc/fstab`. Survives reboots. Lost on deallocation (ephemeral by design). Falls through silently on SKUs without NVMe Direct Disks.
- **Enroot extraction relocated to `/mnt/nvme` when present.** Container imports (e.g. the ~20 GB NeMo container) now extract onto NVMe RAID-0 in seconds rather than minutes on the SCSI resource disk or root. Scratch precedence: `/mnt/nvme` > `/mnt` > `/var/lib`. Both `/var/lib/enroot` and `/var/lib/enroot-data` are symlinked to the chosen base.
- **DGXC (NVIDIA dgxc-benchmarking) compatibility baked into compute nodes.**
  - `/etc/enroot/enroot.conf` now sets `ENROOT_REMAP_ROOT yes` (in addition to existing `ENROOT_ROOTFS_WRITABLE yes`), matching DGXC and most NVIDIA NGC container expectations.
  - `/etc/enroot/environ.d/50-nccl.env` written so NCCL/UCX env vars propagate INTO Pyxis containers (Enroot environ.d runs on container start; `/etc/profile.d/` does not because non-login shells skip it).
  - `/etc/enroot/mounts.d/50-azcluster.fstab` bind-mounts `/opt/microsoft` (containing `ndv5-topo.xml`) into every container read-only, so `NCCL_TOPO_FILE` resolves inside the container.
- **`/shared/examples/dgxc-nemo-container-smoke.sbatch`** — self-contained 1-node × 8-GPU NCCL all-reduce smoke test using `nvcr.io/nvidia/nemo:25.07.02` (20 GB image). No NGC credentials required. Validates the full Pyxis → NVMe → NCCL-in-container path (Enroot environ.d propagation, mounts.d topology bind, IBext_v11 over `mlx5_ib0..7`). Uses plain `torch.distributed.all_reduce` to avoid NeMo recipe API churn between container versions. The full Llama 3.1 8B (and larger) training path is documented in `walkthrough-dgxc.md` via NVIDIA's `llmb-run` driver.
- **`walkthrough-dgxc.md`** — end-to-end DGXC guide: infra smoke test sbatch, full `llmb-install` flow with NGC credentials, multi-node PMIx 4↔5 limitations and workarounds.

### Fixed
- **`AccountingStorageTRES=gres/gpu` was emitted unconditionally in `scheduler.yaml.tmpl`**, causing slurmctld to abort with `fatal: slurmdbd is required to run with TRES gres/gpu` when deploying a GPU pool with `--no-accounting`. Moved the line inside the `ENABLE_ACCOUNTING=true` block. Caught by the v0.13.5 live test on `paul-azcluster-h100b`.
- **`/shared/examples` was unreadable by `azureuser`** because the scheduler bootstrap ran `chown -R "${AZCLUSTER_NAME:-azureuser}":users` which uses the *cluster name* as the username and the non-existent `users` group, both silently failing under `|| true`. Result: NFSv4.1 anonymous mapping left the directory `nobody:nogroup 0700`. Replaced with explicit `chmod 0755 dir; chmod 0644 files; chown -R azureuser:azureuser dir`. Caught by the v0.13.5 live test.

### Changed
- Workspace version `0.13.4` -> `0.13.5`.
- CLI default `--azcluster-version` bumped to `v0.13.5`.

### Verified
- 1-node `Standard_ND96isr_H100_v5` (`paul-azcluster-h100b`, `southafricanorth`) — ARM deploy 699s (~11.6 min) including NVMe RAID-0 of 8x ~3.5 TB disks into 28 TB `/mnt/nvme`. Pyxis import of `nvcr.io/nvidia/nemo:25.07.02` (20 GB) onto NVMe completed in seconds. `dgxc-nemo-container-smoke.sbatch` (8x H100 NVLink all-reduce, 1 GiB fp16, 20 iters) completed in **0.081s elapsed, algbw 266.53 GB/s, avg busbw 466.42 GB/s** with NCCL RDMA Plugin v10 / IBext_v10 loaded and `NCCL_IB_HCA=mlx5_ib` correctly injected from `/etc/enroot/environ.d/50-nccl.env` (proves Enroot environ.d propagation into Pyxis containers). slurmctld `active` under `--no-accounting` (proves TRES gating fix). `/shared/examples` owned `azureuser:azureuser 0755` with files `0644` (proves perms fix).

## [0.13.4] - 2026-05-22

### Fixed
- **GPU compute nodes registered with the wrong Gres name and never joined the GPU partition.** The compute bootstrap parsed `nvidia-smi --query-gpu=name` with `tolower($NF)`, which on an H100 returned `hbm3` (the last token of "NVIDIA H100 80GB HBM3") rather than `h100`. Slurm then registered `Gres=gpu:hbm3:8` while the scheduler had `GresTypes=gpu` and `--gres=gpu:h100:N` job requests, so every GPU job stayed PENDING with `Resources` reason. Parse the first `<letter><digits>` token instead (`h100`, `h200`, `a100`, ...), and emit an extra `Feature=` tag for the HBM tier so jobs can target it explicitly when wanted.
- **Compute nodes overrode `CPUs=` on the slurmd command line, defeating `Parameters=l3cache_as_socket`.** On NDv5 (96 cores, 4 NUMA, large L3 cache) the default Slurm S:C:T detection bunches every core into one socket, which interacts badly with NCCL's GPU↔NIC affinity heuristics. Dropped the explicit `CPUs=${CPUS}` from `SLURMD_OPTIONS` and added `Parameters=l3cache_as_socket` so slurmd autodetects 8 sockets matching the 8 L3 cache groups, one per GPU.
- **Configless Slurm never distributed `gres.conf`, so slurmctld refused to validate `GresTypes=gpu`.** `enable_configless` only distributes `slurm.conf` (not `gres.conf`, `cgroup.conf`, or `plugstack.conf`). Scheduler bootstrap now writes `/etc/slurm/gres.conf` containing `AutoDetect=nvml` so slurmctld can parse the Gres stanza locally; compute bootstrap writes the same file locally so each slurmd discovers its actual `/dev/nvidia*` devices via NVML.
- **Enroot container imports filled the 64 GB root disk on H100 nodes.** The marketplace `microsoft-dsvm:ubuntu-hpc:2404` image leaves only ~7 GB free on `/` after base install. Importing a ~10 GB CUDA container via Pyxis (e.g. the `ghcr.io/azure/ai-infrastructure-on-azure/nccl-test:latest` image used for multi-node NCCL) ran out of space mid-extraction. Compute bootstrap now relocates `/var/lib/enroot` and `/var/lib/enroot-data` to symlinks under `/mnt`, the Azure ephemeral disk (~956 GB NVMe-backed on NDv5), when `/mnt` is mounted. Falls back to `/var/lib/enroot` for SKUs without an ephemeral disk.
- **`scheduler.yaml.tmpl` was missing `GresTypes=gpu` and `AccountingStorageTRES=gres/gpu`.** Without these, slurmctld silently dropped any `Gres=gpu:...` registration from compute nodes and `sacct --format=AllocTRES` could not include GPU usage. Now emitted unconditionally — harmless on CPU-only clusters because `GresTypes=gpu` alone does not require any node to have a GPU.
- **`compute.yaml.tmpl` `ethtool` pipe aborted under `set -euo pipefail` on virtual interfaces.** `drv=$(ethtool -i lo ... | awk ...)` exits non-zero because the awk pattern matches nothing, killing the whole bootstrap before slurmd installs. Added `|| true` inside the command substitution.

### Changed
- `NCCL_IB_HCA` default in `/etc/profile.d/nccl-azcluster.sh` changed from `mlx5` to `mlx5_ib` to match the device prefix actually present on NDv5 NDR400-IB cards (`mlx5_ib0`-`mlx5_ib7`). The previous value also matched IPoE interfaces and confused NCCL's HCA selection.
- Replaced the multi-node NCCL all-reduce example sbatch (`/shared/examples/nccl-allreduce.sbatch`). The old template tried to run `all_reduce_perf` inside an `nvcr.io/nvidia/pytorch:24.10-py3` container over Pyxis, but cross-node MPI inside that container fails on this image because the container's PMIx (5.x) does not match Slurm 25.11's `mpi_pmix_v4.so` (PMIx 4.x ABI). New example runs bare-metal using HPC-X (already in the image, PMIx 4.x compatible) + the prebuilt `/opt/nccl-tests/build/all_reduce_perf`. Documented the Pyxis caveat inline.
- Workspace version `0.13.3` -> `0.13.4`.
- CLI default `--azcluster-version` bumped to `v0.13.4`.

### Added
- `walkthrough.md` — end-to-end recipe: deploy a 2-node NDv5 H100 cluster, run the NCCL all-reduce, interpret the results, and tear down. Covers the bare-metal HPC-X path and the (currently broken) Pyxis container path.

### Verified
- 2-node `Standard_ND96isr_H100_v5` (`paul-azcluster-h100a`, `southafricanorth`) — bare-metal HPC-X NCCL all-reduce across 16 GPUs / 8x NDR400 InfiniBand achieved **466.33 GB/s peak / 348.02 GB/s avg busbw** (16 GiB message size; full size sweep from 8 MiB upward). `NCCL_DEBUG=INFO` confirmed NVLS multicast, `IBext_v11` P2P plugin, HPC-X `nccl_rdma_sharp_plugin`, and IB/SHARP on all 8 `mlx5_ib*` NICs. Pyxis container pull also validated (`ghcr.io#azure/ai-infrastructure-on-azure/nccl-test:latest`, 9.4 GB image imported on both nodes); cross-node container runs reported `busbw=0` due to the PMIx 4/5 ABI mismatch noted in `AGENTS.md`.

## [0.13.3] - 2026-05-22

### Fixed
- **Accounting refused all job submissions.** `AccountingStorageEnforce=associations,limits,qos` requires every `(user, account, cluster)` tuple to be registered with slurmdbd before submission. The bootstrap registered the cluster but never created an account or associated `azureuser` with it, so every `srun`/`sbatch` failed with `Invalid account or account/partition combination specified`. Seed a `default` account and add `azureuser` with it as the default account immediately after `sacctmgr add cluster`. Future LDAP/Entra integration will replace the per-user step.
- **`sacct`/`sinfo` accounting calls from the login VM hit `localhost:6819`.** `AccountingStorageHost=localhost` resolves on the scheduler but is wrong for every other node fetching `slurm.conf` via `slurmd --conf-server`. Set it to `${AZCLUSTER_NAME}-scheduler` so all clients reach the colocated `slurmdbd` on the scheduler VM.

### Verified
- End-to-end accounting smoke test on `acct3` (1× `Standard_D8as_v5`, `southafricanorth`): `azcluster validate` green; `sacct` on scheduler shows two completed jobs (`hostname` + Pyxis `srun --container-image=docker://alpine`) with `Account=default User=azureuser Cluster=acct3 State=COMPLETED`. The accounting backend (Azure DB for MySQL Flexible Server + `slurmdbd`) is now live-validated.

## [0.13.2] - 2026-05-22

### Fixed
- **Accounting bootstrap failed at TLS CA download.** `https://dl.cacerts.digicert.com/DigiCertGlobalRootCA.crt.pem` serves a cert whose SAN does not match the hostname, so `curl` aborts with `SSL: no alternative certificate subject name matches target host name` and `set -euo pipefail` kills the scheduler bootstrap before `slurmdbd.conf` is written. Ubuntu's `ca-certificates` package already includes the DigiCert Global Root CA used by Azure MySQL Flex, so point `StorageParameters=SSL_CA=/etc/ssl/certs/ca-certificates.crt` at the system bundle and drop the download entirely.

## [0.13.1] - 2026-05-22

### Fixed
- **Scheduler bootstrap aborted before `slurmdbd` started.** `curl -fsSL https://aka.ms/InstallAzureCLIDeb | bash` invokes `apt-get install` internally without our `DPkg::Lock::Timeout=600` and raced `apt-daily`/`unattended-upgrades`, dying with `Could not get lock /var/lib/dpkg/lock-frontend`. Under `set -euo pipefail`, the script aborted there, so the accounting block (which runs later) never wrote `/etc/slurm/slurmdbd.conf`, never started `slurmdbd`, and `slurm.conf` never gained its `AccountingStorage*` stanza. The extra apt work added in v0.13.0 (`slurm-smd-slurmdbd` + `mariadb-client`) widened the race window and exposed this latent bug. Replaced the curl-pipe with an explicit `apt-get install azure-cli` from the Microsoft `packages.microsoft.com/repos/azure-cli/` source, using our `DPkg::Lock::Timeout=600` flag.

## [0.13.0] - 2026-05-22

### Added
- **Slurm accounting backend (Azure Database for MySQL Flexible Server + `slurmdbd`).** `--accounting` (default on) provisions a `Standard_B2ms` MySQL Flexible Server (`mysql-<cluster>`, MySQL 8.0.21, 50 GB autogrow, public network disabled, VNet-integrated) and a `slurm_acct_db` database in a new delegated `database` subnet (`10.42.8.0/29`). The scheduler cloud-init installs `slurm-smd-slurmdbd` + `mariadb-client`, fetches the DigiCert Global Root CA, writes `/etc/slurm/slurmdbd.conf` (mode 0600, owned by `slurm:slurm`) with TLS enabled (`StorageParameters=SSL_CA=…`), waits for `:3306` to be reachable, starts `slurmdbd` before `slurmctld`, and registers the cluster with `sacctmgr -i add cluster`. `slurm.conf` now emits `AccountingStorageType=accounting_storage/slurmdbd`, `AccountingStorageEnforce=associations,limits,qos`, and `JobAcctGatherType=jobacct_gather/cgroup` whenever accounting is on. Pass `--no-accounting` to skip the entire MySQL + slurmdbd path for rapid test deploys.
- **`bicep/modules/accounting.bicep`** — MySQL Flexible Server + database + three slurmdbd-recommended server parameters (`innodb_lock_wait_timeout=900`, `max_allowed_packet=16M`, `log_bin_trust_function_creators=ON`).
- **Auto-generated MySQL admin password.** CLI reads 32 bytes from `/dev/urandom`, alphabet-encodes to an ambiguity-free 32-char body, appends `Aa1!` to satisfy Azure MySQL Flex's four-character-class complexity policy, and threads it through as a secure Bicep parameter. The password lands on the scheduler only via the encrypted `customData` channel (`/etc/azcluster/accounting.password`, mode 0600 root:root) and is read into `slurmdbd.conf` then `unset` in the bootstrap shell.
- **`database` subnet** (`10.42.8.0/29`) added to `bicep/modules/network.bicep`, delegated to `Microsoft.DBforMySQL/flexibleServers`. The existing `nsg-<cluster>-internal` `allow-vnet-inbound` rule already covers scheduler → MySQL :3306 traffic.

### Changed
- `enableAccounting` is no longer a tag-only flag; it now provisions real infrastructure when true and toggles the accounting branches in `cloud-init/scheduler.yaml.tmpl`.
- Workspace version 0.12.1 -> 0.13.0.
- CLI default `--azcluster-version` bumped to `v0.13.0`.

## [0.12.1] - 2026-05-22

### Fixed
- **`--shared-storage nfs-scheduler` scheduler bootstrap.** The previous template ran a dedicated `apt-get update && apt-get install nfs-kernel-server` *before* the slurm install. The second `apt-get update` (for the slurm repo) raced against `apt-daily`/`unattended-upgrades` taking the `/var/lib/apt/lists/lock`; `DPkg::Lock::Timeout` does not cover the lists lock, so the script exited (`set -euo pipefail`), `slurmctld` never installed, and login/compute hung forever waiting for `${SCHED_DIR}/munge.key`. Fold `nfs-kernel-server` into the single slurm `apt-get install` call and run the `exports`/`systemctl enable --now nfs-server` step afterward.

### Changed
- `azcluster timings` capture now reads each nested module's resource group directly from the sub-deployment operation's `properties.targetResource.resourceGroup` instead of issuing a separate `az deployment group list` lookup per module. Eliminates ~N extra `az` calls per deploy and a class of failure when a module name doesn't round-trip through the `--query` filter. Output is also sorted and deduped before being written to the snapshot.

## [0.12.0] - 2026-05-22

### Added
- **`--shared-storage` flag** (default `anf`). New `nfs-scheduler` mode skips Azure NetApp Files entirely and exports `/shared` from the scheduler VM via `nfs-kernel-server`, shaving ~12 minutes off provisioning time. Login + compute nodes mount the scheduler's export with a retry loop so they survive races with the scheduler's NFS service coming up. Test-only: no HA, scheduler is a SPOF for shared storage.
- **`--no-monitoring` / `--no-accounting` toggles** for rapid iteration on features that don't depend on observability or accounting. Defaults remain ON; pass `--no-monitoring` (or `--no-accounting`) to skip provisioning during test deploys. `--accounting` is currently reserved for v0.13.x.
- **Per-deploy timing capture.** After a successful `azcluster deploy`, the CLI recurses `az deployment operation sub list` and `az deployment operation group list` for every nested module, computes per-resource durations from ISO-8601 `properties.duration`, and writes a JSON snapshot to `~/.config/azcluster/deployments/<cluster>/<utc-stamp>.json`. A `trend.tsv` is appended in the same directory for cross-run comparison.
- **`azcluster timings <cluster> [--last N] [--trend]` subcommand.** Prints a sorted table (largest durations first) for the last N deployments, or dumps the trend TSV.
- New `timings` Rust module (`crates/azcluster-cli/src/timings.rs`) with an ISO-8601 duration parser and a self-contained epoch-to-UTC formatter (no `chrono` dependency).

### Changed
- Cloud-init template placeholders renamed `{{ANF_MOUNT_IP}}` → `{{SHARED_MOUNT_IP}}` and `{{ANF_EXPORT_PATH}}` → `{{SHARED_EXPORT_PATH}}` to reflect that the source can now be ANF or the scheduler. Bicep scheduler/login/compute module params renamed `anfMountIp`/`anfExportPath` → `sharedMountIp`/`sharedExportPath`.
- Bicep `main.bicep` and `cluster.bicep` accept `sharedStorageMode` (`anf` | `nfs-scheduler`) and `enableAccounting`. The `anf` module is now conditional (`if (sharedStorageMode == 'anf')`).
- Workspace version 0.11.4 -> 0.12.0.
- CLI default `--azcluster-version` bumped to `v0.12.0`.

### Fixed
- Login and compute `/shared` mount now retries for 5 minutes instead of single-shot, which is needed for `nfs-scheduler` mode (where the scheduler is still installing `nfs-kernel-server` when the other VMs hit cloud-init) and harmless for `anf` mode.

## [0.11.4] - 2026-05-22

### Fixed
- **Compute cloud-init syntax error.** The NCCL heredoc terminator (`EOF`) in `cloud-init/compute.yaml.tmpl` lived inside the GPU `if` block at column 8 (2 extra spaces of indent). After YAML stripped the 6-space block-scalar indent, the terminator landed at column 2 instead of column 0, so bash never closed the heredoc. The downstream script then crashed with `syntax error: unexpected end of file`, and slurmd / prometheus / node_exporter never started on compute. Dedented the closer to column 6 so it lands at column 0 in the materialised script. Caught live: v0.11.1 deploy had `up{role="login"}` and `up{role="scheduler"}` ingesting cleanly, but `up{role="compute"}` was absent and `sinfo` showed zero nodes.
- **Grafana Admin RBAC for dashboard import.** v0.11.3 added CLI post-deploy dashboard import via `az grafana dashboard create`, but the deployer principal had no Grafana role on the AMG instance, so the API returned `401 NoRoleAssignedException`. Added a conditional role assignment in `bicep/modules/monitoring.bicep` that grants Grafana Admin (`22926164-76b3-42b3-bc55-97df8dab3e41`) to the deployer principal on the AMG resource scope when `--monitoring` is enabled.

### Added
- CLI resolves the current deployer (`az ad signed-in-user show` for users, `az ad sp show --id <upn>` for service principals) and threads `deployerPrincipalId` + `deployerPrincipalType` into the Bicep deployment whenever `--monitoring` is set.
- `import_dashboards` retries up to 10 times with 30s back-off when AMG returns 401 / `NoRoleAssignedException`, so the post-deploy import survives the typical 1-3 min role propagation window.

### Changed
- Workspace version 0.11.3 -> 0.11.4.
- CLI default `--azcluster-version` bumped to `v0.11.4`.

## [0.11.3] - 2026-05-22

### Fixed
- **Grafana dashboard provisioning.** v0.11.2 used `Microsoft.Dashboard/grafana/dashboards@2024-10-01`, which is not a real ARM resource type (Bicep emitted BCP081, ARM preflight rejected the deployment with `ResourceTypeRegistrationNotFound`). Reverted the `grafana-dashboards.bicep` module and the `dashboards` block in `monitoring.bicep`. Dashboard JSONs are unchanged.

### Changed
- Dashboards are now imported post-deploy by the CLI via `az grafana dashboard create --overwrite true`. The three JSONs in `grafana/dashboards/` are embedded into the CLI binary via `include_str!`, wrapped in the `{dashboard, overwrite, folderId}` envelope, and pushed when `--monitoring` is set. `monitoring.bicep` now exports `grafanaName` for the CLI to target.
- Workspace version 0.11.2 -> 0.11.3.
- CLI default `--azcluster-version` bumped to `v0.11.3`.

## [0.11.2] - 2026-05-22

### Added
- **Phase 3 observability dashboards complete.** Added three Azure Managed Grafana dashboards for the AMW Managed Prometheus datasource: node health, Slurm scheduler, and GPU + InfiniBand.
- `grafana/dashboards/node.json` covers per-role and per-instance CPU, load, memory, network throughput, filesystem usage for `/`, `/shared`, and `/amlfs`, and file descriptor pressure.
- `grafana/dashboards/slurm.json` covers Slurm CPU and node totals, idle/allocated/down/drain/mixed states, pending vs running jobs, exporter scrape duration, and partition breakdowns.
- `grafana/dashboards/gpu_ib.json` covers DCGM GPU utilization, framebuffer memory, SM clocks, power, remapped-row errors, NVLink bandwidth, and node_exporter InfiniBand port receive/transmit throughput.
- `bicep/modules/grafana-dashboards.bicep` provisions the three dashboards as AMG child resources using the generated AMW Prometheus datasource variable instead of hardcoding datasource UIDs.

### Changed
- Workspace version 0.11.1 -> 0.11.2.
- CLI default `--azcluster-version` bumped to `v0.11.2`.
- `bicep/modules/monitoring.bicep` now invokes `grafana-dashboards.bicep` after AMG provisioning so dashboards land alongside the AMG instance when `--monitoring` is set, and exports a `grafanaDashboardIds` array output.

## [0.11.1] - 2026-05-22

### Added
- **Per-VM Prometheus on login + compute** with `remote_write` to AMW, mirroring the v0.11.0 scheduler path. Login scrapes its local `node_exporter` (`:9100`); compute scrapes `node_exporter` and, when GPUs are present, `dcgm-exporter` (`:9400`). External labels include `role` (login / compute) and, for compute, `pool`.
- **monUai attachment on login VM and compute VMSS Flex.** Login adds the monitoring UAI alongside its existing SystemAssigned identity. Compute attaches it `UserAssigned`-only - AzSecPack rejects `SystemAssigned, UserAssigned` on VMSS Flex in tenants with the UAI-only policy.

### Changed
- Workspace version 0.11.0 -> 0.11.1.
- CLI default `--azcluster-version` bumped to `v0.11.1`.
- `bicep/cluster.bicep` now threads `monUaiId` / `monUaiClientId` / `amwIngestionEndpoint` into `login.bicep` and the `compute.bicep` for-loop in addition to the scheduler.
- `bicep/modules/compute.bicep` resource definition now conditionally emits `identity: { type: 'UserAssigned', userAssignedIdentities: { ... } }` when monitoring is enabled, otherwise `identity: null`.

### Deferred Validation
- The v0.11.0 scheduler path was live-validated end-to-end (AMW returns `up{role="scheduler"}=1` for `node_exporter` and `slurm_exporter`). The v0.11.1 login + compute path replicates that exact mechanism (same install steps, same `azuread.managed_identity` remote_write block); Bicep modules compile clean. End-to-end live validation on a fresh cluster is gated on the in-progress `paul-azcluster` RG teardown completing; will be folded into the v0.11.2 deploy.

## [0.11.0] - 2026-05-22

### Added
- **Per-VM Prometheus on the scheduler** with `remote_write` to Azure Monitor Workspace, using Prometheus's native `azuread.managed_identity` authentication (no AMA, no DCR custom scrape). Prometheus 3.3.0 binary installed to `/opt/prometheus`, data under `/mnt/prometheus/data`, listens on `127.0.0.1:9090`, scrapes the local `node_exporter` (`:9100`) and `prometheus-slurm-exporter` (`:8081`).
- **Shared monitoring UAI** (`uai-${clusterName}-mon`) created in `monitoring.bicep` and attached to the scheduler VM alongside the existing scheduler UAI. Granted `Monitoring Metrics Publisher` (`3913510d-...`) on the AMW's auto-created default Data Collection Rule (the actual ingestion gate - role on the AMW itself is insufficient).
- **`bicep/modules/ingestion-endpoint.bicep`** sub-module, deployed at the Azure-managed sister RG scope (`MA_<amwName>_<location>_managed`), resolves the DCE metrics ingestion endpoint + DCR immutable id and creates the cross-RG role assignment.

### Changed
- Workspace version 0.10.1 -> 0.11.0.
- CLI default `--azcluster-version` bumped to `v0.11.0`.
- `monitoring.bicep` no longer takes scheduler/login VM names; it just provisions AMW + monUai + AMG + cross-RG ingestion role + role assignment. Per-VM identity attachment is now done by the consuming VM module (scheduler today; login + compute in v0.11.1).
- `cluster.bicep` now invokes the monitoring module **before** the scheduler module and threads `monUaiId`, `monUaiClientId`, and `amwIngestionEndpoint` into it.
- Removed dead `raScheduler` / `raLogin` role assignments (SystemAssigned MI no longer used for metrics publishing).

### Fixed
- `cat >` in cloud-init creates files with mode `0600` under the cloud-init umask; explicit `chmod 0644 /opt/prometheus/prometheus.yml` so the non-root `prometheus` user can read its config.

### Validated
- Live deploy in `southafricanorth` (RG `paul-azcluster`). After RBAC propagation (~6 min on a brand-new MI + DCR), AMW query endpoint returns `up{job="node_exporter",role="scheduler"} 1` and `up{job="slurm_exporter",role="scheduler"} 1` with `cluster="paul-azcluster/mon"`, `microsoft.amwresourceid=/subscriptions/.../accounts/amw-mon`. Direct `POST` to the ingestion URL with an IMDS-issued token for the monUai also returns `HTTP 200`.

### Deferred
- Per-VM Prometheus on login and compute nodes (v0.11.1). Compute will need `monUaiId` threaded through `compute.bicep` and attached as `UserAssigned` on the VMSS (AzSecPack blocks `SystemAssigned, UserAssigned` on VMSS Flex in tenants with the UAI-only policy).
- Grafana dashboards for node / GPU / InfiniBand panels (v0.11.2).
- Slurm accounting via Azure Database for MySQL Flexible Server + `slurmdbd` (v0.12.x).

## [0.10.1] - 2026-05-22

### Added
- **rivosinc/prometheus-slurm-exporter v1.8.0** installed via `.deb` on the scheduler. Runs as `slurm` user under `prometheus-slurm-exporter.service` systemd unit, binds `127.0.0.1:8081`. Exposes `slurm_cpus_total`, `slurm_node_count_per_state`, `slurm_job_scrape_duration`, etc. Will be scraped locally once the AMW scrape path lands in v0.11.0.

### Changed
- Workspace version 0.10.0 -> 0.10.1.
- CLI default `--azcluster-version` bumped to `v0.10.1`.

### Validated
- Live deploy in `southafricanorth`: `prometheus-slurm-exporter.service` active on scheduler, `/metrics` returns valid Prometheus output (`slurm_cpus_total`, `slurm_node_count_per_state{state="n/a"} 1`, `slurm_job_scrape_duration 4`, ...). `node_exporter` + `slurmctld` also active alongside.

## [0.10.0] - 2026-05-22

### Added
- **Prometheus node_exporter v1.8.2** installed via cloud-init on scheduler, login, and every compute node. Binds `127.0.0.1:9100`, runs as dedicated `node_exporter` system user under a `node_exporter.service` systemd unit. No public exposure; metrics will be scraped locally once the AMW scrape path lands in v0.11.0.
- **NVIDIA DCGM exporter (`nvcr.io/nvidia/k8s/dcgm-exporter:3.3.7-3.4.1-ubuntu22.04`)** auto-started on compute nodes that report GPUs via `nvidia-smi -L | grep -cE '^GPU [0-9]+:'`. Runs as a docker container with `--gpus all --cap-add SYS_ADMIN`, publishing `127.0.0.1:9400`. Install is no-op on CPU-only pools.

### Changed
- Workspace version 0.9.1 -> 0.10.0.
- CLI default `--azcluster-version` bumped to `v0.10.0`.

### Validated
- Live deploy in `southafricanorth` (RG `paul-azcluster`) with `--monitoring --grafana-location uksouth --login-public-ip --pool name=cpu,sku=Standard_D8as_v5,count=0,default`. `node_exporter` active on both scheduler and login; `/metrics` returns valid Prometheus payload (`node_boot_time_seconds`, `node_cpu_seconds_total{...}` observed). DCGM exporter install path exercised in cloud-init template (compile-time only - no GPU node available in this region for runtime validation; deferred to next region with H100/H200 capacity).

### Deferred
- Prometheus scrape path to AMW (DCR `customVMScrapeConfig` via AMA, or local prometheus on scheduler with `remote_write`). Slated for v0.11.0.
- `prometheus-slurm-exporter` on scheduler (separate validation cycle, v0.10.1).
- Compute-VMSS `Monitoring Metrics Publisher` role assignment via per-pool UAI; restore `raCompute` in `monitoring.bicep`.

## [0.9.1] - 2026-05-21

### Fixed
- **Monitoring Data Reader role GUID**: v0.9.0 used `b0d8363b-78d5-41c0-9c38-6abe57b51537`, which does not exist (`RoleDefinitionDoesNotExist`). Correct GUID is `b0d8363b-8ddd-447d-831f-62ca05bff136` (looked up via `az role definition list --name "Monitoring Data Reader"`). The AMG → AMW role assignment now provisions.
- **VMSS Flex SystemAssigned identity rejected** in subscriptions where AzSecPack policy mandates UserAssigned only (`InvalidParameter: The value 'SystemAssigned' of parameter 'identity' is not allowed`). Dropped SystemAssigned MI from compute VMSS; the per-compute `Monitoring Metrics Publisher` role assignment is deferred (scheduler + login still publish). Compute-side metric publishing returns in a later release via the existing AzSecPack UAI or a dedicated UAI per pool.

### Added
- `--grafana-location` CLI flag and `grafanaLocation` Bicep param. Defaults to `--location` but can be overridden when the cluster region does not host Azure Managed Grafana (e.g. `southafricanorth` -> `uksouth`). Without this, `--monitoring` in `southafricanorth` failed with `LocationNotAvailableForResourceType` for `Microsoft.Dashboard/grafana`.

### Changed
- Workspace version 0.9.0 -> 0.9.1.
- CLI default `--azcluster-version` bumped to `v0.9.1`.

### Validated
- Live deploy in `southafricanorth` (RG `paul-azcluster`) with `--monitoring --grafana-location uksouth --pool name=cpu,sku=Standard_D8as_v5,count=0,default`: AMW provisions in `southafricanorth`, AMG in `uksouth` linked to AMW, 3 role assignments materialise (2 Metrics Publisher on AMW for scheduler+login VM MIs, 1 Data Reader for Grafana MI), `azcluster monitor mon` returns the Grafana endpoint URL, endpoint responds (HTTP 401 = auth required, server up).

## [0.9.0] - 2026-05-21

### Added
- **Managed observability (infra)**: opt-in `--monitoring` flag on `azcluster deploy` provisions an Azure Monitor Workspace (AMW, Managed Prometheus) and Azure Managed Grafana (AMG) Standard with the AMW linked as a data source. Grafana's system MI gets `Monitoring Data Reader` on the AMW; each cluster VM/VMSS system MI gets `Monitoring Metrics Publisher` so they can later push Prometheus metrics.
- `azcluster monitor <name>` subcommand: prints the Grafana endpoint URL for the named cluster.
- `enableMonitoring` parameter on `bicep/main.bicep` and `bicep/cluster.bicep`; new `bicep/modules/monitoring.bicep` module.
- `SystemAssigned` managed identity on the scheduler (in addition to existing `UserAssigned`), login VM, and every compute VMSS, enabling per-VM RBAC for AMW publishing without touching the AzSecPack-managed UAI.

### Changed
- Workspace version 0.8.0 → 0.9.0.
- CLI default `--azcluster-version` bumped to `v0.9.0`.

### Notes
- Exporters (node_exporter, slurm_exporter, dcgm-exporter) and the actual scrape/remote-write path to AMW are deferred to v0.9.1 so the AMW+AMG provisioning can be live-validated first. With v0.9.0, AMW and Grafana exist and are wired together, but no metrics will appear until v0.9.1 installs and configures the exporters.

## [0.8.0] - 2026-05-21

### Removed
- **Per-pool Azure Spot support** (`--pool ...,spot[,max_price=N]`). Not all Azure SKUs offer Spot capacity, so the per-pool flag was misleading; deploying a Spot pool on an unsupported SKU failed at VMSS validation time. If Spot is needed in the future it will return as a region/SKU-aware feature.
- `spot` / `max_price` tokens from `parse_pool` and the related unit tests.
- `spot` / `spotMaxPrice` parameters from `bicep/modules/compute.bicep`; `priority`/`evictionPolicy`/`billingProfile` no longer set on the VMSS VM profile.
- Spot Quickstart snippets from `README.md`.

### Changed
- Workspace version 0.7.1 → 0.8.0 (breaking CLI: removes `spot`/`max_price` pool tokens).
- CLI default `--azcluster-version` bumped to `v0.8.0`.

## [0.7.1] - 2026-05-21

### Fixed
- **Dynamic node → partition assignment** (Slurm 25.11): `slurmd --conf "...Partitions=<pool>"` is rejected ("Failed to parse nodeline"). Switched to NodeSet+Feature pattern: each pool emits `NodeSet=<pool>set Feature=pool_<pool>` plus `PartitionName=<pool> Nodes=<pool>set ...` in `slurm.conf`; compute nodes register with `Feature=pool_<pool>`.
- **Pyxis missing on scheduler**: scheduler `plugstack.conf` referenced `/opt/pyxis/spank_pyxis.so` but the plugin was never downloaded, so `srun` from the scheduler crashed with `Dlopen of plugin file failed`. Scheduler cloud-init now fetches `spank_pyxis-<ver>-x86_64-linux.so` from the release assets (matches login/compute).
- **`nvidia-smi` false positive on CPU SKUs**: the `microsoft-dsvm:ubuntu-hpc` image ships `nvidia-smi` even on non-GPU VMs, so `command -v nvidia-smi` succeeded on D-series, then `nvidia-smi -L | wc -l` returned a bogus count and downstream `nvidia-smi -i 0` aborted the install script under `set -e`. Now counts lines matching `^GPU [0-9]+:` with `|| true`.
- **ANF preflight failure** (API `2024-03-01`): `exportPolicy.rules` now requires `kerberos5{,i,p}{ReadOnly,ReadWrite}` fields; added them to `bicep/modules/anf.bicep`.
- **Spot `maxPrice` serialization**: ARM rejected the JSON `Float` form of `maxPrice`; CLI now serializes `max_price` as a quoted string and Bicep converts via `json(spotMaxPrice)`.
- **apt-lock race with `unattended-upgrades` on first boot**: cloud-init now masks `unattended-upgrades.service` and the `apt-daily{,-upgrade}.{service,timer}` units, and passes `-o DPkg::Lock::Timeout=600` to every `apt-get` invocation in scheduler/login/compute templates.

### Changed
- Workspace version 0.7.0 → 0.7.1.
- CLI default `--azcluster-version` bumped to `v0.7.1`.

## [0.7.0] - 2026-05-21

### Added
- Per-pool Azure Spot support: `--pool name=g,sku=...,count=N,spot[,max_price=0.5]`. Defaults to `Regular` with `maxPrice=-1` (no cap, evicted only by capacity).
- 8 unit tests for `parse_pool` covering minimal spec, default flag, spot flag, spot with max_price, missing name/sku, unknown key, malformed token.

### Changed
- Workspace version 0.6.0 → 0.7.0.
- CLI default `--azcluster-version` bumped to `v0.7.0`.
- `compute.bicep` now accepts `spot` (bool) and `spotMaxPrice` (string-encoded number) params; sets `priority`/`evictionPolicy`/`billingProfile` on VMSS VM profile when spot.



### Added
- `azcluster validate <name> [--gpu] [--no-container]` — smoke-test the cluster: sinfo, `srun hostname`, Pyxis container srun, optional GPU srun. Fails non-zero if any check fails.
- Slurm `HealthCheckProgram=/usr/local/sbin/azcluster-healthcheck` (interval 300s) — drains a node when `nvidia-smi -q` reports GPU loss / pending page retirement / ERR, or when InfiniBand link is not Active.
- Health-check script installed by compute cloud-init.

### Changed
- Workspace version 0.5.0 → 0.6.0.
- CLI default `--azcluster-version` bumped to `v0.6.0`.

### Added
- `azcluster logs <name> [--component scheduler|login|<compute-host>] [--tail N] [--follow]` — tail `/var/log/azcluster/install.log` on any cluster node via login as jumpbox.
- AMLFS auto-mount on login node (was compute-only). When `--amlfs-size-tib > 0`, login installs `amlfs-lustre-client` and mounts at `/amlfs` so users can stage data via `azcluster ssh`/`scp`.

### Changed
- `login.bicep` accepts `amlfsMountCommand`; `login.yaml.tmpl` substitutes `{{AMLFS_MOUNT_CMD}}`.
- Workspace version 0.4.0 → 0.5.0.
- CLI default `--azcluster-version` bumped to `v0.5.0`.

### Added
- `azcluster exec <name> -- <cmd...>` — run a one-shot command on the login VM (or scheduler with `--scheduler`).
- `azcluster ssh --scheduler` — SSH straight to the scheduler VM, hopping through login as jumpbox (`ssh -J`).
- Scheduler stages example job scripts in `/shared/examples/`: `hostname.sbatch`, `pyxis-alpine.sbatch`, `gpu-smi.sbatch`, `nccl-allreduce.sbatch` (2x8 H100/H200 via Pyxis + nvcr pytorch container).
- `ssh -A` forward-agent flag on `azcluster ssh` (lets you push the next hop without re-authing).

### Changed
- Workspace version 0.3.0 → 0.4.0.
- CLI default `--azcluster-version` bumped to `v0.4.0`.

### Added
- `azcluster status <name>` — prints saved state and live VMSS capacity per pool.
- `azcluster delete <name>` — `az group delete --no-wait` with typed-name confirmation (`--yes` to skip), removes local state file.
- AMLFS auto-mount on compute nodes: when `--amlfs-size-tib > 0`, compute installs `amlfs-lustre-client` and mounts the filesystem at `/amlfs` from cloud-init.
- `amlfsMountCommand` threaded through `cluster.bicep` → `compute.bicep` → `compute.yaml.tmpl` (`{{AMLFS_MOUNT_CMD}}`).

### Changed
- Workspace version 0.2.0 → 0.3.0.
- CLI default `--azcluster-version` bumped to `v0.3.0`.

### Added
- Multi-pool support: `pools` Bicep param iterates `module compute = [for pool in pools]`; `partitionsConf` joined from pool names.
- CLI `--pool name=...,sku=...,count=N[,default]` repeatable flag (replaces `--compute-pool/--sku/--count`).
- Compute hostnames now include pool name: `<cluster>-<pool>-NNNN`.
- IB tunings: `mlx5_core prof_sel=3`, `pcie_relaxed_ordering`, adaptive coalescing.
- GPU/NCCL tunings: persistence mode, `EXCLUSIVE_PROCESS`, NCCL env defaults via `/etc/profile.d/nccl-azcluster.sh` (NDv5 H100/H200 topology file).
- `memlock`/`stack`/`nofile` raised to unlimited / 1048576 on compute.
- AMLFS (Azure Managed Lustre) bicep module + CLI flags `--amlfs-size-tib`, `--amlfs-sku`, `--amlfs-zone`. Opt-in (`size=0` disables). Outputs `mgsAddress` and `mountCommand` for manual mount; auto-mount on nodes planned for v0.3.
- New `amlfs` subnet at `cidrSubnet(vnet, 24, 3)` (defaults to `10.42.3.0/24`).
- `ClusterState.compute_vmss_names: Vec<String>` (was `compute_vmss_name: Option<String>`).
- `CHANGELOG.md` (Keep a Changelog format) and `AGENTS.md` (agent operating instructions, autonomous-versioning directive).

### Changed
- `scheduler.bicep` takes `partitionsConf` instead of `computePoolName`/`computeSku`.
- `compute.bicep` substitutes `{{POOL_NAME}}` and slurmd `--conf` now includes `Partitions=<pool>`.
- `main.bicep` outputs `computeVmssNames array`, `amlfsMgsAddress`, `amlfsMountCommand`.
- README updated: Phase 1 shipped, Phase 2 in progress, multi-pool quickstart.
- Workspace version 0.1.0 → 0.2.0.

## [0.1.1] - 2026-05

### Added
- NAT Gateway on scheduler/login/compute subnets (egress without public IPs).
- Scheduler User-Assigned Identity with RG-Contributor for VMSS scale operations.
- `slurm-smd-sackd` service (`Type=notify --systemd`) for slurmctld dependency.
- `{{UAI_CLIENT_ID}}` substitution in scheduler cloud-init.
- 300s reqwest timeout in CLI for slow `az vmss scale` calls.
- `AZCLUSTER_VERSION`/`AZCLUSTER_REPO` in login `cluster.env`.

### Fixed
- `gpg --batch --yes --dearmor` to avoid interactive prompt failure in cloud-init.
- `mkdir -p` before `tee` for `/etc/apt/keyrings`.
- Infinite wait loop for `munge.key` on compute (was bounded, failed slow boots).
- `az vmss scale --new-capacity` replaces `az vmss update --set sku.capacity` (avoids `LinkedAuthorizationFailed` against tenant AzSecPack UAI).
- `/etc/slurm/plugstack.conf` written on login + compute (configless doesn't distribute `plugstack.conf.d/`).
- `chmod 1777` on `/var/lib/enroot{,/runtime}` `/var/lib/enroot-data{,/cache}` `/run/enroot` recursively.
- `spank_pyxis.so` installed on login node (not just compute).
- `GPU_COUNT` integer-expression bug (initialise to 0 before conditional).
- Munge service restart after key install.
- Versioned tarball + sha256 filename consistency.

## [0.1.0] - 2026-05

### Added
- Phase 1 end-to-end: VMSS Flex compute pool, ANF shared filesystem, Slurm + Pyxis + Enroot fully wired.
- `azcluster scale <cluster> <pool> <from>/<to>` flips VMSS capacity via control-plane.
- `POST /v1/pools/:name/scale` endpoint on `azcluster-server`.
- Persisted state at `~/.config/azcluster/clusters/<name>.toml`.
- Dynamic-node Slurm: slurmd registers itself with `--conf-server` + `--conf Partitions=...`.
- Pyxis 0.21.0 + Enroot 4.0.1 installed on compute and login.
- Hostnames `<cluster>-cn-NNNN` derived from private-IP fourth/third octet.

### Validated (live, `paul-azcluster`/`southafricanorth`)
- `sinfo` / `srun -N1 hostname` → `p1-cn-0001`.
- `srun -N1 --container-image=docker://alpine:latest hostname` (Pyxis container path).
- `azcluster scale p1 gpu 0/1` round-trip verified via `az vmss show`.

## [0.0.1] - 2026-05

### Added
- Phase 0 skeleton: scheduler VM + login VM running Ubuntu HPC.
- `azcluster-server` axum daemon serving `/v1/healthz`.
- `azcluster-cli` clap skeleton: `deploy`, `ssh`, `tunnel`.
- Bicep: `main.bicep` (subscription scope), `cluster.bicep`, modules for `network`, `scheduler`, `login`, `anf`.
- Cloud-init templates for scheduler + login.
- CI (`ci.yml`) + Release (`release.yml`) workflows; binaries published to GitHub Releases.
- `Vec<NodePool>` core data model in `azcluster-core` (no autoscaling).

[Unreleased]: https://github.com/edwardsp/azcluster/compare/v0.24.0...HEAD
[0.24.0]: https://github.com/edwardsp/azcluster/releases/tag/v0.24.0
[0.23.2]: https://github.com/edwardsp/azcluster/releases/tag/v0.23.2
[0.23.1]: https://github.com/edwardsp/azcluster/releases/tag/v0.23.1
[0.23.0]: https://github.com/edwardsp/azcluster/releases/tag/v0.23.0
[0.22.5]: https://github.com/edwardsp/azcluster/releases/tag/v0.22.5
[0.22.4]: https://github.com/edwardsp/azcluster/releases/tag/v0.22.4
[0.22.3]: https://github.com/edwardsp/azcluster/releases/tag/v0.22.3
[0.22.2]: https://github.com/edwardsp/azcluster/releases/tag/v0.22.2
[0.22.1]: https://github.com/edwardsp/azcluster/releases/tag/v0.22.1
[0.22.0]: https://github.com/edwardsp/azcluster/releases/tag/v0.22.0
[0.21.4]: https://github.com/edwardsp/azcluster/releases/tag/v0.21.4
[0.21.3]: https://github.com/edwardsp/azcluster/releases/tag/v0.21.3
[0.21.2]: https://github.com/edwardsp/azcluster/releases/tag/v0.21.2
[0.21.1]: https://github.com/edwardsp/azcluster/releases/tag/v0.21.1
[0.21.0]: https://github.com/edwardsp/azcluster/releases/tag/v0.21.0
[0.19.3]: https://github.com/edwardsp/azcluster/releases/tag/v0.19.3
[0.19.2]: https://github.com/edwardsp/azcluster/releases/tag/v0.19.2
[0.19.1]: https://github.com/edwardsp/azcluster/releases/tag/v0.19.1
[0.19.0]: https://github.com/edwardsp/azcluster/releases/tag/v0.19.0
[0.13.8]: https://github.com/edwardsp/azcluster/releases/tag/v0.13.8
[0.13.7]: https://github.com/edwardsp/azcluster/releases/tag/v0.13.7
[0.13.6]: https://github.com/edwardsp/azcluster/releases/tag/v0.13.6
[0.13.5]: https://github.com/edwardsp/azcluster/releases/tag/v0.13.5
[0.11.4]: https://github.com/edwardsp/azcluster/releases/tag/v0.11.4
[0.11.3]: https://github.com/edwardsp/azcluster/releases/tag/v0.11.3
[0.11.2]: https://github.com/edwardsp/azcluster/releases/tag/v0.11.2
[0.11.1]: https://github.com/edwardsp/azcluster/releases/tag/v0.11.1
[0.11.0]: https://github.com/edwardsp/azcluster/releases/tag/v0.11.0
[0.10.1]: https://github.com/edwardsp/azcluster/releases/tag/v0.10.1
[0.10.0]: https://github.com/edwardsp/azcluster/releases/tag/v0.10.0
[0.9.1]: https://github.com/edwardsp/azcluster/releases/tag/v0.9.1
[0.9.0]: https://github.com/edwardsp/azcluster/releases/tag/v0.9.0
[0.8.0]: https://github.com/edwardsp/azcluster/releases/tag/v0.8.0
[0.7.0]: https://github.com/edwardsp/azcluster/releases/tag/v0.7.0
[0.6.0]: https://github.com/edwardsp/azcluster/releases/tag/v0.6.0
[0.5.0]: https://github.com/edwardsp/azcluster/releases/tag/v0.5.0
[0.4.0]: https://github.com/edwardsp/azcluster/releases/tag/v0.4.0
[0.3.0]: https://github.com/edwardsp/azcluster/releases/tag/v0.3.0
[0.2.0]: https://github.com/edwardsp/azcluster/releases/tag/v0.2.0
[0.1.1]: https://github.com/edwardsp/azcluster/releases/tag/v0.1.1
[0.1.0]: https://github.com/edwardsp/azcluster/releases/tag/v0.1.0
[0.0.1]: https://github.com/edwardsp/azcluster/releases/tag/v0.0.1
