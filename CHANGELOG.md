# Changelog

All notable changes to azcluster are documented here.
Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Versioning: [SemVer](https://semver.org/).

## [Unreleased]


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

[Unreleased]: https://github.com/edwardsp/azcluster/compare/v0.13.8...HEAD
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
