# Health Checks

How azcluster keeps a broken compute node out of the scheduling pool.

## TL;DR

A small Rust binary (`azhealthcheck`) runs on every compute node every 5 minutes via Slurm's `HealthCheckProgram` hook. It runs a fixed set of cheap, dependency-free checks. If any check returns `Error`, the binary exits non-zero, and `slurmctld` automatically drains the node. No pings to the scheduler, no custom service, no daemon — just a one-shot exec on a timer.

```
   slurmctld (scheduler)
        │  every 5 min, on every node
        ▼
   slurmd (compute node)
        │  exec
        ▼
   /usr/local/sbin/azcluster-healthcheck   (1-line wrapper, written by cloud-init)
        │  exec
        ▼
   /usr/local/bin/azhealthcheck --services slurmd,prometheus,node_exporter[,dcgm-exporter]
        │
        ├── gpu_count   (sysfs PCI ↔ /dev/nvidia* parity)
        ├── gpu_xid     (NVRM Xid scan in dmesg, fatal/soft classification)
        ├── network     (eth + IB operstate/carrier + flap counter)
        ├── kmsg        (kernel emerg/alert/crit messages in last hour)
        └── systemd     (`systemctl is-active` per service)

   exit code = max severity (Ok=0, Warning=1, Error=2)
   exit != 0 → slurmctld drains the node with the reason logged
```

## Why this shape

- **Slurm already has a hook for this.** `HealthCheckProgram=<path>` + `HealthCheckInterval=300` + `HealthCheckNodeState=ANY,CYCLE` is the standard Slurm contract — non-zero exit drains. No need to invent a parallel notification path.
- **One-shot exec, no daemon.** Nothing to keep alive, nothing to monitor, no socket. The cost of a missed run is "the next run, 5 min later".
- **No external runtime deps.** Pure Rust + libstd. No DCGM, no `nvidia-smi`, no Python. Anything we can read from `/sys`, `/dev`, or `dmesg --level=...` we read directly. This means the checks survive even when GPU userspace is wedged.
- **Severity → exit code is the only IPC.** The binary prints human-readable lines (or `--json`) for the slurmctld log, but Slurm only cares about the exit code. Mapping is `Ok=0`, `Warning=1`, `Error=2`. Slurm treats 1 and 2 identically (drain), but the split lets us extend later (e.g. warning → log only).
- **Skip-by-default for missing units.** The `systemd` check treats `unit not found` as `missing`, not `failed`. Lets us ship a single global service list (`slurmd,prometheus,node_exporter,dcgm-exporter`) that works on both CPU and GPU nodes — `dcgm-exporter` is simply absent on CPU nodes and the check stays green. (The wrapper script also auto-trims `dcgm-exporter` when `nvidia-smi` is missing, as a belt-and-braces measure.)

## The five checks

All implemented in `crates/azhealthcheck/src/checks.rs`. Each returns a `CheckOutcome { name, severity, message, findings }`.

### `gpu_count`

Walks `/sys/bus/pci/devices/*`, counts entries whose `vendor == 0x10de` (NVIDIA) **and** whose `class` starts with `0x0300` or `0x0302` (display/3D controller). Then counts `/dev/nvidia<N>` device nodes. The two numbers must match.

| Outcome | Condition |
|---|---|
| `Ok`    | both counts are 0 (CPU node) |
| `Ok`    | counts match and > 0 |
| `Error` | counts differ (e.g. PCI sees 8 GPUs, /dev sees 6 → driver lost two) |

This catches the common NDv5 failure where the driver enumerates fewer GPUs than the PCI fabric exposes. The check is sysfs-only — no `nvidia-smi` shell-out, so it works even when userspace is hung.

### `gpu_xid`

Runs `dmesg --time-format=iso`, scans for the substring `NVRM: Xid`, parses out the Xid number (using `split_once("): ")` to skip past the PCI address). Each Xid is classified:

- **Fatal** (`{48, 61, 62, 63, 64, 74, 79, 94, 95}`): MMU fault, GSP-RM error, contained ECC error, GPU has fallen off the bus, NVLink error, uncontained ECC, etc. → `Error`, drain.
- **Soft warning** (`{43, 45}`): SW-induced GPU reset, preemptive cleanup of running app → `Warning`. Often a sign of an OOM in a user job; logs but does not drain.
- **Any other Xid**: treated as `Error` (drain). Better to be conservative; XIDs are rare in steady state.

The classification table is the conservative subset of the NVIDIA Xid catalogue — every fatal we've listed is on NVIDIA's own "node should be drained" list.

### `network`

Walks `/sys/class/net/*`, skipping `lo`, `docker*`, `veth*`. Keeps only `type == 1` (Ethernet) or `type == 32` (InfiniBand). For each survivor:

- `operstate` must be `up` (else `Error`).
- `carrier` must be `1` (else `Error`).
- If `operstate == up` but `carrier_down_count > 0`, emits `Warning` ("flapped") — a NIC that has flapped at least once since boot is suspicious even if currently up.

