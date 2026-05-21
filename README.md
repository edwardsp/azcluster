# azcluster

Fast Rust-based Slurm cluster deployer for Azure, with Pyxis + Enroot for containerised AI workloads.

One `az deployment sub create` triggers everything. No daemons on your laptop.

## Status

**Phase 0** (shipped, v0.0.1): scheduler VM + login VM, `azcluster-server` serving `/v1/healthz`.

**Phase 1** (shipped, v0.1.x): VMSS Flex compute pool, ANF shared filesystem, Slurm + Pyxis + Enroot fully wired, `azcluster scale` flips VMSS capacity. Live-validated on `southafricanorth`: `srun --container-image=docker://alpine:latest hostname` works end-to-end.

**Phase 2** (in progress, v0.2.0): multi-pool partitions (CPU + GPU side-by-side), IB/NCCL tunings for NDv5 H100/H200, AMLFS (optional), GPU smoke + NCCL all-reduce validation.

**Phase 3** (in progress, v0.9.0): managed observability infra — opt-in `--monitoring` provisions Azure Monitor Workspace (Managed Prometheus) + Azure Managed Grafana with the AMW linked. Exporters and scrape config land in v0.9.1.

## Prerequisites

- `az` CLI logged in (`az login`)
- `jq`
- An SSH key (`~/.ssh/id_ed25519.pub` or `~/.ssh/id_rsa.pub`)
- Permissions to create resource groups in the target subscription

## Quickstart

Single GPU pool (default, H200, count=0):

```bash
azcluster deploy \
  --name demo \
  --location southafricanorth \
  --resource-group my-rg \
  --login-public-ip
```

Multi-pool (CPU + GPU):

```bash
azcluster deploy \
  --name demo \
  --location southafricanorth \
  --resource-group my-rg \
  --pool name=cpu,sku=Standard_D8as_v5,count=2,default \
  --pool name=gpu,sku=Standard_ND96isr_H200_v5,count=0 \
  --login-public-ip
```

Scale a pool:

```bash
azcluster scale demo gpu 0/2
```

Inspect state:

```bash
azcluster status demo
```

Tear it down:

```bash
azcluster delete demo
```

Provision observability (opt-in, additional cost):

```bash
azcluster deploy --name demo --location southafricanorth --grafana-location uksouth --monitoring ...
azcluster monitor demo                       # prints the Grafana URL
```

`--grafana-location` defaults to `--location`; override when the cluster region does not host Azure Managed Grafana (e.g. `southafricanorth` -> `uksouth`).

Tail install logs (debugging):

```bash
azcluster logs demo --component scheduler --tail 200
azcluster logs demo --component login --follow
azcluster logs demo --component demo-gpu-0001
```

Validate cluster end-to-end:

```bash
azcluster validate demo                # sinfo + srun hostname + Pyxis srun
azcluster validate demo --gpu          # also nvidia-smi via srun (requires a GPU node up)
azcluster validate demo --no-container # skip Pyxis if you don't want to pull alpine
```

SSH in:

```bash
azcluster ssh demo                  # interactive shell on login
azcluster ssh demo --scheduler      # hop through login to scheduler
azcluster exec demo -- sinfo        # one-shot command on login
azcluster exec demo --scheduler -- squeue
sinfo
srun -N1 --container-image=docker://alpine:latest hostname
sbatch /shared/examples/nccl-allreduce.sbatch
```

## Architecture

- **scheduler VM**: runs `slurmctld` + `azcluster-server` (control plane on `:8443`).
- **login VM**: user entry point. Public IP optional (off by default).
- **compute VMSS Flex**: one VMSS per pool; nodes register dynamically with `slurmd --conf-server` and self-tag with `Feature=pool_<name>`, which `slurm.conf`'s `NodeSet` matches into the corresponding partition.
- **Storage**: ANF NFSv4.1 mounted on `/shared` (configurable tier + size). AMLFS planned for v0.2+.
- **Egress**: NAT Gateway on all subnets (no public IPs required on compute/scheduler).

Binaries are distributed via GitHub Releases (built by CI on tag push). Cloud-init on each node fetches the release tarball, verifies SHA256, and starts the relevant systemd unit.

## Repo Layout

```
crates/
  azcluster-core/    domain model (Cluster, NodePool, ...)
  azcluster-server/  control-plane daemon (axum)
  azcluster-cli/     management CLI (clap)
bicep/               main.bicep + cluster.bicep + modules/
cloud-init/          *.yaml.tmpl templates
.github/workflows/   ci.yml + release.yml
research/            local reference checkouts (gitignored)
.sisyphus/           planning artifacts (gitignored)
CHANGELOG.md         every user-visible change, per release
AGENTS.md            instructions for AI agents working on this repo
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

## Releasing

Tag-triggered. The agent maintains `CHANGELOG.md` per [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). To release: move `Unreleased` content under a new `[X.Y.Z] - YYYY-MM-DD` heading, bump `Cargo.toml` versions and the `--azcluster-version` CLI default, commit, then `git tag vX.Y.Z && git push --tags`. CI builds `azcluster-server-x86_64-linux`, `azcluster-cli-x86_64-linux`, `azcluster-cli-aarch64-darwin`, a versioned tarball, `spank_pyxis-vX.Y.Z-x86_64-linux.so`, and `SHA256SUMS`.

## License

TBD.
