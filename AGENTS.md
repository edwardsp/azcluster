# AGENTS.md

Instructions for AI agents working on this repository. Keep this file current.

## Documentation Discipline (MANDATORY)

Every change MUST update three artifacts in lockstep:

1. **`CHANGELOG.md`** — add entry under `## [Unreleased]` for every user-visible or operator-visible change. On tag, rename `Unreleased` to the new version with the release date, and open a fresh `Unreleased` section. Categories: `Added`, `Changed`, `Fixed`, `Removed`, `Deprecated`, `Security`.
2. **`README.md`** — keep `Status`, `Quickstart`, `Architecture`, `Repo Layout` honest. If a phase ships, update the Status block. If a flag/command changes, update the Quickstart.
3. **`AGENTS.md`** (this file) — when a new workflow, convention, or guard-rail emerges, add it here so the next agent inherits it.

If a PR touches code but skips any of these three, it is incomplete.

## Release Workflow

1. Land all `Unreleased` work; verify `cargo fmt && cargo clippy -- -D warnings && cargo test --workspace` and `az bicep build` clean across all modules.
2. Edit `CHANGELOG.md`: rename `## [Unreleased]` → `## [X.Y.Z] - YYYY-MM-DD`, add a fresh empty `## [Unreleased]` block at the top, update the link references at the bottom.
3. Edit `README.md` Status block if phase advanced.
4. Bump version: `crates/*/Cargo.toml`, the `--azcluster-version` CLI default in `crates/azcluster-cli/src/main.rs`.
5. Commit, tag `vX.Y.Z`, push tag — CI publishes the release.
6. Live-validate on `paul-azcluster`/`southafricanorth` before declaring done.

## Hard Rules (carried from earlier sessions)

- **No CycleCloud / Jetpack / CCWS references** in any public artifact (README, code, commits, tags, release notes). Past mentions were already scrubbed; don't reintroduce them.
- **No personal identifiers or tenant-specific values** in committed files. Subscription IDs, RG names beyond the documented `paul-azcluster`, emails, etc. stay out of git.
- **Public IPs off by default**; opt-in via `--login-public-ip`.
- **All code Rust** except Bicep + bootstrap shell in cloud-init.
- **Research checkouts** live under `research/` (gitignored). Planning artifacts under `.sisyphus/` (gitignored).
- **Minimize comments**; let code be self-documenting. Necessary exceptions: clap doc-comments (they render as `--help` text), complex algorithms, security-critical sections.
- **Never suppress type errors** (`as any`, `#[allow]` blanket, `unwrap()` on user input paths).
- **Never commit** unless the user explicitly asks.
- **Autonomous versioning is enabled** (user directive, AFK): when a coherent unit of work is complete, verified, and changelogged, the agent SHOULD bump the version, commit, tag, and push. Do not wait for explicit per-release permission. Apply SemVer: feature batch → minor; bugfix-only → patch; breaking → major.

## Azure / Infra Gotchas