On NDv5 this validates all 8 `mlx5_ib*` IB ports plus `eth0` in one pass.

### `kmsg`

Runs `dmesg --level=emerg,alert,crit --since="1 hour ago"`. Any non-empty line is an `Error` finding. The 1-hour window is the same as the Slurm health-check interval × 12 — enough to catch issues that surfaced between two checks but not noisy enough to re-report ancient events for hours.

Catches things like `Hardware error from APEI`, `mce: [Hardware Error]`, `EDAC ... uncorrectable error`, NVMe controller resets — anything the kernel itself considers critical.

### `systemd`

For each service name passed via `--services`, runs `systemctl is-active <svc>` and bucketises the stdout:

| `is-active` says | Bucket | Effect |
|---|---|---|
| `active` | active | (counts toward green) |
| `failed` | failed | `Error` |
| `inactive` / `activating` / `deactivating` / `reloading` | inactive | `Warning` |
| `unknown` | missing | silently skipped (CPU nodes don't have `dcgm-exporter`) |

Default service list on compute nodes (set in the wrapper script):

- `slurmd` — must be running, else the node can't take jobs.
- `prometheus` — local node-exporter scraper that remote-writes to AMW.
- `node_exporter` — host metrics.
- `dcgm-exporter` — GPU metrics (auto-added by the wrapper when `nvidia-smi` is present and lists at least one GPU).

## Severity → exit code

`crates/azhealthcheck/src/types.rs`:

```rust
pub enum Severity { Ok, Warning, Error }

impl Severity {
    pub fn exit_code(self) -> i32 {
        match self {
            Severity::Ok      => 0,
            Severity::Warning => 1,
            Severity::Error   => 2,
        }
    }
}
```

The binary takes the **max** severity across all check outcomes and exits with that. Slurm treats any non-zero exit as a drain trigger, but keeping `Warning ≠ Error` leaves the door open for future "log but don't drain" behaviour (would require a wrapper that masks exit 1 → 0, or an op-side decision to lower flap-only outcomes to OK).

## Where each piece lives

### Binary

- **Crate**: `crates/azhealthcheck/` — workspace member, builds to a single ~835 KB stripped static binary.
- **CLI** (`src/main.rs`): flags `--checks`, `--services`, `--json`, `--sys-root`, `--dev-root`. The last two are the test-injection seams; production always uses the defaults `/sys` and `/dev`.
- **Tests**: 14 unit tests in `checks.rs` (matching, mismatch, fatal Xid, soft Xid, IB up, Ethernet down, flap, kmsg clean/critical, systemd active/failed/inactive). Tests inject a `FakeRunner` (a HashMap of canned `(prog args) → Output`) instead of shelling out to real `dmesg`/`systemctl`.

### Wrapper script

Written by cloud-init at first boot on each compute node (`cloud-init/compute.yaml.tmpl`):

```bash
#!/bin/bash
SERVICES="slurmd,prometheus,node_exporter"
if command -v nvidia-smi >/dev/null 2>&1 && nvidia-smi -L 2>/dev/null | grep -qE '^GPU [0-9]+:'; then
  SERVICES="${SERVICES},dcgm-exporter"
fi
exec /usr/local/bin/azhealthcheck --services "${SERVICES}" "$@"
```

Why a wrapper at all? Slurm's `HealthCheckProgram` takes a single absolute path, not a command-line. Putting the service-list assembly in a wrapper keeps the Slurm config static (`HealthCheckProgram=/usr/local/sbin/azcluster-healthcheck` is identical on every node) while letting the script auto-detect GPU presence. (We use `nvidia-smi -L | grep -cE '^GPU [0-9]+:'` because the `microsoft-dsvm:ubuntu-hpc` image ships `nvidia-smi` even on CPU SKUs — see AGENTS.md.)

### Slurm wiring

In `cloud-init/scheduler.yaml.tmpl` (these three lines are written into `/etc/slurm/slurm.conf` and distributed to every compute node via Slurm configless mode):

```
HealthCheckProgram=/usr/local/sbin/azcluster-healthcheck
HealthCheckInterval=300
HealthCheckNodeState=ANY,CYCLE
```

- `Interval=300` — every 5 min.
- `NodeState=ANY,CYCLE` — run on any node state (idle, allocated, drained, down) and cycle through nodes so the load doesn't spike. Crucially this includes ALLOCATED nodes, so a GPU that goes bad mid-job is caught without waiting for the job to finish.

### Release & install path

1. Tag push → `.github/workflows/release.yml` builds `azhealthcheck` on the linux job and uploads `azhealthcheck-vX.Y.Z-x86_64-linux.tar.gz` to the GitHub release.
2. Compute cloud-init downloads the tarball from `https://github.com/${REPO}/releases/download/${VERSION}/azhealthcheck-${VERSION}-x86_64-linux.tar.gz`, extracts into `/usr/local/bin/`, chmod 0755.
3. Cloud-init writes the wrapper to `/usr/local/sbin/azcluster-healthcheck`, chmod 0755.
4. By the time `slurmd` starts and registers, the healthcheck binary + wrapper are both in place. The next scheduler tick (≤ 5 min later) will invoke it.

## How to invoke it manually

On a compute node (`azcluster ssh <cluster> --scheduler -- srun -N1 -- ...`):

```bash
# Run all checks, human output (what slurmctld sees by default):
sudo /usr/local/sbin/azcluster-healthcheck

# JSON for log scraping:
sudo /usr/local/bin/azhealthcheck --json --services slurmd,prometheus,node_exporter

# Just one check (e.g. probe for Xid events without touching anything else):
sudo /usr/local/bin/azhealthcheck --checks gpu_xid

# Check exit code matches severity:
sudo /usr/local/sbin/azcluster-healthcheck; echo "exit=$?"
```

Sample human output:

```
OK    gpu_count: 8 GPUs visible
OK    gpu_xid: no Xid events in kernel log
OK    network: 9 interface(s) up: eth0,mlx5_ib0,mlx5_ib1,mlx5_ib2,mlx5_ib3,mlx5_ib4,mlx5_ib5,mlx5_ib6,mlx5_ib7
OK    kmsg: no critical kernel messages in last hour
OK    systemd: 4 service(s) active
```

Sample failure output (one fatal Xid):

```
OK    gpu_count: 8 GPUs visible
ERROR gpu_xid: 1 fatal Xid event(s) in kernel log
        - Xid 79: 2026-05-23T14:22:11 NVRM: Xid (PCI:0000:b1:00): 79, pid=12345, GPU has fallen off the bus
OK    network: 9 interface(s) up: ...
OK    kmsg: no critical kernel messages in last hour
OK    systemd: 4 service(s) active
```

Exit code: 2. `slurmctld` logs the binary's stderr + exit code and drains the node with reason `HealthCheck failed` (visible in `scontrol show node <name>` → `Reason=`).

## How to recover a drained node

After fixing the underlying issue (e.g. rebooting to clear an Xid 79), the operator must `scontrol update nodename=<n> state=resume` from the scheduler. The healthcheck does not auto-undrain — that's intentional, because a fatal Xid usually warrants operator review even after the symptom clears.

## What's intentionally not in scope

These were considered and deferred:

- **DCGM-backed checks** (`gpu_dcgm`, `gpu_nvlink`): NVLink CRC errors, thermal throttle, ECC events. Needs either libdcgm bindings (Rust FFI) or a `nvidia-smi -q` parser. Backlog (not pinned to a release). The current checks catch hard failures; DCGM would catch slow degradation.
- **Intrusive diagnostics** (`gpu_diag`): NCCL p2p ring tests, IB loopback, etc. Too expensive to run every 5 min on an allocated node — would interfere with the user's job. Belongs to a separate `azcluster diag` operator command, not the periodic health check.
- **Azure GHR (GPU Health Report) integration**: would let us call the Azure-side health API and surface platform-reported issues (e.g. node marked unhealthy by the Azure-side telemetry pipeline). Not implemented; could be added as a sixth check that polls IMDS or the Azure API via the node's UAI.
- **Self-undrain on recovery**: see above; intentional.

## Code map

| File | Role |
|---|---|
| `crates/azhealthcheck/Cargo.toml` | Crate manifest. Deps: `anyhow`, `clap`, `serde`, `serde_json`. Dev-dep: `tempfile`. |
| `crates/azhealthcheck/src/main.rs` | Clap CLI, dispatch table, JSON-vs-human output, exit-code computation. |
| `crates/azhealthcheck/src/types.rs` | `Severity`, `CheckOutcome`, `Runner` trait, `RealRunner`, `FakeRunner` (test only). |
| `crates/azhealthcheck/src/checks.rs` | The 5 check functions + 14 unit tests. |
| `cloud-init/compute.yaml.tmpl` | Downloads the release tarball, installs the binary, writes the wrapper script. |
| `cloud-init/scheduler.yaml.tmpl` | Sets `HealthCheckProgram`, `HealthCheckInterval`, `HealthCheckNodeState` in `slurm.conf`. |
| `.github/workflows/release.yml` | Builds + uploads `azhealthcheck-vX.Y.Z-x86_64-linux.tar.gz` on tag. |

## Extending

Adding a new check:

1. Add a `pub fn my_check(runner: &dyn Runner, ...) -> CheckOutcome` in `crates/azhealthcheck/src/checks.rs`. Use `Runner` (not `Command::new` directly) for anything that shells out, so it's testable with `FakeRunner`.
2. Add it to the `ALL_CHECKS` slice and the dispatch `match` in `crates/azhealthcheck/src/main.rs`.
3. Add unit tests using `FakeRunner` for shell-out paths or `tempfile` for sysfs paths.
4. Pick severity carefully: `Error` drains the node, `Warning` doesn't (yet — see "Severity → exit code" above), `Ok` is silent green.
5. No new runtime deps unless absolutely necessary — the value of a static, dep-free binary is that it runs anywhere with no preconditions.
