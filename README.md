# azcluster

Fast Rust-based Slurm cluster deployer for Azure, with Pyxis + Enroot for containerised AI workloads.

One `az deployment sub create` triggers everything. No daemons on your laptop.

## Status

**Phase 0** (current): scheduler VM + login VM, both running Ubuntu HPC, `azcluster-server` daemon serving `/v1/healthz`. No compute pools, no shared filesystems yet.

**Phase 1** (planned): VMSS Flex compute pools, ANF + AMLFS, Slurm + Pyxis + Enroot fully wired, manual scale via `azcluster-cli`.

**Phase 2** (planned): NDv5 H100, NCCL validation, multi-partition (CPU + GPU pools), production hardening.

## Prerequisites

- `az` CLI logged in (`az login`)
- `jq`
- An SSH key (`~/.ssh/id_ed25519.pub` or `~/.ssh/id_rsa.pub`)
- Permissions to create resource groups in the target subscription

## Quickstart (Phase 0)

```bash
./deploy.sh \
  --name demo \
  --location uksouth \
  --login-public-ip
```

After ~5 min:

```bash
LOGIN_IP=$(az deployment sub show --name <deployment-name> --query properties.outputs.loginPublicIp.value -o tsv)
ssh -L 8443:scheduler:8443 azureuser@$LOGIN_IP
# In another shell:
curl -k https://localhost:8443/v1/healthz
# => {"status":"ok","version":"..."}
```

## Architecture

- **scheduler VM**: runs `slurmctld` (Phase 1+) and the `azcluster-server` daemon (control plane).
- **login VM**: user entry point. Public IP optional (off by default).
- **compute VMSS** (Phase 1+): one VMSS Flex per `NodePool`.
- **Storage** (Phase 1+): ANF (default) and/or AMLFS, configurable tier + size.

Binaries are distributed via GitHub Releases (built by CI on tag push). Cloud-init on the scheduler fetches the release tarball, verifies SHA256, and starts a systemd unit.

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
deploy.sh            wrapper around `az deployment sub create`
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

Tag-triggered. Push a `vX.Y.Z` tag and CI builds `azcluster-server-x86_64-linux`, `azcluster-cli-x86_64-linux`, `azcluster-cli-aarch64-darwin`, a tarball, and `SHA256SUMS`, attaching them to the GitHub Release.

## License

TBD.