- AzSecPack policy auto-attaches `AzSecPackAutoConfigUA-<region>` UAI to every VM/VMSS. Any PUT on a VMSS `identity` collection triggers `LinkedAuthorizationFailed` unless the caller has `Managed Identity Operator` on that UAI (out-of-band). **Use `az vmss scale --new-capacity`, not `az vmss update --set sku.capacity`.**
- Slurm configless mode does **not** distribute `plugstack.conf.d/`. Write `/etc/slurm/plugstack.conf` (absolute path) on every node that uses Pyxis (compute + login).
- Enroot needs `1777` on `/var/lib/enroot{,/runtime}`, `/var/lib/enroot-data{,/cache}`, and `/run/enroot` — top-level AND subdirs.
- VMSS Flex VMs surface as `Microsoft.Compute/virtualMachines` named `vmss-<cluster>-<pool>_<hex>`, not under the VMSS resource.
- Image: `microsoft-dsvm:ubuntu-hpc:2404` (default), `2204` fallback.
- Slurm 25.11 + Pyxis 0.21.0 ABI match. NVIDIA Pyxis 0.24.0 exists; only bump if Slurm 26 is needed.
- Slurm 25.11 dynamic nodes: `slurmd --conf "...Partitions=<pool>"` is rejected ("Failed to parse nodeline"). Use the **NodeSet+Feature** pattern: emit `NodeSet=<pool>set Feature=pool_<pool>` and `PartitionName=<pool> Nodes=<pool>set ...` in `slurm.conf`; the compute slurmd registers with `--conf "...Feature=pool_<pool>"` and slurmctld places it in the matching NodeSet/partition.
- Pyxis spank library (`spank_pyxis.so`) must be installed on **every** node that may submit `srun` — scheduler, login, and compute — because `plugstack.conf` loads at submit time. Forgetting it on scheduler crashes any `srun` invoked from there with `Dlopen of plugin file failed`.
- The `microsoft-dsvm:ubuntu-hpc` image ships `nvidia-smi` even on CPU SKUs, so `command -v nvidia-smi` cannot be used as a GPU presence check. Use `nvidia-smi -L | grep -cE '^GPU [0-9]+:' || true` instead.
- **Managed Grafana region coverage**: `Microsoft.Dashboard/grafana` is NOT available in `southafricanorth` (and several other regions). Use `--grafana-location uksouth` (or another supported region) when the cluster region lacks AMG.
- **Monitoring Data Reader role GUID** is `b0d8363b-8ddd-447d-831f-62ca05bff136` (NOT the `...51537` value some docs list). Verify role GUIDs with `az role definition list --name "..."` before baking into Bicep.
- **AMW ingestion RBAC scope**: `Monitoring Metrics Publisher` MUST be granted on the AMW's **default Data Collection Rule**, not on the AMW account itself. Role at the AMW scope passes role-listing checks but the ingestion endpoint still returns `403 The authentication token provided does not have access to ingest data for the data collection rule with immutable Id 'dcr-...'`. The default DCR lives in the Azure-managed sister RG `MA_<amwName>_<location>_managed`.
- **AMW ingestion RBAC propagation**: 5-10 minutes on a freshly-created MI + DCR. After it propagates, `systemctl restart prometheus` is required - prometheus and IMDS cache the bearer token, and the cached token's authorization is decided server-side at request time but the connection state appears to survive the role landing. A direct `curl` test (`POST` empty body to the ingestion URL with an IMDS token) is the fastest way to confirm whether the failure is RBAC vs config.
- **Prometheus 3.x cloud-init perms**: `cat > /opt/prometheus/prometheus.yml <<EOF ... EOF` in a cloud-init `runcmd` produces a file with mode `0600` (root's umask). The non-root `prometheus` service user cannot read it -> "permission denied" at startup. Always follow with explicit `chmod 0644`.
- **Prometheus `azuread.managed_identity` remote_write**: works directly against the AMW DCE ingestion URL (`${dceEndpoint}/dataCollectionRules/${dcrImmutableId}/streams/Microsoft-PrometheusMetrics/api/v1/write?api-version=2023-04-24`). No AMA or DCR custom scrape config needed for VMs. Block shape:
  ```yaml
  remote_write:
    - url: "..."
      azuread:
        cloud: AzurePublic
        managed_identity:
          client_id: "<uai-client-id>"
  ```
  Audience is implicit (`https://monitor.azure.com/`). The UAI MUST be attached to the VM AND hold Metrics Publisher on the DCR (see above).
- **VMSS Flex + SystemAssigned identity** is rejected in subscriptions with AzSecPack/UAI-only policy (`InvalidParameter` on `identity`). VMs are fine; VMSS must use UserAssigned (or no MI).
- **Grafana dashboard JSON** lives under `grafana/dashboards/` and is provisioned by `bicep/modules/grafana-dashboards.bicep`. Keep panel ids stable across edits, use a datasource template variable named `DS_PROMETHEUS`, and never hardcode Azure Managed Grafana datasource UIDs.
- Phase 1+ test region: `southafricanorth`, RG `paul-azcluster`, max 2 GPU nodes.

## Subnetting (VNet `10.42.0.0/16`)

- scheduler `10.42.1.0/24` (first `.4`)
- login `10.42.2.0/24` (first `.4`)
- compute `10.42.4.0/22` (first `.4.4`)
- anf `10.42.0.0/26`
- database `10.42.8.0/29` (delegated to `Microsoft.DBforMySQL/flexibleServers`, only when `--accounting` on)
- Ports `8443` (control plane), `6817` (slurmctld), `6819` (slurmdbd, localhost only), `3306` (MySQL Flex from scheduler). UID/GID `11100` for `slurm`.

## Delegation Conventions

- Live Azure / KQL investigation → use the `azure-infra-analyst` skill.
- Multi-file refactors → `explore` agents in parallel before editing.
- External libraries (Pyxis, Enroot, Slurm internals) → `librarian` agent.
- Hard architecture calls or post-implementation review → `oracle`.
- Plans that get written under `.sisyphus/plans/*.md` → `momus` for review.

## Verification Gates (before declaring "done")

- `cargo fmt --all` clean
- `cargo clippy --workspace --all-targets -- -D warnings` clean
- `cargo test --workspace` green
- `for f in bicep/main.bicep bicep/cluster.bicep bicep/modules/*.bicep; do az bicep build --file "$f" --stdout > /dev/null; done` clean
- `CHANGELOG.md` updated
- `README.md` Status/Quickstart still accurate
- `AGENTS.md` updated if process changed

If any of these fail, fix before claiming completion.
- **YAML write_files heredoc terminators**: cloud-init `write_files` with `content: |` block scalars strips the FIRST line's leading whitespace as the indent and removes that prefix from every subsequent line. Any `EOF` terminator MUST sit at exactly that base indent column in the template so it lands at column 0 in the materialised script - bash heredocs without `<<-` only recognise the closer at column 0. Extra indentation from being nested inside `if`/`for` (e.g. 6-space base + 2-space if-body = 8-space `EOF`) silently breaks the heredoc and chains errors until bash hits EOF with "syntax error: unexpected end of file". Always put `EOF` at the template's base indent column even inside `if` blocks.
- **AMG Grafana Admin for dashboard import**: `az grafana dashboard create` calls the Grafana HTTP API (`POST /api/dashboards/db`), which requires a Grafana role on the AMG. ARM contributor / owner is not enough. Grant the deployer principal `Grafana Admin` (`22926164-76b3-42b3-bc55-97df8dab3e41`) on the AMG resource scope, and expect 1-3 min propagation - retry with back-off on `401 NoRoleAssignedException`.
- **`Microsoft.Dashboard/grafana/dashboards@*` is NOT a real ARM resource**: BCP081 ("resource type does not have types available") is the warning, and ARM preflight returns `ResourceTypeRegistrationNotFound`. Import dashboards via the Grafana HTTP API (`az grafana dashboard create` or direct REST), not Bicep. The CLI does this post-deploy with the dashboard JSONs embedded via `include_str!`.
- **Shared filesystem modes**: `--shared-storage anf` (default) provisions Azure NetApp Files; `--shared-storage nfs-scheduler` exports `/shared` from the scheduler via `nfs-kernel-server`. The latter is test-only (no HA, SPOF on scheduler) but saves ~12 min off provisioning. Login + compute always mount with a 5-minute retry loop because the scheduler is still installing its NFS server while they boot in `nfs-scheduler` mode.
- **Deployment timing**: every successful `azcluster deploy` writes `~/.config/azcluster/deployments/<cluster>/<utc-stamp>.json` with per-resource `duration_seconds` and a `trend.tsv` for comparison. Inspect via `azcluster timings <cluster>`. Source: `az deployment operation {sub,group} list` recursed across nested modules; `properties.duration` parsed from ISO-8601 (`PT##H##M##S`).
- **`--no-monitoring` / `--no-accounting`**: monitoring + accounting default to ON. Pass these flags to skip them during rapid test deploys that don't depend on observability or accounting. Accounting (v0.13.0+) provisions an Azure Database for MySQL Flexible Server + runs `slurmdbd` on the scheduler.
- **MySQL Flex TLS CA**: Azure MySQL Flexible Server presents a DigiCert Global Root CA chain, which is **already in Ubuntu's `ca-certificates`** bundle. Set `StorageParameters=SSL_CA=/etc/ssl/certs/ca-certificates.crt` in `slurmdbd.conf`; do NOT try to download `https://dl.cacerts.digicert.com/DigiCertGlobalRootCA.crt.pem` — that host returns a cert whose SAN doesn't match the hostname and `curl` aborts with `SSL: no alternative certificate subject name matches target host name`. v0.13.0/v0.13.1 had this bug; fixed in v0.13.2.
- **Slurm accounting (v0.13.0)**: MySQL Flexible Server (`mysql-<cluster>`, `Standard_B2ms`, MySQL 8.0.21, 50 GB autogrow, `publicNetworkAccess=Disabled`) is provisioned in a delegated subnet `10.42.8.0/29` (delegation `Microsoft.DBforMySQL/flexibleServers`). The CLI auto-generates the admin password from `/dev/urandom` (32 random chars from an ambiguity-free alphabet plus literal `Aa1!` to satisfy Azure's "≥3 character classes" policy — Azure rejects passwords with only 2 classes) and passes it as a `@secure()` Bicep parameter. On the scheduler, the password lands in `/etc/azcluster/accounting.password` (mode 0600 root:root via cloud-init `write_files`) and is read into `slurmdbd.conf` (mode 0600 slurm:slurm) at boot, then the shell var is `unset`. TLS is required: the bootstrap downloads `https://dl.cacerts.digicert.com/DigiCertGlobalRootCA.crt.pem` to `/etc/slurm/ssl/` and `slurmdbd.conf` sets `StorageParameters=SSL_CA=…`. Bootstrap order MUST be: install `slurm-smd-slurmdbd` → write `slurmdbd.conf` → wait for `:3306` reachability (60×5s probe via `/dev/tcp`) → start `slurmdbd` → sleep 5s → start `slurmctld` → `sacctmgr -i add cluster`. Starting `slurmctld` before `slurmdbd` with `AccountingStorageEnforce=associations,limits,qos` set will cause slurmctld to refuse to start. Three MySQL server params are tuned per slurmdbd recommendations: `innodb_lock_wait_timeout=900`, `max_allowed_packet=16777216`, `log_bin_trust_function_creators=ON`.
- **`aka.ms/InstallAzureCLIDeb` is FORBIDDEN inside `set -euo pipefail` bootstraps** (v0.13.1 fix). The pipe script runs `apt-get install` internally without our `DPkg::Lock::Timeout=600`, so it races `apt-daily`/`unattended-upgrades` and aborts the entire bootstrap with `Could not get lock /var/lib/dpkg/lock-frontend`. Cloud-init still reports "finished" because runcmd pipes through `tee`. Always install azure-cli via the explicit MS apt repo (`packages.microsoft.com/repos/azure-cli/`) using our own `apt-get -o DPkg::Lock::Timeout=600 install -y azure-cli`. The MS GPG key (`/usr/share/keyrings/microsoft-prod.gpg`) is already imported earlier in the scheduler bootstrap for the slurm repo, so reuse it.
