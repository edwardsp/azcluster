---
name: azcluster
description: "Deploy and operate azcluster Slurm/Pyxis/Enroot HPC-AI clusters on Azure (NDv5 H100/H200, multi-node NCCL, containerised MPI). Use for: deploying a cluster, ssh/exec/scp into login/scheduler/compute, scaling pools, running validation/NCCL tests, submitting Slurm jobs, managing LDAP users, monitoring (Grafana), reading install logs, tearing down, purging Key Vaults. Downloads the latest azcluster CLI from GitHub Releases — no local repo required. Also covers cloning the repo to debug cloud-init/Bicep/CLI internals."
---

# azcluster

`azcluster` is a single Rust binary that deploys a production HPC/AI Slurm cluster on Azure (Slurm + Pyxis + Enroot, NDv5 H100/H200, IB + NCCL tunings, ANF `/shared`, per-cluster blob storage, Key Vault, LDAP multi-user, Grafana). It runs on your machine, authenticates to Azure via OAuth2 (no `az` CLI dependency), and calls ARM REST natively. The cluster runs entirely on Azure; there is no laptop-side daemon.

- **Repo:** https://github.com/edwardsp/azcluster (owner `edwardsp`)
- **Issues / roadmap:** https://github.com/edwardsp/azcluster/issues
- **Binary name:** `azcluster`

