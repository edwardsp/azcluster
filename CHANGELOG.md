# Changelog

All notable changes to azcluster are documented here.
Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Versioning: [SemVer](https://semver.org/).

## [Unreleased]

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

[Unreleased]: https://github.com/edwardsp/azcluster/compare/v0.7.0...HEAD
[0.7.0]: https://github.com/edwardsp/azcluster/releases/tag/v0.7.0
[0.6.0]: https://github.com/edwardsp/azcluster/releases/tag/v0.6.0
[0.5.0]: https://github.com/edwardsp/azcluster/releases/tag/v0.5.0
[0.4.0]: https://github.com/edwardsp/azcluster/releases/tag/v0.4.0
[0.3.0]: https://github.com/edwardsp/azcluster/releases/tag/v0.3.0
[0.2.0]: https://github.com/edwardsp/azcluster/releases/tag/v0.2.0
[0.1.1]: https://github.com/edwardsp/azcluster/releases/tag/v0.1.1
[0.1.0]: https://github.com/edwardsp/azcluster/releases/tag/v0.1.0
[0.0.1]: https://github.com/edwardsp/azcluster/releases/tag/v0.0.1