You do **not** need the repo checked out to operate clusters — install the CLI from GitHub Releases (below). Clone the repo only when you need to debug cloud-init, Bicep, or CLI internals (see [Debugging & the repo](#debugging--the-repo)).

---

## 1. Install the CLI (latest release, no repo)

The CLI is published per-tag to GitHub Releases. Each release ships a versioned tarball plus a `SHA256SUMS` file. Each tarball contains a single top-level `azcluster` binary.

| Asset | Example (v0.24.12) |
|---|---|
| Linux x86_64 CLI | `azcluster-cli-v0.24.12-x86_64-linux.tar.gz` |
| macOS arm64 CLI | `azcluster-cli-v0.24.12-aarch64-darwin.tar.gz` |
| Checksums | `SHA256SUMS` |

### Recommended: fetch latest tag, verify checksum, install

```bash
set -euo pipefail
REPO=edwardsp/azcluster

# 1. Resolve the latest release tag from the GitHub API
VERSION=$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
  | grep -m1 '"tag_name"' | sed -E 's/.*"tag_name": *"([^"]+)".*/\1/')

# 2. Pick the arch asset (uname-based)
case "$(uname -s)/$(uname -m)" in
  Linux/x86_64)   ARCH=x86_64-linux ;;
  Darwin/arm64)   ARCH=aarch64-darwin ;;
  *) echo "unsupported platform: $(uname -s)/$(uname -m)"; exit 1 ;;
esac

# 3. Download tarball + checksums
TARBALL="azcluster-cli-${VERSION}-${ARCH}.tar.gz"
BASE="https://github.com/${REPO}/releases/download/${VERSION}"
curl -fsSLO "${BASE}/${TARBALL}"
curl -fsSLO "${BASE}/SHA256SUMS"

# 4. Verify (only the file we downloaded)
sha256sum --ignore-missing -c SHA256SUMS

# 5. Extract + install (tarball has a top-level `azcluster`)
tar -xzf "${TARBALL}"
install -m 0755 azcluster /usr/local/bin/azcluster   # add sudo if needed
azcluster --version
```

Pin a specific version by setting `VERSION=v0.24.12` and skipping step 1.

### Other release assets (rarely needed by an operator)

`azcluster-server-${VERSION}-x86_64-linux.tar.gz` (scheduler daemon), `azhealthcheck-${VERSION}-x86_64-linux.tar.gz` (per-node health probe), `spank_pyxis-${VERSION}-x86_64-linux.so` (Slurm Pyxis plugin), `azcluster-assets-${VERSION}.tar.gz` (scripts/cloud-init/bicep), `azcluster-main-${VERSION}.json` (transpiled ARM template). Cloud-init on each node fetches these automatically at boot — you normally never download them by hand.

### Build from source (only if you cloned the repo)

```bash
cargo build --release --workspace   # → target/release/azcluster
```

---

## 2. Authenticate

No `az` CLI needed. Tokens cache at `~/.azure/azcli_tokens.json` (mode 0600).

```bash
azcluster login                                  # interactive browser PKCE
azcluster login --device-code                    # headless / SSH session
azcluster login --tenant <id> --subscription <id>
```

- On Microsoft tenants where Conditional Access blocks device-code (AADSTS53003), run `azcluster login` once in a browser-equipped shell, then `azcluster login --subscription <id>` rebinds from any TTY (a fast cache mutation, no re-auth).

---

## 3. Deploy a cluster

Minimal production deploy (ANF + monitoring + accounting + Bastion, no public IPs):

```bash
azcluster deploy --name demo \
  --location eastus --grafana-location eastus \
  --pool name=gpu,sku=Standard_ND96isr_H100_v5,count=2,default \
  --bastion
```

`--pool` is **repeatable**; format `name=<n>,sku=<sku>,count=<N>[,default]`. The `default` pool's partition is Slurm's default.

### Deploy flags (from the clap definition — authoritative)

| Flag | Type / default | Notes |
|---|---|---|
| `--name <name>` | required | Cluster name; drives RG (`rg-azcluster-<name>`), VM/KV names |
| `--location <region>` | required | Azure region for compute |
| `--grafana-location <region>` | defaults to `--location` | AMG isn't in every region (e.g. use `uksouth` for `southafricanorth`) |
| `--resource-group <name>` | auto `rg-azcluster-<name>` | Override RG name |
| `--pool name=...,sku=...,count=N[,default]` | required, repeatable | One VMSS Flex per pool |
| `--scheduler-sku <sku>` | `Standard_D8as_v5` | Use `Standard_HB120rs_v3` when D-class capacity is tight |
| `--login-sku <sku>` | `Standard_D4as_v5` | |
| `--ubuntu {2204,2404}` | `2404` | `microsoft-dsvm:ubuntu-hpc` series |
| `--bastion` | off | Standard Bastion + tunneling; `ssh`/`exec`/`scp`/`tunnel` auto-route |
| `--login-public-ip` | off | Public IP on login (vs Bastion) |
| `--allowed-ssh-cidrs <cidr,...>` | `0.0.0.0/0` | NSG allowlist when login has a public IP |
| `--shared-storage {anf,nfs-scheduler}` | `anf` | `nfs-scheduler` = scheduler-exported NFS (SPOF, faster, test only) |
| `--anf-size-tib <N>` / `--anf-tier {Standard,Premium,Ultra}` | `2` / `Standard` | ANF volume |
| `--amlfs-size-tib <N>` / `--amlfs-sku <sku>` / `--amlfs-zone <z>` | `0` (off) / `AMLFS-Durable-Premium-250` / `1` | Azure Managed Lustre at `/amlfs` |
| `--monitoring` / `--no-monitoring` | on | AMW + AMG (`--no-monitoring` saves ~3 min) |
| `--accounting` / `--no-accounting` | on | MySQL Flex + slurmdbd (`--no-accounting` saves ~5 min) |
| `--storage` / `--no-storage` | on | Per-cluster Storage account |
| `--storage-name <name>` | derived | Override storage account name (3–24 lowercase alnum, globally unique) |
| `--storage-sku <sku>` | `Standard_LRS` | one of `Standard_LRS,Standard_ZRS,Standard_GRS,Standard_RAGRS,Premium_LRS` |
| `--storage-tier {Hot,Cool}` | `Hot` | |
| `--storage-hns` | off | ADLS Gen2 / hierarchical namespace |
| `--storage-public-access` | off | Default is Private-Endpoint-only |
| `--azcp-version <vX.Y.Z>` | `v0.4.5` | `azcp` binary version baked into cloud-init |
| `--extra-package <pkg>` | repeatable | Extra apt packages on every node at boot |
| `--azcluster-version <vX.Y.Z>` | matches CLI | Cloud-init fetches the matching release tarball |
| `--azcluster-repo <owner/repo>` | `edwardsp/azcluster` | Source repo for cloud-init artifact downloads |
| `--template <path>` | — | Override embedded ARM template |
| `--what-if` | off | ARM what-if preview, no deploy |
| `--no-wait` | off | Submit ARM and return; finish later with `azcluster resume` |
| `--skip-arm` | off | Re-run post-deploy hooks only (conflicts with `--no-wait`) |

### Useful deploy variants

```bash
# Mixed CPU + GPU pools (both partitions in Slurm)
azcluster deploy --name demo --bastion \
  --pool name=cpu,sku=Standard_HB120rs_v3,count=2,default \
  --pool name=gpu,sku=Standard_ND96isr_H100_v5,count=2

# Rapid test (~7 min, SPOF NFS, no monitoring/accounting)
azcluster deploy --name demo --login-public-ip \
  --shared-storage nfs-scheduler --no-monitoring --no-accounting \
  --pool name=cpu,sku=Standard_D8as_v5,count=1,default

# Fire-and-forget
azcluster deploy --name demo --no-wait --bastion \
  --pool name=gpu,sku=Standard_ND96isr_H100_v5,count=2,default
azcluster resume --name demo     # waits for ARM + runs post-deploy hooks
```

---

## 4. Operator commands (authoritative surface)

| Command | Args / flags | Purpose |
|---|---|---|
| `azcluster version` | — | Print version |
| `azcluster login` | `[--device-code] [--tenant <id>] [--subscription <id>]` | OAuth2 + cache token |
| `azcluster list` | `[--json]` | All azcluster clusters in the subscription (via RG tags) |
| `azcluster deploy` | see §3 | Provision a cluster |
| `azcluster resume` | `--name <name>` | Finish a `--no-wait`/interrupted deploy + run hooks |
| `azcluster status` | `<name>` | Pool capacities + bootstrap probe (READY / in-progress) |
| `azcluster scale` | `<name> <pool> <count>` | Set pool to a **target absolute count**, e.g. `azcluster scale demo gpu 2` (NOT `0/2`) |
| `azcluster ssh` | `<name> [--scheduler\|--host <node>] [-u/--user <ldap>] [--identity <path>] [--no-bastion] [--local-port <p>]` | Interactive shell |
| `azcluster exec` | `<name> [--scheduler\|--host <node>] [-u/--user <ldap>] [-A/--forward-agent] [--no-bastion] -- <cmd...>` | One-shot remote command |
| `azcluster scp` | `<name> [-r] [-p] [-i <key>] [-u <ldap>] [--no-bastion] <SRC>... <DST>` | scp-style; remote path `[node]:path`, empty node = login |
| `azcluster tunnel` | `<name> <local-port> [--scheduler\|--host <node>] [-u <ldap>]` | Local TCP forward through login |
| `azcluster validate` | `<name> [--gpu] [--multi-node] [--no-container] [--partition <p>] [--identity <path>]` | `sinfo` + `srun hostname` + Pyxis import + (opt) 2-node NCCL all-reduce |
| `azcluster logs` | `<name> --component {scheduler\|login\|<node>} [--tail N] [--follow] [--identity <path>]` | Tail `/var/log/azcluster/install.log` or journalctl |
| `azcluster monitor` | `<name>` | Print the Grafana URL |
| `azcluster timings` | `<name> [--last N] [--trend]` | Per-resource ARM deploy times |
| `azcluster delete` | `<name> [--yes]` | Delete the resource group (async) |
| `azcluster purge-cache` | `[--name <n>]` | Drop local manifest cache (default: all) |
| `azcluster purge-kv` | `[--name <n> --location <loc>] [--all] [--yes] [--dry-run]` | Hard-purge soft-deleted azcluster Key Vaults |
| `azcluster user ...` | see below | LDAP user management |

> Global flag: `--no-cache` (on any command) bypasses the local manifest cache and forces a Key Vault round-trip. `--subscription`/`--tenant` exist **only** on `login`.
>
> Hidden internal subcommand `azcluster bastion-proxy --cluster <n> --target {login,scheduler} [--port 22]` is the stdio bridge used by `ssh -o ProxyCommand`; you never invoke it directly.

### User (LDAP) management

```bash
azcluster user add <name> --username <u> [--uid N] [--gid N] [--gecos ""] \
    [--shell /bin/bash] [--ssh-key <path>]... [--admin] [--no-generate-keypair]
azcluster user remove <name> --username <u>
azcluster user list <name>
azcluster user setadmin   <name> --username <u>
azcluster user unsetadmin <name> --username <u>
azcluster user sshkey add    <name> --username <u> --key-file <path>
azcluster user sshkey remove <name> --username <u> --key-file <path>
azcluster user sshkey list   <name> --username <u>
```

- Two default users exist at deploy: `clusteradmin`, `clusteruser`.
- `--user <ldap>` is honoured at every SSH hop (ProxyJump + destination use the same identity).
- `--scheduler --user <ldap>` does **not** work — the scheduler hosts slapd and runs no SSSD client. Use the admin user for scheduler shells; submit jobs from login.

---

## 5. Typical end-to-end session

```bash
azcluster deploy --name demo --bastion \
  --pool name=gpu,sku=Standard_ND96isr_H100_v5,count=2,default
azcluster status demo                                   # wait for both nodes READY
azcluster validate demo --gpu --multi-node              # sinfo + NCCL all-reduce
azcluster ssh demo --user clusteradmin                  # interactive login shell
azcluster exec demo --user clusteradmin -- \
  "sbatch /shared/examples/dgxc-nemo-multinode-smoke.sbatch"
azcluster monitor demo                                  # Grafana URL
azcluster delete demo                                   # tear down (releases GPU quota)
azcluster purge-kv --name demo --location eastus --yes  # release the soft-deleted KV name
```

Example sbatch templates ship on the cluster under `/shared/examples/`. Storage pipeline for big models: HuggingFace → per-cluster blob (`azcp`, once per model) → IB broadcast to per-node NVMe (`azcp-cluster`, every job).

---

## 6. Debugging & the repo

When the CLI alone isn't enough (cloud-init failed, Bicep drift, CLI bug), clone the repo:

```bash
git clone https://github.com/edwardsp/azcluster.git
cd azcluster
```

### Where things live

```
crates/azcluster-cli/src/main.rs   clap CLI definition (every command/flag); dispatch fns: deploy(), ssh(), exec()...
crates/azcluster-cli/src/          user.rs, bastion/ (tunnel.rs ws bridge), cluster_resolver.rs, cluster_state.rs (KV/identity)
crates/azcluster-server/           scheduler control daemon (axum)
crates/azhealthcheck/              per-node health probe (5 checks)
bicep/main.bicep, cluster.bicep    ARM entrypoint + per-cluster orchestration
bicep/modules/                     network, scheduler, login, compute, anf, amlfs, accounting, monitoring, keyvault, storage, bastion
bicep/main.json                    committed transpiled ARM template (CLI embeds it via include_str!)
cloud-init/{scheduler,login,compute}.yaml.tmpl   per-node bootstrap (slurm, pyxis/enroot, NCCL/IB, NVMe RAID, prometheus)
grafana/dashboards/                4 auto-imported dashboards
doc/full-walkthrough-plan.md       canonical version-agnostic end-to-end recipe (every sbatch inlined)
doc/healthchecks.md                azhealthcheck reference
CHANGELOG.md                       per-version history
AGENTS.md                          ⭐ operating manual + a huge per-version "gotchas" log — READ THIS for any non-obvious bug
```

**`AGENTS.md` is the institutional debugging memory.** Almost every cloud-init / Slurm / NCCL / Bastion / Key Vault failure mode has a documented root cause and fix there. Grep it first.

### Live debugging from the CLI (no repo)

```bash
azcluster status <name>                                  # READY vs in-progress, both nodes
azcluster logs <name> --component scheduler --tail 200   # /var/log/azcluster/install.log
azcluster logs <name> --component <compute-hostname> --follow
azcluster exec <name> -- "tail -50 /var/log/azcluster/install.log"   # cloud-init inner script log
azcluster exec <name> --host <node> -- "sinfo -R"        # why a node is drained
azcluster timings <name> --last 1                        # per-resource ARM durations
azcluster validate <name> --gpu --multi-node             # functional smoke test
```

Key truth: `cloud-init status` reports `done` even when an inner `install-*.sh` aborted (the log is piped through `tee`). **Always `tail /var/log/azcluster/install.log` and grep `Error|fail|exit`** rather than trusting cloud-init. `/var/log/azcluster/ready` existing = that node's bootstrap fully completed.

### Build / verify when editing the repo

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
# After editing any bicep/*.bicep, regenerate the committed ARM JSON (CI fails on drift):
az bicep build --file bicep/main.bicep --outfile bicep/main.json
```

### High-frequency operational gotchas (see AGENTS.md for the full list)

- **Capacity:** ND H100 SKUs are intermittent per region. If scheduler/login deploys fail `SkuNotAvailable`, override `--scheduler-sku Standard_HB120rs_v3 --login-sku Standard_HB120rs_v3`.
- **RBAC propagation:** Grafana Admin / KV Secrets Officer / Storage Blob Data Contributor take 5–20 min to propagate; first dashboard import or first `azcp` may 401/403 — retry. Re-running `azcluster deploy` is idempotent (or `--skip-arm` to re-run hooks only).
- **Soft-deleted KVs** block name reuse; `azcluster purge-kv --name <n> --location <loc> --yes` after `delete`.
- **Grafana region:** Managed Grafana isn't available everywhere — use `--grafana-location <supported-region>`.
- **Bastion clusters** (no public IP): `ssh`/`exec`/`scp`/`tunnel`/`validate` auto-route through Bastion; `--no-bastion` opts out.
```
