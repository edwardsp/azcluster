mod arm;
mod auth;
mod bastion;
mod cluster_resolver;
mod cluster_state;
mod crypto;
mod deploy_progress;
mod keyvault;
mod timings;
mod user;

use anyhow::{anyhow, bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use cluster_state::{ClusterState, PendingDeploy};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};

static NO_CACHE: AtomicBool = AtomicBool::new(false);

#[derive(Parser)]
#[command(name = "azcluster", version = azcluster_core::VERSION, about = "Manage Slurm clusters on Azure")]
struct Cli {
    /// Bypass the local cluster manifest cache and force a Key Vault round-trip.
    #[arg(long, global = true)]
    no_cache: bool,
    #[command(subcommand)]
    command: CliCommand,
}

#[derive(Subcommand)]
enum CliCommand {
    Version,
    Login(LoginArgs),
    Deploy(Box<DeployArgs>),
    Ssh(ConnectArgs),
    Tunnel(ConnectArgs),
    Scale(ScaleArgs),
    Status(StatusArgs),
    Resume(ResumeArgs),
    Delete(DeleteArgs),
    Exec(ExecArgs),
    Scp(ScpArgs),
    Logs(LogsArgs),
    Validate(ValidateArgs),
    Monitor(MonitorArgs),
    Timings(TimingsArgs),
    TimingsCapture(TimingsCaptureArgs),
    User(UserArgs),
    /// List azcluster-managed clusters in the current subscription (discovered by RG tag).
    List(ListArgs),
    /// Remove cached cluster manifests under ~/.config/azcluster/clusters/.
    PurgeCache(PurgeCacheArgs),
    /// Permanently purge soft-deleted azcluster Key Vaults (bypasses the 7-day retention).
    PurgeKv(PurgeKvArgs),
    /// Internal: stdio bridge through Azure Bastion (used as ssh ProxyCommand).
    #[command(hide = true)]
    BastionProxy(BastionProxyArgs),
}

#[derive(Args)]
struct ListArgs {
    /// Emit JSON array instead of a plain-text table.
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
struct PurgeCacheArgs {
    /// Only purge the cache entry for this cluster (default: purge all).
    #[arg(long)]
    name: Option<String>,
}

#[derive(Args)]
struct PurgeKvArgs {
    /// Cluster name whose KV to target (derives `kv-azc-<hash>`; requires --location).
    #[arg(long)]
    name: Option<String>,
    /// Azure region of the soft-deleted vault (required with --name).
    #[arg(long)]
    location: Option<String>,
    /// Purge every soft-deleted vault matching `kv-azc-*` in this subscription.
    #[arg(long, conflicts_with = "name")]
    all: bool,
    /// Skip interactive confirmation.
    #[arg(long)]
    yes: bool,
    /// List candidates and exit without purging.
    #[arg(long)]
    dry_run: bool,
}

#[derive(Args)]
struct BastionProxyArgs {
    #[arg(long)]
    cluster: String,
    /// login | scheduler
    #[arg(long, default_value = "login")]
    target: String,
    #[arg(long, default_value_t = 22)]
    port: u16,
}

#[derive(Args)]
struct LoginArgs {
    /// Use device code flow instead of opening a browser (for headless / ssh sessions).
    #[arg(long)]
    device_code: bool,
    /// Entra ID tenant ID or domain to log in to. Defaults to the user's home tenant.
    #[arg(long)]
    tenant: Option<String>,
    /// Subscription ID to bind to this session. Defaults to the first visible subscription.
    #[arg(long)]
    subscription: Option<String>,
}

#[derive(Args)]
struct DeployArgs {
    #[arg(long)]
    name: String,
    #[arg(long)]
    location: String,
    #[arg(long)]
    resource_group: Option<String>,
    #[arg(long, default_value_t = false)]
    login_public_ip: bool,
    #[arg(long)]
    allowed_ssh_cidrs: Option<String>,
    #[arg(long, default_value = "v0.24.19")]
    azcluster_version: String,
    #[arg(long, default_value = "edwardsp/azcluster")]
    azcluster_repo: String,
    #[arg(long, default_value = "2404")]
    ubuntu: String,
    #[arg(long, default_value_t = 2)]
    anf_size_tib: u32,
    #[arg(long, default_value = "Standard")]
    anf_tier: String,
    /// AMLFS (Azure Managed Lustre) capacity in TiB. 0 disables AMLFS.
    #[arg(long, default_value_t = 0)]
    amlfs_size_tib: u32,
    /// AMLFS SKU: 40, 125, 250, 500 (MB/s per TiB).
    #[arg(long, default_value = "AMLFS-Durable-Premium-250")]
    amlfs_sku: String,
    /// Availability zone for AMLFS.
    #[arg(long, default_value = "1")]
    amlfs_zone: String,
    /// Compute pool spec, repeatable. Format: name=cpu,sku=Standard_D8s_v5,count=0[,default]
    #[arg(long = "pool")]
    pools: Vec<String>,
    /// Provision Azure Managed Prometheus + Managed Grafana for the cluster (default: on).
    #[arg(long, default_value_t = true, overrides_with = "no_monitoring", action = clap::ArgAction::Set, num_args = 0..=1, default_missing_value = "true")]
    monitoring: bool,
    /// Disable Managed Prometheus + Grafana for rapid test deploys (skips ~3 min provision time).
    #[arg(long, default_value_t = false, overrides_with = "monitoring")]
    no_monitoring: bool,
    /// Provision Slurm accounting (Azure Database for MySQL Flexible Server + slurmdbd) (default: on).
    #[arg(long, default_value_t = true, overrides_with = "no_accounting", action = clap::ArgAction::Set, num_args = 0..=1, default_missing_value = "true")]
    accounting: bool,
    /// Disable Slurm accounting for rapid test deploys.
    #[arg(long, default_value_t = false, overrides_with = "accounting")]
    no_accounting: bool,
    /// Shared filesystem backing /shared. `anf` (default) provisions Azure NetApp Files; `nfs-scheduler` exports /shared from the scheduler VM (test-only, no HA, ~12 min faster).
    #[arg(long, default_value = "anf", value_parser = ["anf", "nfs-scheduler"])]
    shared_storage: String,
    /// Azure region for Managed Grafana when monitoring is on. Defaults to --location. Override when --location does not host Managed Grafana.
    #[arg(long)]
    grafana_location: Option<String>,
    #[arg(long)]
    template: Option<PathBuf>,
    #[arg(long, default_value_t = false)]
    what_if: bool,
    /// Submit ARM with `--no-wait` and return immediately. Run `azcluster resume <name>` afterwards to wait for ARM and run post-deploy hooks (state file, timings JSON, Grafana dashboard import). Without `--no-wait`, deploy blocks and finalizes in one shot.
    #[arg(long, default_value_t = false)]
    no_wait: bool,
    /// Skip the ARM submission entirely and re-run post-deploy hooks only (Grafana dashboard import, timings JSON, state file refresh). Use when the cluster is already healthy and you only want to retry dashboard import. Mutually exclusive with `--no-wait`.
    #[arg(long, default_value_t = false, conflicts_with = "no_wait")]
    skip_arm: bool,
    /// Extra apt packages to install on every node (scheduler, login, compute). Repeatable. Validated against a Debian package name grammar subset (`^[a-z0-9][a-z0-9.+-]*$`). Example: --extra-package git-lfs --extra-package python3.12-venv
    #[arg(long = "extra-package", action = clap::ArgAction::Append, value_parser = parse_pkg_name)]
    extra_packages: Vec<String>,
    /// Provision an Azure Bastion (Standard SKU with native client tunneling) for SSH access without a public IP on login. Opt-in (~$140/month). Enables `azcluster ssh/exec/tunnel` to auto-route via Bastion when login has no public IP.
    #[arg(long, default_value_t = false)]
    bastion: bool,
    /// VM SKU for the scheduler (slurmctld + control daemon). Default `Standard_D8as_v5`.
    #[arg(long, default_value = "Standard_D8as_v5")]
    scheduler_sku: String,
    /// VM SKU for the login VM (operator entry point). Default `Standard_D4as_v5`.
    #[arg(long, default_value = "Standard_D4as_v5")]
    login_sku: String,
    /// Provision a per-cluster storage account with a single container `data` (default: on). Private Endpoint on by default; disable via --storage-public-access. Compute + login VMs authenticate via the cluster UAI through IMDS (Storage Blob Data Contributor).
    #[arg(long, default_value_t = true, overrides_with = "no_storage", action = clap::ArgAction::Set, num_args = 0..=1, default_missing_value = "true")]
    storage: bool,
    /// Disable storage account provisioning (skips ~2 min provision time).
    #[arg(long, default_value_t = false, overrides_with = "storage")]
    no_storage: bool,
    /// Override the auto-generated storage account name (3-24 lowercase alphanumeric, globally unique). Default is deterministic stazc<8-hex-blake3(sub|name|location)>.
    #[arg(long)]
    storage_name: Option<String>,
    /// Enable Hierarchical Namespace (ADLS Gen2) on the storage account. Default off. When true, a `dfs` Private Endpoint sub-resource is also provisioned.
    #[arg(long, default_value_t = false)]
    storage_hns: bool,
    /// Allow public network access on the storage account (skips Private Endpoint + Private DNS provisioning). Default off (PE-only).
    #[arg(long, default_value_t = false)]
    storage_public_access: bool,
    /// Storage account SKU.
    #[arg(long, default_value = "Standard_LRS", value_parser = ["Standard_LRS", "Standard_ZRS", "Standard_GRS", "Standard_RAGRS", "Premium_LRS"])]
    storage_sku: String,
    /// Storage account default access tier. Ignored for Premium SKUs.
    #[arg(long, default_value = "Hot", value_parser = ["Hot", "Cool"])]
    storage_tier: String,
    /// azcp version to install on login + compute (https://github.com/edwardsp/azcp). Pinned per release; override for testing newer azcp builds.
    #[arg(long, default_value = "v0.4.5")]
    azcp_version: String,
}

#[derive(Args)]
struct ResumeArgs {
    /// Cluster name (matches the `--name` used at deploy time).
    #[arg(long)]
    name: String,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct PoolSpec {
    name: String,
    sku: String,
    count: u32,
    #[serde(rename = "default")]
    is_default: bool,
}

fn parse_pkg_name(s: &str) -> Result<String, String> {
    let bytes = s.as_bytes();
    if bytes.is_empty() {
        return Err("empty package name".into());
    }
    let first = bytes[0];
    if !(first.is_ascii_lowercase() || first.is_ascii_digit()) {
        return Err(format!(
            "package name '{s}' must start with [a-z0-9] (Debian package name grammar)"
        ));
    }
    for &b in &bytes[1..] {
        let ok =
            b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'+' || b == b'-';
        if !ok {
            return Err(format!(
                "package name '{s}' contains invalid character; allowed: [a-z0-9.+-]"
            ));
        }
    }
    Ok(s.to_string())
}

fn parse_pool(spec: &str) -> Result<PoolSpec> {
    let mut name = None;
    let mut sku = None;
    let mut count: u32 = 0;
    let mut is_default = false;
    for kv in spec.split(',') {
        let kv = kv.trim();
        if kv.is_empty() {
            continue;
        }
        if kv == "default" {
            is_default = true;
            continue;
        }
        let (k, v) = kv.split_once('=').ok_or_else(|| {
            anyhow!("pool spec '{spec}': expected key=value or 'default', got '{kv}'")
        })?;
        match k.trim() {
            "name" => name = Some(v.trim().to_string()),
            "sku" => sku = Some(v.trim().to_string()),
            "count" => count = v.trim().parse().context("pool count")?,
            "default" => is_default = v.trim().parse::<bool>().context("pool default")?,
            other => bail!("pool spec '{spec}': unknown key '{other}'"),
        }
    }
    Ok(PoolSpec {
        name: name.ok_or_else(|| anyhow!("pool spec '{spec}': missing name="))?,
        sku: sku.ok_or_else(|| anyhow!("pool spec '{spec}': missing sku="))?,
        count,
        is_default,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_pkg_name_accepts_canonical() {
        for ok in ["git-lfs", "python3.12-venv", "libssl3", "g++", "0ad"] {
            assert!(parse_pkg_name(ok).is_ok(), "should accept '{ok}'");
        }
    }

    #[test]
    fn parse_pkg_name_rejects_bad_inputs() {
        for bad in [
            "",
            "-leading-dash",
            "UPPER",
            "with space",
            "semi;rm",
            "$(whoami)",
            "../etc/passwd",
            "name\nwith\nnewline",
        ] {
            assert!(parse_pkg_name(bad).is_err(), "should reject '{bad}'");
        }
    }

    #[test]
    fn parse_pool_minimal() {
        let p = parse_pool("name=cpu,sku=Standard_D8as_v5,count=2").unwrap();
        assert_eq!(p.name, "cpu");
        assert_eq!(p.sku, "Standard_D8as_v5");
        assert_eq!(p.count, 2);
        assert!(!p.is_default);
    }

    #[test]
    fn parse_pool_default_flag() {
        let p = parse_pool("name=gpu,sku=X,count=0,default").unwrap();
        assert!(p.is_default);
    }

    #[test]
    fn parse_pool_missing_name() {
        assert!(parse_pool("sku=X,count=1").is_err());
    }

    #[test]
    fn parse_pool_missing_sku() {
        assert!(parse_pool("name=g,count=1").is_err());
    }

    #[test]
    fn parse_pool_unknown_key() {
        let err = parse_pool("name=g,sku=X,bogus=1").unwrap_err().to_string();
        assert!(err.contains("unknown key 'bogus'"), "{err}");
    }

    #[test]
    fn parse_pool_bad_token() {
        let err = parse_pool("name=g,sku=X,whatever").unwrap_err().to_string();
        assert!(err.contains("expected key=value"), "{err}");
    }

    #[test]
    fn parse_scp_path_local_no_colon() {
        assert_eq!(parse_scp_path("./file"), ScpPath::Local("./file".into()));
        assert_eq!(parse_scp_path("/tmp/x"), ScpPath::Local("/tmp/x".into()));
        assert_eq!(parse_scp_path("file"), ScpPath::Local("file".into()));
    }

    #[test]
    fn parse_scp_path_local_colon_after_slash() {
        assert_eq!(
            parse_scp_path("./a:b"),
            ScpPath::Local("./a:b".into()),
            "colon after slash is local"
        );
        assert_eq!(
            parse_scp_path("/tmp/x:y"),
            ScpPath::Local("/tmp/x:y".into())
        );
    }

    #[test]
    fn parse_scp_path_remote_defaults_to_login() {
        assert_eq!(
            parse_scp_path(":/shared/foo"),
            ScpPath::Remote {
                node: "login".into(),
                path: "/shared/foo".into()
            }
        );
    }

    #[test]
    fn parse_scp_path_remote_named_node() {
        assert_eq!(
            parse_scp_path("scheduler:/etc/slurm/slurm.conf"),
            ScpPath::Remote {
                node: "scheduler".into(),
                path: "/etc/slurm/slurm.conf".into()
            }
        );
        assert_eq!(
            parse_scp_path("vmss-v21a-cpu000000:/tmp/x"),
            ScpPath::Remote {
                node: "vmss-v21a-cpu000000".into(),
                path: "/tmp/x".into()
            }
        );
    }

    fn fixture_state(public_ip: Option<&str>) -> ClusterState {
        ClusterState {
            name: "t".into(),
            subscription_id: "s".into(),
            resource_group: "rg".into(),
            location: "loc".into(),
            admin_username: "azureuser".into(),
            scheduler_private_ip: "10.42.1.4".into(),
            login_public_ip: public_ip.map(String::from),
            anf_mount_ip: Some("10.42.1.4".into()),
            compute_vmss_names: vec![],
            extra_packages: vec![],
            accounting_enabled: false,
            bastion_enabled: public_ip.is_none(),
            bastion_name: None,
            bastion_dns_name: None,
            bastion_resource_id: None,
            storage_enabled: false,
            storage_account_name: None,
            storage_blob_endpoint: None,
            storage_dfs_endpoint: None,
            storage_data_container_url: None,
            storage_hns: false,
            storage_public_access: false,
            azcp_version: None,
        }
    }

    #[test]
    fn resolve_scp_route_login_bastion() {
        let s = fixture_state(None);
        let (proxy, jump, host) = resolve_scp_route(&s, "login", true).unwrap();
        assert_eq!(proxy.as_deref(), Some("login"));
        assert_eq!(jump, None);
        assert_eq!(host, "127.0.0.1");
    }

    #[test]
    fn resolve_scp_route_login_public_ip() {
        let s = fixture_state(Some("1.2.3.4"));
        let (proxy, jump, host) = resolve_scp_route(&s, "login", false).unwrap();
        assert_eq!(proxy, None);
        assert_eq!(jump, None);
        assert_eq!(host, "1.2.3.4");
    }

    #[test]
    fn resolve_scp_route_scheduler_bastion_direct() {
        let s = fixture_state(None);
        let (proxy, jump, host) = resolve_scp_route(&s, "scheduler", true).unwrap();
        assert_eq!(proxy.as_deref(), Some("scheduler"));
        assert_eq!(jump, None);
        assert_eq!(host, "10.42.1.4");
    }

    #[test]
    fn resolve_scp_route_scheduler_via_jump() {
        let s = fixture_state(Some("1.2.3.4"));
        let (proxy, jump, host) = resolve_scp_route(&s, "scheduler", false).unwrap();
        assert_eq!(proxy, None);
        assert_eq!(jump.as_deref(), Some("1.2.3.4"));
        assert_eq!(host, "10.42.1.4");
    }

    #[test]
    fn resolve_scp_route_compute_via_bastion_jump() {
        let s = fixture_state(None);
        let (proxy, jump, host) = resolve_scp_route(&s, "vmss-t-cpu000000", true).unwrap();
        assert_eq!(proxy.as_deref(), Some("login"));
        assert_eq!(jump.as_deref(), Some("127.0.0.1"));
        assert_eq!(host, "vmss-t-cpu000000");
    }

    #[test]
    fn resolve_scp_route_compute_via_public_jump() {
        let s = fixture_state(Some("1.2.3.4"));
        let (proxy, jump, host) = resolve_scp_route(&s, "vmss-t-cpu000000", false).unwrap();
        assert_eq!(proxy, None);
        assert_eq!(jump.as_deref(), Some("1.2.3.4"));
        assert_eq!(host, "vmss-t-cpu000000");
    }

    #[test]
    fn resolve_scp_route_no_route_errors() {
        let s = fixture_state(None);
        assert!(resolve_scp_route(&s, "login", false).is_err());
    }

    fn dv(name: &str, loc: &str) -> DeletedVault {
        DeletedVault {
            name: name.to_string(),
            location: loc.to_string(),
            deletion_date: String::new(),
            scheduled_purge_date: String::new(),
        }
    }

    #[test]
    fn parse_deleted_vault_extracts_fields() {
        let raw = serde_json::json!({
            "name": "kv-azc-abcd1234",
            "properties": {
                "location": "southafricanorth",
                "deletionDate": "2026-05-25T10:00:00Z",
                "scheduledPurgeDate": "2026-06-01T10:00:00Z",
                "vaultId": "/subscriptions/x/.../kv-azc-abcd1234"
            }
        });
        let p = parse_deleted_vault(&raw).unwrap();
        assert_eq!(p.name, "kv-azc-abcd1234");
        assert_eq!(p.location, "southafricanorth");
        assert_eq!(p.deletion_date, "2026-05-25T10:00:00Z");
        assert_eq!(p.scheduled_purge_date, "2026-06-01T10:00:00Z");
    }

    #[test]
    fn parse_deleted_vault_missing_name_rejected() {
        let raw = serde_json::json!({ "properties": { "location": "x" } });
        assert!(parse_deleted_vault(&raw).is_none());
    }

    #[test]
    fn filter_purge_kv_keeps_only_azc_prefix() {
        let all = vec![
            dv("kv-azc-1111", "eastus"),
            dv("kv-other-2222", "eastus"),
            dv("kv-azc-3333", "westus"),
        ];
        let kept = filter_purge_kv_candidates(all, None, None);
        assert_eq!(kept.len(), 2);
        assert!(kept.iter().all(|v| v.name.starts_with("kv-azc-")));
    }

    #[test]
    fn filter_purge_kv_narrows_by_target_name() {
        let all = vec![dv("kv-azc-1111", "eastus"), dv("kv-azc-2222", "eastus")];
        let kept = filter_purge_kv_candidates(all, Some("kv-azc-2222"), None);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].name, "kv-azc-2222");
    }

    #[test]
    fn filter_purge_kv_narrows_by_location_case_insensitive() {
        let all = vec![dv("kv-azc-1111", "eastus"), dv("kv-azc-2222", "westus")];
        let kept = filter_purge_kv_candidates(all, None, Some("EASTUS"));
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].location, "eastus");
    }

    #[test]
    fn filter_purge_kv_name_and_location_both_required_to_match() {
        let all = vec![dv("kv-azc-1111", "eastus"), dv("kv-azc-1111", "westus")];
        let kept = filter_purge_kv_candidates(all, Some("kv-azc-1111"), Some("westus"));
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].location, "westus");
    }
}

#[derive(Args)]
struct ConnectArgs {
    name: String,
    #[arg(long, default_value_t = 8443)]
    local_port: u16,
    #[arg(long)]
    identity: Option<PathBuf>,
    /// Hop through login to the scheduler VM. Mutually exclusive with --host.
    #[arg(long, default_value_t = false, conflicts_with = "host")]
    scheduler: bool,
    /// Hop through login to an arbitrary hostname (typically a compute VMSS instance).
    #[arg(long)]
    host: Option<String>,
    /// Connect as this user instead of the cluster admin (e.g. an LDAP user from `azcluster user add`).
    #[arg(long, short = 'u')]
    user: Option<String>,
    /// Disable auto-routing through Azure Bastion when no login public IP is set.
    #[arg(long, default_value_t = false)]
    no_bastion: bool,
}

#[derive(Args)]
struct ExecArgs {
    name: String,
    #[arg(long)]
    identity: Option<PathBuf>,
    /// Hop through login to the scheduler VM. Mutually exclusive with --host.
    #[arg(long, default_value_t = false, conflicts_with = "host")]
    scheduler: bool,
    /// Hop through login to an arbitrary hostname (typically a compute VMSS instance).
    #[arg(long)]
    host: Option<String>,
    /// Connect as this user instead of the cluster admin (e.g. an LDAP user from `azcluster user add`).
    #[arg(long, short = 'u')]
    user: Option<String>,
    /// Forward the SSH agent (`-A`) so nested `ssh` from the remote host can re-use local keys.
    #[arg(long, short = 'A', default_value_t = false)]
    forward_agent: bool,
    /// Disable auto-routing through Azure Bastion when no login public IP is set.
    #[arg(long, default_value_t = false)]
    no_bastion: bool,
    #[arg(trailing_var_arg = true, required = true)]
    cmd: Vec<String>,
}

#[derive(Args)]
struct ScpArgs {
    name: String,
    /// Recursively copy directories.
    #[arg(short = 'r', long)]
    recursive: bool,
    /// Preserve modification times, access times, and modes.
    #[arg(short = 'p', long)]
    preserve: bool,
    #[arg(short = 'i', long)]
    identity: Option<PathBuf>,
    /// Connect as this user instead of the cluster admin (e.g. an LDAP user from `azcluster user add`).
    #[arg(long, short = 'u')]
    user: Option<String>,
    /// Disable auto-routing through Azure Bastion when no login public IP is set.
    #[arg(long, default_value_t = false)]
    no_bastion: bool,
    /// Source(s) and destination, scp-style. Use `[node]:path` for remote
    /// (node = login (default), scheduler, or compute hostname).
    #[arg(required = true, num_args = 2..)]
    paths: Vec<String>,
}

#[derive(Args)]
struct LogsArgs {
    name: String,
    /// Which node's install log: scheduler, login, or a compute hostname.
    #[arg(long, default_value = "scheduler")]
    component: String,
    /// Tail N lines (0 = full file).
    #[arg(long, default_value_t = 200)]
    tail: u32,
    #[arg(long, default_value_t = false)]
    follow: bool,
    #[arg(long)]
    identity: Option<PathBuf>,
}

#[derive(Args)]
struct ValidateArgs {
    name: String,
    #[arg(long)]
    identity: Option<PathBuf>,
    /// Skip the container (Pyxis) smoke test.
    #[arg(long, default_value_t = false)]
    no_container: bool,
    /// Run nvidia-smi via srun (requires a GPU pool with nodes up).
    #[arg(long, default_value_t = false)]
    gpu: bool,
    /// Run 2-node variants: cross-node hostname, cross-node Pyxis container,
    /// and (with --gpu) a bounded NCCL all-reduce via HPC-X. Requires >=2
    /// idle nodes in the target partition. The NCCL check is tuned for
    /// Azure ND H100 v5 (mlx5_ib + ndv5-topo.xml).
    #[arg(long, default_value_t = false)]
    multi_node: bool,
    /// Slurm partition to target (defaults to the cluster's default partition).
    #[arg(long)]
    partition: Option<String>,
}

#[derive(Args)]
struct ScaleArgs {
    name: String,
    pool: String,
    count: u32,
}

#[derive(Args)]
struct StatusArgs {
    name: String,
}

#[derive(Args)]
struct DeleteArgs {
    name: String,
    #[arg(long, default_value_t = false)]
    yes: bool,
}

#[derive(Args)]
struct MonitorArgs {
    name: String,
}

#[derive(Args)]
struct TimingsArgs {
    name: String,
    #[arg(long, default_value_t = 1)]
    last: usize,
    #[arg(long, default_value_t = false)]
    trend: bool,
}

#[derive(Args)]
struct TimingsCaptureArgs {
    name: String,
    deployment: String,
    resource_group: String,
    #[arg(long, default_value = "anf")]
    shared_storage: String,
}

#[derive(Args)]
struct UserArgs {
    #[command(subcommand)]
    cmd: UserCmd,
}

#[derive(Subcommand)]
enum UserCmd {
    /// Add a POSIX user to the cluster directory (LDAP). Auto-generates a per-user SSH keypair stored at ~/.azcluster/keys/<cluster>-<username> on this machine; public key is added to the user's LDAP sshPublicKey (alongside any --ssh-key files). Private key is NEVER copied to Key Vault — only the operator who runs `user add` gets it on their laptop.
    Add {
        cluster: String,
        #[arg(long)]
        username: String,
        #[arg(long)]
        uid: Option<u32>,
        #[arg(long)]
        gid: Option<u32>,
        #[arg(long, default_value = "")]
        gecos: String,
        #[arg(long, default_value = "/bin/bash")]
        shell: String,
        /// SSH public key files (repeatable). One key per file. Added to the user's LDAP sshPublicKey alongside the auto-generated keypair.
        #[arg(long = "ssh-key")]
        ssh_keys: Vec<PathBuf>,
        /// Grant LDAP admin privileges (member of `cn=cluster-admins`, gets sudo via /etc/sudoers.d/cluster-admins).
        #[arg(long)]
        admin: bool,
        /// Skip generating a per-user keypair on this machine. Use only when --ssh-key supplies all needed keys.
        #[arg(long)]
        no_generate_keypair: bool,
    },
    /// Remove a user.
    Remove {
        cluster: String,
        #[arg(long)]
        username: String,
    },
    /// List users (shows admin status from cluster-admins LDAP group).
    List { cluster: String },
    /// Promote an LDAP user to admin (add to cluster-admins).
    Setadmin {
        cluster: String,
        #[arg(long)]
        username: String,
    },
    /// Demote an LDAP user from admin (remove from cluster-admins).
    Unsetadmin {
        cluster: String,
        #[arg(long)]
        username: String,
    },
    /// Manage authorized SSH keys for a user.
    Sshkey {
        #[command(subcommand)]
        cmd: SshkeyCmd,
    },
}

#[derive(Subcommand)]
enum SshkeyCmd {
    Add {
        cluster: String,
        #[arg(long)]
        username: String,
        #[arg(long)]
        key_file: PathBuf,
    },
    Remove {
        cluster: String,
        #[arg(long)]
        username: String,
        #[arg(long)]
        key_file: PathBuf,
    },
    List {
        cluster: String,
        #[arg(long)]
        username: String,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    NO_CACHE.store(cli.no_cache, Ordering::Relaxed);
    match cli.command {
        CliCommand::Version => {
            println!("azcluster {}", azcluster_core::VERSION);
            Ok(())
        }
        CliCommand::Login(args) => login(args),
        CliCommand::Deploy(args) => deploy(*args),
        CliCommand::Ssh(args) => ssh(args),
        CliCommand::Tunnel(args) => tunnel(args),
        CliCommand::Scale(args) => scale(args),
        CliCommand::Status(args) => status(args),
        CliCommand::Resume(args) => resume(args),
        CliCommand::Delete(args) => delete(args),
        CliCommand::Exec(args) => exec(args),
        CliCommand::Scp(args) => scp(args),
        CliCommand::Logs(args) => logs(args),
        CliCommand::Validate(args) => validate(args),
        CliCommand::Monitor(args) => monitor(args),
        CliCommand::Timings(args) => timings(args),
        CliCommand::TimingsCapture(args) => {
            timings::capture(
                &arm_client()?,
                &args.name,
                &args.deployment,
                &args.resource_group,
                &args.shared_storage,
            )?;
            Ok(())
        }
        CliCommand::User(args) => user_dispatch(args),
        CliCommand::List(args) => list(args),
        CliCommand::PurgeCache(args) => purge_cache(args),
        CliCommand::PurgeKv(args) => purge_kv(args),
        CliCommand::BastionProxy(args) => bastion_proxy(args),
    }
}

const EMBEDDED_MAIN_TEMPLATE: &str = include_str!("../../../bicep/main.json");

fn resolve_template(explicit: Option<PathBuf>) -> Result<serde_json::Value> {
    if let Some(p) = explicit {
        if !p.exists() {
            bail!("template {} not found", p.display());
        }
        let ext = p
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| s.to_ascii_lowercase());
        if ext.as_deref() != Some("json") {
            bail!(
                "--template {} must be an ARM JSON file (.json). Transpile bicep with: az bicep build --file <input>.bicep --outfile <output>.json",
                p.display()
            );
        }
        let raw = std::fs::read_to_string(&p)
            .with_context(|| format!("read template {}", p.display()))?;
        return serde_json::from_str(&raw)
            .with_context(|| format!("parse ARM JSON {}", p.display()));
    }
    serde_json::from_str(EMBEDDED_MAIN_TEMPLATE).context("parse embedded main.json")
}

/// Get a valid Azure access token from the cache populated by `azcluster login`.
fn get_access_token() -> Result<String> {
    let cache = auth::TokenCache::load()?;
    let account = cache
        .accounts
        .values()
        .max_by_key(|a| a.expires_at)
        .ok_or_else(|| anyhow!("not logged in to Azure. Run: azcluster login"))?;
    let mut provider =
        auth::TokenProvider::new(account.subscription_id.clone(), account.tenant_id.clone())?;
    provider.get_token()
}

fn current_subscription_id() -> Result<String> {
    let cache = auth::TokenCache::load()?;
    let account = cache
        .accounts
        .values()
        .filter(|a| !a.subscription_id.is_empty())
        .max_by_key(|a| a.expires_at)
        .ok_or_else(|| anyhow!("no active subscription. Run: azcluster login"))?;
    Ok(account.subscription_id.clone())
}

fn arm_client() -> Result<arm::client::ArmClient> {
    let token = get_access_token()?;
    let sub_id = current_subscription_id()?;
    let client = arm::client::ArmClient::new(token, sub_id)?;
    Ok(client.with_refresh_callback(get_access_token))
}

fn get_vault_token() -> Result<String> {
    let cache = auth::TokenCache::load()?;
    let account = cache
        .accounts
        .values()
        .max_by_key(|a| a.expires_at)
        .ok_or_else(|| anyhow!("not logged in to Azure. Run: azcluster login"))?;
    let mut provider =
        auth::TokenProvider::new(account.subscription_id.clone(), account.tenant_id.clone())?;
    provider.get_vault_token()
}

fn resolve_cluster(name: &str) -> Result<ClusterState> {
    let arm = arm_client()?;
    let vault_token = get_vault_token()?;
    let no_cache = NO_CACHE.load(Ordering::Relaxed);
    let resolver = cluster_resolver::Resolver::new(&arm, vault_token, no_cache);
    let resolved = resolver.resolve(name)?;
    if resolved.source == cluster_resolver::ResolveSource::KeyVault {
        eprintln!(
            "==> Using cluster '{name}' in subscription {} (from Key Vault)",
            arm.subscription_id()
        );
    }
    Ok(resolved.state)
}

pub(crate) fn resolve_identity(
    explicit: Option<&Path>,
    cluster_name: &str,
) -> Result<std::path::PathBuf> {
    if let Some(p) = explicit {
        return Ok(p.to_path_buf());
    }
    fetch_admin_private_key(cluster_name)
}

/// Identity resolution for commands with a `--user` flag.
/// - explicit `-i`            → always honoured
/// - connect_user == admin    → KV admin key (same as `resolve_identity`)
/// - connect_user != admin    → `None`, letting ssh fall back to the agent /
///   `~/.ssh/id_*`. The admin KV key would fail for LDAP users because their
///   `authorized_keys` (via SSSD `sshPublicKey`) contains the pubkey the
///   operator enrolled with `azcluster user {add,sshkey add} --ssh-key`,
///   not the admin ed25519.
pub(crate) fn resolve_identity_for_user(
    explicit: Option<&Path>,
    cluster_name: &str,
    connect_user: &str,
    admin_user: &str,
) -> Result<Option<std::path::PathBuf>> {
    if let Some(p) = explicit {
        return Ok(Some(p.to_path_buf()));
    }
    if connect_user == admin_user {
        return Ok(Some(fetch_admin_private_key(cluster_name)?));
    }
    // v0.24: per-user keypair auto-generated by `azcluster user add` lives at
    // ~/.azcluster/keys/<cluster>-<user>. If present locally, use it.
    if let Some(home) = dirs::home_dir() {
        let per_user_key = home
            .join(".azcluster")
            .join("keys")
            .join(format!("{}-{}", cluster_name, connect_user));
        if per_user_key.exists() {
            return Ok(Some(per_user_key));
        }
    }
    // v0.24: default LDAP users `clusteradmin` and `clusteruser` have their
    // sshPublicKey seeded with the admin pubkey at deploy time. Fall back to
    // the admin private key so `azcluster ssh --user clusteradmin` works out
    // of the box from the deployer's laptop.
    if connect_user == "clusteradmin" || connect_user == "clusteruser" {
        if let Ok(p) = fetch_admin_private_key(cluster_name) {
            return Ok(Some(p));
        }
    }
    Ok(None)
}

fn fetch_admin_private_key(name: &str) -> Result<std::path::PathBuf> {
    let dir = dirs::home_dir()
        .ok_or_else(|| anyhow!("could not determine HOME"))?
        .join(".azcluster")
        .join("keys");
    let key_path = dir.join(name);
    if key_path.exists() {
        return Ok(key_path);
    }
    std::fs::create_dir_all(&dir).with_context(|| format!("mkdir {}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).ok();
    }

    if let Some(local_secrets) = cluster_state::ClusterSecrets::load_optional(name)? {
        let privkey = &local_secrets.admin_ssh_private_key;
        if !privkey.is_empty() {
            std::fs::write(&key_path, privkey)
                .with_context(|| format!("write {}", key_path.display()))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600))
                    .with_context(|| format!("chmod 0600 {}", key_path.display()))?;
            }
            eprintln!(
                "==> materialised admin ssh key for cluster '{name}' (from local secrets file) -> {}",
                key_path.display()
            );
            return Ok(key_path);
        }
    }

    let arm = arm_client()?;
    let vault_token = get_vault_token()?;
    let resolver = cluster_resolver::Resolver::new(&arm, vault_token.clone(), false);
    let resolved = resolver.resolve(name)?;
    let kv_name = arm
        .get_resource_group_tags(&resolved.state.resource_group)?
        .get(cluster_resolver::TAG_KV)
        .ok_or_else(|| {
            anyhow!(
                "RG '{}' missing tag {}; cluster predates v0.22",
                resolved.state.resource_group,
                cluster_resolver::TAG_KV
            )
        })?
        .clone();
    let kv = keyvault::client::KeyVaultClient::new(keyvault::vault_uri(&kv_name), vault_token)?;
    let bundle = kv
        .get_secret(cluster_resolver::SECRETS_BUNDLE)
        .with_context(|| format!("fetch {} from {kv_name}", cluster_resolver::SECRETS_BUNDLE))?;
    let secrets: cluster_state::ClusterSecrets =
        serde_json::from_str(&bundle.value).context("parse secrets-bundle JSON")?;
    if secrets.admin_ssh_private_key.is_empty() {
        bail!("secrets-bundle in vault '{kv_name}' has no admin_ssh_private_key");
    }
    std::fs::write(&key_path, &secrets.admin_ssh_private_key)
        .with_context(|| format!("write {}", key_path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("chmod 0600 {}", key_path.display()))?;
    }
    eprintln!(
        "==> materialised admin ssh key for cluster '{name}' -> {}",
        key_path.display()
    );
    Ok(key_path)
}

/// Append OpenSSH `ProxyCommand` args that jump via `jump_target` using the explicit
/// `identity` key. Replaces `-J <jump_target>` because OpenSSH's `-J` does NOT propagate
/// the outer `-i` to the inner ssh — the jump hop falls back to the agent / `~/.ssh/id_*`,
/// which is empty in v0.22 (admin key lives in `~/.azcluster/keys/<cluster>`).
pub(crate) fn add_ssh_jump_with_identity(
    cmd: &mut Command,
    identity: &std::path::Path,
    jump_target: &str,
) {
    let pc = format!(
        "ProxyCommand=ssh -W %h:%p -i {} -o IdentitiesOnly=yes -o StrictHostKeyChecking=accept-new -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR {}",
        identity.display(),
        jump_target,
    );
    cmd.args(["-o", &pc]);
}

/// Append a jump hop. With an explicit identity, uses the v0.22.1 ProxyCommand
/// pattern to propagate `-i` to the inner ssh (OpenSSH `-J` does NOT do that).
/// Without an identity, bare `-J` is correct — the inner ssh falls back to the
/// same default key discovery (agent / `~/.ssh/id_*`) the outer ssh uses.
pub(crate) fn add_ssh_jump(
    cmd: &mut Command,
    identity: Option<&std::path::Path>,
    jump_target: &str,
) {
    match identity {
        Some(key) => add_ssh_jump_with_identity(cmd, key, jump_target),
        None => {
            cmd.args(["-J", jump_target]);
        }
    }
}

fn login(args: LoginArgs) -> Result<()> {
    let tenant = args.tenant.as_deref();

    if let Some(want) = args.subscription.as_deref() {
        if let Some(account) = auth::token_provider::try_rebind_cached(want, tenant)? {
            println!("Reused cached credentials for {}", account.username);
            println!("Active subscription: {want}");
            println!("Tenant: {}", account.tenant_id);
            println!("Token cached at: ~/.azure/azcli_tokens.json");
            return Ok(());
        }
    }

    let account = if args.device_code {
        auth::token_provider::run_device_code_login(tenant)?
    } else {
        auth::token_provider::run_interactive_login(tenant)?
    };

    println!("Authenticated as: {}", account.username);

    let subs = auth::list_subscriptions(&account.access_token)?;
    if subs.is_empty() {
        bail!("Login succeeded but no subscriptions are visible to this account");
    }

    let chosen = if let Some(want) = args.subscription.as_deref() {
        subs.iter()
            .find(|s| s.subscription_id == want)
            .ok_or_else(|| anyhow!("subscription {want} not visible to this account"))?
            .clone()
    } else {
        subs[0].clone()
    };

    let bound =
        auth::token_provider::bind_subscription(&account.tenant_id, &chosen.subscription_id)?;

    println!(
        "Active subscription: {} ({})",
        chosen.subscription_id,
        chosen.display_name.as_deref().unwrap_or("?")
    );
    println!("Tenant: {}", bound.tenant_id);
    println!("Token cached at: ~/.azure/azcli_tokens.json");

    if subs.len() > 1 && args.subscription.is_none() {
        println!();
        println!(
            "{} subscriptions visible. Pass --subscription <id> to pick a different one:",
            subs.len()
        );
        for s in &subs {
            println!(
                "  {}  {}",
                s.subscription_id,
                s.display_name.as_deref().unwrap_or("?")
            );
        }
    }

    Ok(())
}

fn deploy(args: DeployArgs) -> Result<()> {
    let template = resolve_template(args.template.clone())?;

    let sub_id = current_subscription_id()?;
    let key_vault_name = crypto::derive_kv_name(&sub_id, &args.name, &args.location);
    let (deployer_oid, deployer_ptype) = current_principal()?;
    eprintln!(
        "==> deployer principal: {deployer_oid} ({deployer_ptype}) -> will receive Key Vault Secrets Officer + (when monitoring) Grafana Admin"
    );
    eprintln!("==> per-cluster Key Vault: {key_vault_name}");

    let storage_enabled = args.storage && !args.no_storage;
    let storage_account_name = if storage_enabled {
        match args.storage_name.as_deref() {
            Some(name) => {
                crypto::validate_storage_account_name(name)?;
                name.to_string()
            }
            None => crypto::derive_storage_account_name(&sub_id, &args.name, &args.location),
        }
    } else {
        String::new()
    };
    if storage_enabled {
        eprintln!(
            "==> per-cluster storage account: {storage_account_name} (hns={}, public_access={})",
            args.storage_hns, args.storage_public_access
        );
    }

    let allowed_cidrs_json: serde_json::Value = match args.allowed_ssh_cidrs.as_deref() {
        Some(csv) if !csv.is_empty() => serde_json::Value::Array(
            csv.split(',')
                .filter(|s| !s.is_empty())
                .map(|s| serde_json::Value::String(s.to_string()))
                .collect(),
        ),
        _ => serde_json::Value::Array(vec![]),
    };

    let resolved_rg = args
        .resource_group
        .clone()
        .unwrap_or_else(|| format!("rg-azcluster-{}", args.name));

    if args.resource_group.is_some() {
        let client = arm_client()?;
        let tags = serde_json::json!({
            "azcluster": "true",
            "azcluster-name": args.name,
        });
        client
            .create_resource_group(&resolved_rg, &args.location, Some(tags))
            .with_context(|| format!("create resource group {}", resolved_rg))?;
    }

    let deployment_name = if args.skip_arm {
        let pending = PendingDeploy::load_optional(&args.name)?.ok_or_else(|| {
            anyhow!(
                "--skip-arm: no pending deploy marker for '{}' (~/.config/azcluster/clusters/{}-pending.toml). \
                 The marker is needed to identify the most recent ARM deployment to finalize against. \
                 If the cluster was already finalized, delete and redeploy, or run `azcluster deploy --name {}` without --skip-arm.",
                args.name, args.name, args.name
            )
        })?;
        eprintln!(
            "==> --skip-arm: reusing pending deployment_name '{}'",
            pending.deployment_name
        );
        pending.deployment_name
    } else {
        format!("azcluster-{}-{}", args.name, utc_stamp())
    };

    let pools: Vec<PoolSpec> = if args.pools.is_empty() {
        vec![PoolSpec {
            name: "gpu".into(),
            sku: "Standard_ND96isr_H100_v5".into(),
            count: 0,
            is_default: true,
        }]
    } else {
        args.pools
            .iter()
            .map(|s| parse_pool(s))
            .collect::<Result<_>>()?
    };
    let pools_value = serde_json::to_value(&pools).context("encode pools")?;

    let monitoring_enabled = args.monitoring && !args.no_monitoring;
    let accounting_enabled = args.accounting && !args.no_accounting;
    let existing_secrets = cluster_state::ClusterSecrets::load_optional(&args.name)?;
    if existing_secrets.is_some() {
        eprintln!(
            "==> reusing persisted secrets for cluster '{}' (re-invocation safe)",
            args.name
        );
    }
    let mysql_password = if accounting_enabled {
        match existing_secrets
            .as_ref()
            .and_then(|s| s.mysql_admin_password.clone())
        {
            Some(p) => p,
            None => gen_mysql_password()?,
        }
    } else {
        String::new()
    };
    let ldap_password = match existing_secrets
        .as_ref()
        .map(|s| s.ldap_admin_password.clone())
    {
        Some(p) => p,
        None => gen_mysql_password()?,
    };

    let (admin_ssh_public_key, admin_ssh_private_key) = match existing_secrets.as_ref() {
        Some(s) if !s.admin_ssh_public_key.is_empty() && !s.admin_ssh_private_key.is_empty() => (
            s.admin_ssh_public_key.clone(),
            s.admin_ssh_private_key.clone(),
        ),
        _ => {
            let kp = crypto::generate_admin_keypair(&format!("azcluster-{}", args.name))?;
            eprintln!(
                "==> generated fresh ed25519 admin keypair for cluster '{}'",
                args.name
            );
            (kp.public_openssh, kp.private_openssh_pem)
        }
    };

    let secrets_to_save = cluster_state::ClusterSecrets {
        ldap_admin_password: ldap_password.clone(),
        mysql_admin_password: if accounting_enabled {
            Some(mysql_password.clone())
        } else {
            existing_secrets
                .as_ref()
                .and_then(|s| s.mysql_admin_password.clone())
        },
        admin_ssh_public_key: admin_ssh_public_key.clone(),
        admin_ssh_private_key: admin_ssh_private_key.clone(),
    };
    let secrets_path = secrets_to_save.save(&args.name)?;
    eprintln!("==> saved cluster secrets -> {}", secrets_path.display());

    use serde_json::{json, Value};
    let params: Vec<(&str, Value)> = vec![
        ("clusterName", json!(args.name)),
        ("location", json!(args.location)),
        ("sshPublicKey", json!(admin_ssh_public_key.trim())),
        ("loginPublicIp", json!(args.login_public_ip)),
        ("allowedSshCidrs", allowed_cidrs_json),
        ("azclusterVersion", json!(args.azcluster_version)),
        ("azclusterRepo", json!(args.azcluster_repo)),
        ("ubuntuSku", json!(args.ubuntu)),
        (
            "existingResourceGroup",
            json!(args.resource_group.clone().unwrap_or_default()),
        ),
        ("anfSizeTiB", json!(args.anf_size_tib)),
        ("anfServiceLevel", json!(args.anf_tier)),
        ("amlfsSizeTiB", json!(args.amlfs_size_tib)),
        ("amlfsSkuName", json!(args.amlfs_sku)),
        ("amlfsZone", json!(args.amlfs_zone)),
        ("pools", pools_value),
        ("enableMonitoring", json!(monitoring_enabled)),
        ("sharedStorageMode", json!(args.shared_storage)),
        ("enableAccounting", json!(accounting_enabled)),
        ("mysqlAdminPassword", json!(mysql_password)),
        ("ldapAdminPassword", json!(ldap_password)),
        (
            "grafanaLocation",
            json!(args
                .grafana_location
                .clone()
                .unwrap_or_else(|| args.location.clone())),
        ),
        ("extraPackages", json!(args.extra_packages.join(" "))),
        ("enableBastion", json!(args.bastion)),
        ("schedulerSku", json!(args.scheduler_sku)),
        ("loginSku", json!(args.login_sku)),
        ("keyVaultName", json!(key_vault_name)),
        ("enableStorage", json!(storage_enabled)),
        ("storageAccountName", json!(storage_account_name)),
        ("storageHns", json!(args.storage_hns)),
        ("storagePublicAccess", json!(args.storage_public_access)),
        ("storageSku", json!(args.storage_sku)),
        ("storageAccessTier", json!(args.storage_tier)),
        ("azcpVersion", json!(args.azcp_version)),
        ("deployerPrincipalId", json!(deployer_oid)),
        ("deployerPrincipalType", json!(deployer_ptype)),
    ];

    if monitoring_enabled {
        eprintln!("==> monitoring enabled: AMG Grafana Admin role will be granted via ARM");
    }

    let mut params_obj = serde_json::Map::new();
    for (k, v) in params {
        params_obj.insert(k.to_string(), json!({ "value": v }));
    }
    let params_json = Value::Object(params_obj);

    let client = arm_client()?;

    if args.what_if {
        eprintln!("==> ARM whatIf deployment '{}'", deployment_name);
        let result = client
            .whatif_subscription_deployment(&deployment_name, &args.location, template, params_json)
            .context("whatIf submission failed")?;
        let pretty = serde_json::to_string_pretty(&result).unwrap_or_else(|_| result.to_string());
        println!("{pretty}");
        return Ok(());
    }

    let pending = PendingDeploy {
        cluster: args.name.clone(),
        deployment_name: deployment_name.clone(),
        resource_group: resolved_rg.clone(),
        started_at: utc_iso8601(),
        monitoring_enabled,
        accounting_enabled,
        shared_storage: args.shared_storage.clone(),
        grafana_location: args.grafana_location.clone(),
        extra_packages: args.extra_packages.clone(),
        bastion_enabled: args.bastion,
        storage_enabled,
        storage_account_name: if storage_enabled {
            Some(storage_account_name.clone())
        } else {
            None
        },
        storage_hns: args.storage_hns,
        storage_public_access: args.storage_public_access,
        azcp_version: Some(args.azcp_version.clone()),
    };
    if !args.skip_arm {
        let pending_path = pending.save()?;
        eprintln!("==> saved pending deploy -> {}", pending_path.display());
    } else {
        eprintln!("==> --skip-arm: leaving existing pending marker intact");
    }

    if !args.skip_arm {
        eprintln!(
            "==> ARM create deployment '{}'{}",
            deployment_name,
            if args.no_wait { " (--no-wait)" } else { "" }
        );
        client
            .create_subscription_deployment(&deployment_name, &args.location, template, params_json)
            .context("ARM deployment submission failed")?;
    } else {
        eprintln!(
            "==> --skip-arm: bypassing ARM submission for '{}'; running post-deploy hooks only",
            args.name
        );
    }

    if args.no_wait {
        eprintln!(
            "==> ARM deployment '{}' submitted. Run `azcluster resume --name {}` to wait for completion and run post-deploy hooks.",
            deployment_name, args.name
        );
        eprintln!("==> Track progress with: azcluster status {}", args.name);
        return Ok(());
    }

    if !args.skip_arm {
        eprintln!(
            "==> waiting for ARM deployment '{}' to complete...",
            deployment_name
        );
        let mut progress = deploy_progress::Renderer::new();
        let final_state = client
            .wait_for_deployment_completion_with_progress(&deployment_name, &mut |ops| {
                progress.render(ops);
            })
            .context("polling ARM deployment")?;
        progress.finish();
        let state_str = final_state
            .get("properties")
            .and_then(|p| p.get("provisioningState"))
            .and_then(|s| s.as_str())
            .unwrap_or("");
        if state_str != "Succeeded" {
            bail!(
                "ARM deployment '{}' ended in state {state_str}. Run `azcluster delete --name {}` to tear down.",
                deployment_name, args.name
            );
        }
    }

    finalize_deploy(
        &args,
        &deployment_name,
        &resolved_rg,
        &sub_id,
        accounting_enabled,
        monitoring_enabled,
        &ldap_password,
        &mysql_password,
        existing_secrets.as_ref(),
    )?;
    PendingDeploy::delete(&args.name)?;
    Ok(())
}

fn resume(args: ResumeArgs) -> Result<()> {
    let pending = PendingDeploy::load_optional(&args.name)?.ok_or_else(|| {
        anyhow!(
            "no pending deploy for cluster '{}'. If `azcluster deploy` already finalized successfully, there is nothing to do. Otherwise run `azcluster deploy --name {} …` first.",
            args.name, args.name
        )
    })?;

    eprintln!(
        "==> resuming pending deploy {} (started {})",
        pending.deployment_name, pending.started_at
    );

    let sub_id = current_subscription_id()?;

    let terminal = poll_deployment_until_terminal(&pending.deployment_name)?;
    match terminal.as_str() {
        "Succeeded" => {
            let secrets = cluster_state::ClusterSecrets::load_optional(&args.name)?
                .ok_or_else(|| {
                    anyhow!(
                        "pending deploy resumed but secrets file ~/.config/azcluster/clusters/{}-secrets.toml is missing; cannot recover ARM secure parameters",
                        args.name
                    )
                })?;
            let ldap_password = secrets.ldap_admin_password.clone();
            let mysql_password = secrets.mysql_admin_password.clone().unwrap_or_default();
            let location = arm_client()?
                .get_resource_group(&pending.resource_group)
                .with_context(|| format!("get resource group {}", pending.resource_group))?
                .get("location")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    anyhow!("could not resolve location for {}", pending.resource_group)
                })?
                .to_string();
            let synthetic_args = DeployArgs {
                name: args.name.clone(),
                location,
                resource_group: Some(pending.resource_group.clone()),
                login_public_ip: false,
                allowed_ssh_cidrs: None,
                azcluster_version: String::new(),
                azcluster_repo: String::new(),
                ubuntu: String::new(),
                anf_size_tib: 0,
                anf_tier: String::new(),
                amlfs_size_tib: 0,
                amlfs_sku: String::new(),
                amlfs_zone: String::new(),
                pools: Vec::new(),
                monitoring: pending.monitoring_enabled,
                no_monitoring: !pending.monitoring_enabled,
                accounting: pending.accounting_enabled,
                no_accounting: !pending.accounting_enabled,
                shared_storage: pending.shared_storage.clone(),
                grafana_location: pending.grafana_location.clone(),
                template: None,
                what_if: false,
                no_wait: false,
                skip_arm: false,
                extra_packages: pending.extra_packages.clone(),
                bastion: pending.bastion_enabled,
                scheduler_sku: String::new(),
                login_sku: String::new(),
                storage: pending.storage_enabled,
                no_storage: !pending.storage_enabled,
                storage_name: pending.storage_account_name.clone(),
                storage_hns: pending.storage_hns,
                storage_public_access: pending.storage_public_access,
                storage_sku: "Standard_LRS".into(),
                storage_tier: "Hot".into(),
                azcp_version: pending
                    .azcp_version
                    .clone()
                    .unwrap_or_else(|| "v0.4.5".into()),
            };
            finalize_deploy(
                &synthetic_args,
                &pending.deployment_name,
                &pending.resource_group,
                &sub_id,
                pending.accounting_enabled,
                pending.monitoring_enabled,
                &ldap_password,
                &mysql_password,
                Some(&secrets),
            )?;
            PendingDeploy::delete(&args.name)?;
            eprintln!("==> resume complete");
            Ok(())
        }
        other => {
            bail!(
                "ARM deployment {} ended in state '{}'. Run `azcluster delete {}` (or `az group delete --name {}`) and retry, then remove ~/.config/azcluster/clusters/{}-pending.toml.",
                pending.deployment_name, other, args.name, pending.resource_group, args.name
            )
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn upload_cluster_to_keyvault(
    kv_name: &str,
    state: &ClusterState,
    secrets: &cluster_state::ClusterSecrets,
) -> Result<()> {
    let vault_uri = format!("https://{kv_name}.vault.azure.net");
    let token = get_vault_token()?;
    let kv = keyvault::client::KeyVaultClient::new(vault_uri, token)?;
    let manifest = serde_json::to_string(state).context("serialize cluster manifest")?;
    let bundle = serde_json::to_string(secrets).context("serialize secrets bundle")?;
    kv.set_secret(
        cluster_resolver::MANIFEST_SECRET,
        &manifest,
        Some("application/json"),
    )?;
    kv.set_secret(
        cluster_resolver::SECRETS_BUNDLE,
        &bundle,
        Some("application/json"),
    )?;
    Ok(())
}

fn tag_resource_group_for_cluster(
    arm: &arm::client::ArmClient,
    rg_name: &str,
    cluster_name: &str,
    kv_name: &str,
    version: &str,
) -> Result<()> {
    let mut tags = std::collections::HashMap::new();
    tags.insert(
        cluster_resolver::TAG_MANAGED.to_string(),
        "true".to_string(),
    );
    tags.insert(
        cluster_resolver::TAG_NAME.to_string(),
        cluster_name.to_string(),
    );
    tags.insert(cluster_resolver::TAG_KV.to_string(), kv_name.to_string());
    tags.insert(
        cluster_resolver::TAG_VERSION.to_string(),
        version.to_string(),
    );
    tags.insert(
        cluster_resolver::TAG_DEPLOYED_AT.to_string(),
        chrono::Utc::now().to_rfc3339(),
    );
    arm.patch_resource_group_tags(rg_name, tags)
        .with_context(|| format!("patch tags on {rg_name}"))
}

#[allow(clippy::too_many_arguments)]
fn finalize_deploy(
    args: &DeployArgs,
    deployment_name: &str,
    resolved_rg: &str,
    sub_id: &str,
    accounting_enabled: bool,
    _monitoring_enabled: bool,
    ldap_password: &str,
    mysql_password: &str,
    existing_secrets: Option<&cluster_state::ClusterSecrets>,
) -> Result<()> {
    let deployment = arm_client()?.get_deployment(deployment_name)?;
    let outputs = deployment
        .get("properties")
        .and_then(|p| p.get("outputs"))
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));

    let pick = |k: &str| {
        outputs
            .get(k)
            .and_then(|v| v.get("value"))
            .and_then(|v| v.as_str())
            .map(String::from)
    };

    let scheduler_private_ip = pick("schedulerPrivateIp")
        .ok_or_else(|| anyhow!("deployment did not return schedulerPrivateIp"))?;
    let login_public_ip = pick("loginPublicIp").filter(|s| !s.is_empty());

    let state = ClusterState {
        name: args.name.clone(),
        subscription_id: sub_id.to_string(),
        resource_group: resolved_rg.to_string(),
        location: args.location.clone(),
        admin_username: "azureuser".into(),
        login_public_ip,
        scheduler_private_ip,
        anf_mount_ip: pick("anfMountIp"),
        compute_vmss_names: outputs
            .get("computeVmssNames")
            .and_then(|v| v.get("value"))
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default(),
        extra_packages: args.extra_packages.clone(),
        accounting_enabled,
        bastion_enabled: args.bastion,
        bastion_name: pick("bastionName").filter(|s| !s.is_empty()),
        bastion_dns_name: pick("bastionDnsName").filter(|s| !s.is_empty()),
        bastion_resource_id: pick("bastionId").filter(|s| !s.is_empty()),
        storage_enabled: pick("storageAccountName")
            .map(|s| !s.is_empty())
            .unwrap_or(false),
        storage_account_name: pick("storageAccountName").filter(|s| !s.is_empty()),
        storage_blob_endpoint: pick("storageBlobEndpoint").filter(|s| !s.is_empty()),
        storage_dfs_endpoint: pick("storageDfsEndpoint").filter(|s| !s.is_empty()),
        storage_data_container_url: pick("storageDataContainerUrl").filter(|s| !s.is_empty()),
        storage_hns: args.storage_hns,
        storage_public_access: args.storage_public_access,
        azcp_version: Some(args.azcp_version.clone()),
    };
    let saved = state.save()?;
    eprintln!("==> saved cluster state -> {}", saved.display());

    let on_disk_secrets = cluster_state::ClusterSecrets::load_optional(&args.name)?;
    let secrets = cluster_state::ClusterSecrets {
        ldap_admin_password: ldap_password.to_string(),
        mysql_admin_password: if accounting_enabled {
            Some(mysql_password.to_string())
        } else {
            existing_secrets.and_then(|s| s.mysql_admin_password.clone())
        },
        admin_ssh_public_key: on_disk_secrets
            .as_ref()
            .map(|s| s.admin_ssh_public_key.clone())
            .or_else(|| existing_secrets.map(|s| s.admin_ssh_public_key.clone()))
            .unwrap_or_default(),
        admin_ssh_private_key: on_disk_secrets
            .as_ref()
            .map(|s| s.admin_ssh_private_key.clone())
            .or_else(|| existing_secrets.map(|s| s.admin_ssh_private_key.clone()))
            .unwrap_or_default(),
    };
    let secrets_path = secrets.save(&args.name)?;
    eprintln!("==> saved cluster secrets -> {}", secrets_path.display());

    if let Some(kv_name) = pick("keyVaultName") {
        match upload_cluster_to_keyvault(&kv_name, &state, &secrets) {
            Ok(()) => eprintln!("==> uploaded cluster manifest + secrets bundle to Key Vault '{kv_name}'"),
            Err(e) => eprintln!("==> WARNING: Key Vault upload to '{kv_name}' failed: {e:#}. Local state intact; re-run `azcluster deploy --name {}` to retry.", args.name),
        }
        match tag_resource_group_for_cluster(
            &arm_client()?,
            &state.resource_group,
            &args.name,
            &kv_name,
            &args.azcluster_version,
        ) {
            Ok(()) => eprintln!(
                "==> tagged RG '{}' with azcluster:* discovery tags",
                state.resource_group
            ),
            Err(e) => eprintln!(
                "==> WARNING: RG tag PATCH on '{}' failed: {e:#}. Cluster will be invisible to `azcluster list`; re-run deploy to retry.",
                state.resource_group
            ),
        }
    } else {
        eprintln!("==> WARNING: deployment did not return keyVaultName output; skipping KV upload");
    }

    if let Err(e) = timings::capture(
        &arm_client()?,
        &args.name,
        deployment_name,
        &state.resource_group,
        &args.shared_storage,
    ) {
        eprintln!("==> warning: timing capture failed: {e:#}");
    }

    // Dashboard import moved server-side in v0.24.14 (issue #1): the
    // scheduler's azcluster-grafana-import.service POSTs dashboards via
    // an IMDS token for the monitoring UAI which holds Grafana Admin.

    Ok(())
}

fn poll_deployment_until_terminal(deployment_name: &str) -> Result<String> {
    let client = arm_client()?;
    let mut progress = deploy_progress::Renderer::new();
    let v = client
        .wait_for_deployment_completion_with_progress(deployment_name, &mut |ops| {
            progress.render(ops);
        })
        .with_context(|| format!("poll {}", deployment_name))?;
    progress.finish();
    let state = v
        .get("properties")
        .and_then(|p| p.get("provisioningState"))
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_string();
    Ok(state)
}

fn current_principal() -> Result<(String, String)> {
    let token = get_access_token()?;
    let (oid, ptype) = auth::token_provider::extract_principal(&token)?;
    Ok((oid, ptype.as_arm_str().to_string()))
}

fn utc_stamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let days = secs / 86_400;
    let secs_today = secs % 86_400;
    let h = secs_today / 3600;
    let m = (secs_today % 3600) / 60;
    let s = secs_today % 60;
    let (y, mo, d) = civil_from_days(days as i64);
    format!("{y:04}{mo:02}{d:02}-{h:02}{m:02}{s:02}")
}

// Azure MySQL Flexible Server requires 8-128 chars containing at least 3 of:
// uppercase, lowercase, digit, non-alphanumeric. The fixed "Aa1!" suffix
// guarantees all four classes regardless of the random body.
fn gen_mysql_password() -> Result<String> {
    use std::io::Read;
    let mut buf = [0u8; 32];
    std::fs::File::open("/dev/urandom")
        .context("open /dev/urandom")?
        .read_exact(&mut buf)
        .context("read /dev/urandom")?;
    let alphabet: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnpqrstuvwxyz23456789";
    let mut out: String = buf
        .iter()
        .map(|b| alphabet[(*b as usize) % alphabet.len()] as char)
        .collect();
    out.push_str("Aa1!");
    Ok(out)
}

fn utc_iso8601() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days_since_epoch = (secs / 86400) as i64;
    let (y, m, d) = civil_from_days(days_since_epoch);
    let rem = secs % 86400;
    let (hh, mm, ss) = (
        (rem / 3600) as u32,
        ((rem % 3600) / 60) as u32,
        (rem % 60) as u32,
    );
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

fn should_use_bastion(state: &ClusterState, no_bastion_flag: bool) -> bool {
    !no_bastion_flag && state.bastion_enabled && state.login_public_ip.is_none()
}

fn target_vm_resource_id(state: &ClusterState, target: &str) -> Result<String> {
    let suffix = match target {
        "login" => "login",
        "scheduler" => "scheduler",
        other => bail!("unknown bastion target '{other}' (login|scheduler)"),
    };
    Ok(format!(
        "/subscriptions/{}/resourceGroups/{}/providers/Microsoft.Compute/virtualMachines/vm-{}-{}",
        state.subscription_id, state.resource_group, state.name, suffix
    ))
}

fn self_exe_path() -> Result<String> {
    let p = std::env::current_exe().context("locate current azcluster binary")?;
    Ok(p.to_string_lossy().into_owned())
}

fn bastion_compute_proxy_command(
    cluster: &str,
    admin_user: &str,
    admin_key: &std::path::Path,
) -> Result<String> {
    let exe = self_exe_path()?;
    let inner = format!("{} bastion-proxy --cluster {} --target login", exe, cluster);
    Ok(format!(
        "ssh -W %h:%p -i {} -o IdentitiesOnly=yes -o StrictHostKeyChecking=accept-new -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR -o ProxyCommand='{}' {}@127.0.0.1",
        admin_key.display(),
        inner,
        admin_user,
    ))
}

fn bastion_proxy(args: BastionProxyArgs) -> Result<()> {
    let state = resolve_cluster(&args.cluster)?;
    if !state.bastion_enabled {
        bail!("cluster '{}' was not deployed with --bastion", args.cluster);
    }
    let bastion_dns = state
        .bastion_dns_name
        .as_deref()
        .ok_or_else(|| anyhow!("bastion DNS name missing from cluster state"))?
        .to_string();
    let resource_id = target_vm_resource_id(&state, &args.target)?;
    let port = args.port;

    let access_token = get_access_token()?;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("build tokio runtime for bastion proxy")?;
    rt.block_on(async move {
        let client = std::sync::Arc::new(bastion::BastionClient::new(access_token));
        bastion::run_stdio_bridge(client, bastion_dns, resource_id, port).await
    })
}

fn ssh(args: ConnectArgs) -> Result<()> {
    let state = resolve_cluster(&args.name)?;
    let use_bastion = should_use_bastion(&state, args.no_bastion);
    let forward = format!("{}:{}:8443", args.local_port, state.scheduler_private_ip);
    let connect_user = args.user.as_deref().unwrap_or(&state.admin_username);
    let jump_user = connect_user;
    let mut cmd = Command::new("ssh");
    cmd.args(["-A", "-L", &forward]);
    let identity = resolve_identity_for_user(
        args.identity.as_deref(),
        &args.name,
        connect_user,
        &state.admin_username,
    )?;
    if let Some(key) = identity.as_deref() {
        cmd.args(["-i", &key.display().to_string()]);
    }
    if use_bastion {
        let exe = self_exe_path()?;
        if let Some(host) = &args.host {
            let inner_id = resolve_identity(None, &args.name)?;
            let outer_proxy =
                bastion_compute_proxy_command(&args.name, &state.admin_username, &inner_id)?;
            cmd.args(["-o", &format!("ProxyCommand={}", outer_proxy)]);
            let dest = format!("{}@{}", connect_user, host);
            cmd.arg(&dest);
            eprintln!("==> ssh via Bastion -> login -> {dest}");
        } else {
            let (target_name, host_ip) = if args.scheduler {
                ("scheduler", state.scheduler_private_ip.clone())
            } else {
                ("login", "127.0.0.1".to_string())
            };
            let proxy = format!(
                "{} bastion-proxy --cluster {} --target {}",
                exe, args.name, target_name
            );
            cmd.args(["-o", &format!("ProxyCommand={}", proxy)]);
            let target = format!("{}@{}", connect_user, host_ip);
            cmd.arg(&target);
            eprintln!("==> ssh via Bastion -> {target}");
        }
    } else {
        let host = state.login_public_ip.as_deref().ok_or_else(|| {
            anyhow!(
                "cluster '{}' has no login public IP and bastion is not enabled. \
                 Redeploy with --login-public-ip or --bastion.",
                args.name
            )
        })?;
        let jump_login = format!("{}@{}", jump_user, host);
        if let Some(hostname) = &args.host {
            let dest = format!("{}@{}", connect_user, hostname);
            add_ssh_jump(&mut cmd, identity.as_deref(), &jump_login);
            cmd.arg(&dest);
            eprintln!("==> ssh -J {jump_login} {dest}");
        } else if args.scheduler {
            let sched_target = format!("{}@{}", connect_user, state.scheduler_private_ip);
            add_ssh_jump(&mut cmd, identity.as_deref(), &jump_login);
            cmd.arg(&sched_target);
            eprintln!("==> ssh -J {jump_login} {sched_target}");
        } else {
            let login_target = format!("{}@{}", connect_user, host);
            cmd.arg(&login_target);
            eprintln!("==> ssh -L {forward} {login_target}");
        }
    }
    let status = cmd.status().context("spawn ssh")?;
    std::process::exit(status.code().unwrap_or(1));
}

fn tunnel(args: ConnectArgs) -> Result<()> {
    let state = resolve_cluster(&args.name)?;
    let use_bastion = should_use_bastion(&state, args.no_bastion);
    let forward = format!("{}:{}:8443", args.local_port, state.scheduler_private_ip);
    let mut cmd = Command::new("ssh");
    cmd.args([
        "-N",
        "-L",
        &forward,
        "-o",
        "ServerAliveInterval=30",
        "-o",
        "ExitOnForwardFailure=yes",
    ]);
    let identity = resolve_identity(args.identity.as_deref(), &args.name)?;
    cmd.args(["-i", &identity.display().to_string()]);
    if use_bastion {
        let exe = self_exe_path()?;
        let target = format!("{}@{}", state.admin_username, state.scheduler_private_ip);
        let proxy = format!(
            "{} bastion-proxy --cluster {} --target scheduler",
            exe, args.name
        );
        cmd.args(["-o", &format!("ProxyCommand={}", proxy)]);
        cmd.arg(&target);
        eprintln!(
            "==> tunnel localhost:{} -> {}:8443 via Bastion (Ctrl-C to stop)",
            args.local_port, state.scheduler_private_ip
        );
    } else {
        let host = state.login_public_ip.as_deref().ok_or_else(|| {
            anyhow!(
                "cluster '{}' has no login public IP and bastion is not enabled. \
                 Redeploy with --login-public-ip or --bastion.",
                args.name
            )
        })?;
        let target = format!("{}@{}", state.admin_username, host);
        cmd.arg(&target);
        eprintln!(
            "==> tunnel localhost:{} -> {}:8443 (Ctrl-C to stop)",
            args.local_port, state.scheduler_private_ip
        );
    }
    let status = cmd.status().context("spawn ssh")?;
    std::process::exit(status.code().unwrap_or(1));
}

fn scale(args: ScaleArgs) -> Result<()> {
    let state = resolve_cluster(&args.name)?;
    let vmss_name = format!("vmss-{}-{}", state.name, args.pool);
    if !state.compute_vmss_names.is_empty() && !state.compute_vmss_names.contains(&vmss_name) {
        bail!(
            "pool '{}' not found in cluster state (vmss '{vmss_name}' not in {:?}). \
             Known pools: {}",
            args.pool,
            state.compute_vmss_names,
            state
                .compute_vmss_names
                .iter()
                .filter_map(|n| n.strip_prefix(&format!("vmss-{}-", state.name)))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    eprintln!(
        "==> scaling vmss {vmss_name} (rg={}) -> capacity {}",
        state.resource_group, args.count
    );
    arm_client()?
        .scale_vmss(&state.resource_group, &vmss_name, args.count)
        .with_context(|| format!("scale {vmss_name}"))?;
    println!(
        "scaled {vmss_name} to capacity {} (resource group {})",
        args.count, state.resource_group
    );
    Ok(())
}

fn status(args: StatusArgs) -> Result<()> {
    let pending = PendingDeploy::load_optional(&args.name)?;
    let state_opt = resolve_cluster(&args.name).ok();

    if state_opt.is_none() && pending.is_none() {
        bail!(
            "no state or pending deploy for cluster '{}'. Run `azcluster deploy --name {}` first.",
            args.name,
            args.name
        );
    }

    if let Some(p) = pending.as_ref() {
        println!("pending deploy:    {}", p.deployment_name);
        println!("  started:         {}", p.started_at);
        println!("  resource group:  {}", p.resource_group);
        let client = arm_client().ok();
        match client
            .as_ref()
            .and_then(|c| c.get_deployment(&p.deployment_name).ok())
        {
            Some(v) => println!(
                "  ARM state:       {}",
                v.get("properties")
                    .and_then(|p| p.get("provisioningState"))
                    .and_then(|s| s.as_str())
                    .unwrap_or("?")
            ),
            None => println!("  ARM state:       ERR (lookup failed)"),
        }
        match client.as_ref().and_then(|c| {
            c.list_subscription_deployment_operations(&p.deployment_name)
                .ok()
        }) {
            Some(list) => {
                let mut succ = 0usize;
                let mut run = 0usize;
                let mut fail = 0usize;
                let mut other = 0usize;
                for op in &list {
                    match op
                        .get("properties")
                        .and_then(|p| p.get("provisioningState"))
                        .and_then(|s| s.as_str())
                        .unwrap_or("")
                    {
                        "Succeeded" => succ += 1,
                        "Running" => run += 1,
                        "Failed" => fail += 1,
                        _ => other += 1,
                    }
                }
                println!(
                    "  operations:      {} total ({} succeeded, {} running, {} failed, {} other)",
                    list.len(),
                    succ,
                    run,
                    fail,
                    other
                );
            }
            None => println!("  operations:      ERR (lookup failed)"),
        }
        println!(
            "  -> run `azcluster resume --name {}` once ARM state is Succeeded",
            args.name
        );
        println!();
    }

    let Some(state) = state_opt else {
        println!(
            "no cluster state yet. Once the ARM deployment succeeds, run `azcluster resume --name {}` to run post-deploy hooks.",
            args.name
        );
        return Ok(());
    };

    println!("name:              {}", state.name);
    println!("resource group:    {}", state.resource_group);
    println!("location:          {}", state.location);
    println!("scheduler ip:      {}", state.scheduler_private_ip);
    println!(
        "login public ip:   {}",
        state.login_public_ip.as_deref().unwrap_or("<none>")
    );
    println!(
        "anf mount ip:      {}",
        state.anf_mount_ip.as_deref().unwrap_or("<none>")
    );
    println!("compute pools:");
    if state.compute_vmss_names.is_empty() {
        println!("  <none>");
    } else {
        let client = arm_client().ok();
        for vmss in &state.compute_vmss_names {
            print!("  {vmss}: ");
            std::io::Write::flush(&mut std::io::stdout()).ok();
            match client
                .as_ref()
                .and_then(|c| c.get_vmss(&state.resource_group, vmss).ok())
            {
                Some(v) => {
                    let cap = v
                        .get("sku")
                        .and_then(|s| s.get("capacity"))
                        .and_then(|n| n.as_u64());
                    match cap {
                        Some(n) => println!("capacity={n}"),
                        None => println!("ERR (no sku.capacity)"),
                    }
                }
                None => println!("ERR (lookup failed)"),
            }
        }
    }

    if state.login_public_ip.is_some()
        || (state.bastion_enabled && state.bastion_dns_name.is_some())
    {
        println!("bootstrap probe:");
        bootstrap_probe(&state);
    }

    Ok(())
}

fn bootstrap_probe(state: &ClusterState) {
    let use_bastion = should_use_bastion(state, false);
    let login_ip = state.login_public_ip.as_deref();
    if !use_bastion && login_ip.is_none() {
        return;
    }
    let identity = match resolve_identity(None, &state.name) {
        Ok(p) => p,
        Err(e) => {
            println!("  login    : SKIP (no admin key: {e})");
            println!("  scheduler: SKIP (no admin key: {e})");
            return;
        }
    };
    let exe = if use_bastion {
        self_exe_path().ok()
    } else {
        None
    };
    let login_host = if use_bastion {
        "127.0.0.1".to_string()
    } else {
        login_ip.unwrap().to_string()
    };
    let login_target = format!("{}@{}", state.admin_username, login_host);
    let probe = |label: &str, is_scheduler: bool| {
        let host = if is_scheduler {
            state.scheduler_private_ip.clone()
        } else {
            login_host.clone()
        };
        let target = format!("{}@{}", state.admin_username, host);
        let mut cmd = Command::new("ssh");
        cmd.args([
            "-o",
            "ConnectTimeout=8",
            "-o",
            "BatchMode=yes",
            "-o",
            "StrictHostKeyChecking=accept-new",
            "-o",
            "UserKnownHostsFile=/dev/null",
            "-o",
            "LogLevel=ERROR",
            "-o",
            "IdentitiesOnly=yes",
            "-i",
            &identity.display().to_string(),
        ]);
        if use_bastion {
            let proxy_target = if is_scheduler { "scheduler" } else { "login" };
            let proxy_cmd = format!(
                "{} bastion-proxy --cluster {} --target {}",
                exe.as_deref().unwrap_or("azcluster"),
                state.name,
                proxy_target
            );
            cmd.args(["-o", &format!("ProxyCommand={proxy_cmd}")]);
        } else if is_scheduler {
            add_ssh_jump_with_identity(&mut cmd, &identity, &login_target);
        }
        cmd.args([
            &target,
            "if [ -f /var/log/azcluster/ready ]; then echo READY; else tail -n1 /var/log/azcluster/install.log 2>/dev/null || echo '<no log yet>'; fi",
        ]);
        match cmd.output() {
            Ok(o) if o.status.success() => {
                let line = String::from_utf8_lossy(&o.stdout);
                let trimmed = line.trim();
                if trimmed == "READY" {
                    println!("  {label}: READY");
                } else {
                    println!("  {label}: in-progress | last log: {trimmed}");
                }
            }
            Ok(o) => println!(
                "  {label}: ERR ({}: {})",
                o.status,
                String::from_utf8_lossy(&o.stderr).trim()
            ),
            Err(e) => println!("  {label}: ERR ({e})"),
        }
    };
    probe("login    ", false);
    probe("scheduler", true);
}

fn delete(args: DeleteArgs) -> Result<()> {
    let (cluster_name, resource_group) = match resolve_cluster(&args.name) {
        Ok(s) => (s.name, s.resource_group),
        Err(_) => match PendingDeploy::load_optional(&args.name)? {
            Some(p) => (p.cluster, p.resource_group),
            None => bail!(
                "no state or pending deploy for cluster '{}'; nothing to delete",
                args.name
            ),
        },
    };
    if !args.yes {
        eprint!(
            "Delete resource group '{}' (cluster '{}')? Type cluster name to confirm: ",
            resource_group, cluster_name
        );
        std::io::Write::flush(&mut std::io::stderr()).ok();
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        if line.trim() != cluster_name {
            bail!("aborted");
        }
    }
    eprintln!(
        "==> deleting resource group {} (async, no-wait)",
        resource_group
    );
    arm_client()?
        .delete_resource_group(&resource_group)
        .with_context(|| format!("delete resource group {}", resource_group))?;
    let path = cluster_state::state_path(&cluster_name)?;
    if path.exists() {
        std::fs::remove_file(&path).ok();
        eprintln!("==> removed local state {}", path.display());
    }
    PendingDeploy::delete(&cluster_name).ok();
    Ok(())
}

fn exec(args: ExecArgs) -> Result<()> {
    let state = resolve_cluster(&args.name)?;
    let use_bastion = should_use_bastion(&state, args.no_bastion);
    let connect_user = args.user.as_deref().unwrap_or(&state.admin_username);
    let jump_user = connect_user;
    let mut cmd = Command::new("ssh");
    if args.forward_agent {
        cmd.arg("-A");
    }
    let identity = resolve_identity_for_user(
        args.identity.as_deref(),
        &args.name,
        connect_user,
        &state.admin_username,
    )?;
    if let Some(key) = identity.as_deref() {
        cmd.args(["-i", &key.display().to_string()]);
    }
    if use_bastion {
        let exe = self_exe_path()?;
        if let Some(host) = &args.host {
            let inner_id = resolve_identity(None, &args.name)?;
            let outer_proxy =
                bastion_compute_proxy_command(&args.name, &state.admin_username, &inner_id)?;
            cmd.args(["-o", &format!("ProxyCommand={}", outer_proxy)]);
            cmd.arg(format!("{}@{}", connect_user, host));
        } else {
            let (target_name, host_ip) = if args.scheduler {
                ("scheduler", state.scheduler_private_ip.clone())
            } else {
                ("login", "127.0.0.1".to_string())
            };
            let proxy = format!(
                "{} bastion-proxy --cluster {} --target {}",
                exe, args.name, target_name
            );
            cmd.args(["-o", &format!("ProxyCommand={}", proxy)]);
            cmd.arg(format!("{}@{}", connect_user, host_ip));
        }
    } else {
        let host = state.login_public_ip.as_deref().ok_or_else(|| {
            anyhow!(
                "cluster '{}' has no login public IP and bastion is not enabled. \
                 Redeploy with --login-public-ip or --bastion.",
                args.name
            )
        })?;
        let jump_login = format!("{}@{}", jump_user, host);
        if let Some(hostname) = &args.host {
            add_ssh_jump(&mut cmd, identity.as_deref(), &jump_login);
            cmd.arg(format!("{}@{}", connect_user, hostname));
        } else if args.scheduler {
            add_ssh_jump(&mut cmd, identity.as_deref(), &jump_login);
            cmd.arg(format!("{}@{}", connect_user, state.scheduler_private_ip));
        } else {
            cmd.arg(format!("{}@{}", connect_user, host));
        }
    }
    cmd.arg("--");
    for part in &args.cmd {
        cmd.arg(part);
    }
    let status = cmd.status().context("spawn ssh exec")?;
    std::process::exit(status.code().unwrap_or(1));
}

#[derive(Debug, PartialEq, Eq)]
enum ScpPath {
    Local(String),
    Remote { node: String, path: String },
}

fn parse_scp_path(raw: &str) -> ScpPath {
    let Some(colon) = raw.find(':') else {
        return ScpPath::Local(raw.to_string());
    };
    let head = &raw[..colon];
    if head.contains('/') {
        return ScpPath::Local(raw.to_string());
    }
    let node = if head.is_empty() { "login" } else { head };
    ScpPath::Remote {
        node: node.to_string(),
        path: raw[colon + 1..].to_string(),
    }
}

fn scp(args: ScpArgs) -> Result<()> {
    if args.paths.len() < 2 {
        bail!("scp requires at least one source and one destination");
    }
    let state = resolve_cluster(&args.name)?;
    let use_bastion = should_use_bastion(&state, args.no_bastion);

    let parsed: Vec<ScpPath> = args.paths.iter().map(|s| parse_scp_path(s)).collect();

    let mut remote_node: Option<String> = None;
    let mut any_remote = false;
    let mut any_local = false;
    for p in &parsed {
        match p {
            ScpPath::Local(_) => any_local = true,
            ScpPath::Remote { node, .. } => {
                any_remote = true;
                match &remote_node {
                    None => remote_node = Some(node.clone()),
                    Some(existing) if existing == node => {}
                    Some(existing) => bail!(
                        "all remote paths must reference the same node (saw '{existing}' and '{node}'); split into multiple scp invocations"
                    ),
                }
            }
        }
    }
    if !any_remote {
        bail!(
            "at least one path must be remote ([node]:path); use plain `cp` for local-only copies"
        );
    }
    if !any_local {
        bail!("at least one path must be local; remote-to-remote scp is not supported (run scp on a remote node directly)");
    }
    let node = remote_node.expect("any_remote implies remote_node");

    let connect_user = args.user.as_deref().unwrap_or(&state.admin_username);
    let (proxy_target, jump_login_host, dest_host) = resolve_scp_route(&state, &node, use_bastion)?;

    let mut cmd = Command::new("scp");
    if args.recursive {
        cmd.arg("-r");
    }
    if args.preserve {
        cmd.arg("-p");
    }
    let identity = resolve_identity_for_user(
        args.identity.as_deref(),
        &args.name,
        connect_user,
        &state.admin_username,
    )?;
    if let Some(key) = identity.as_deref() {
        cmd.args(["-i", &key.display().to_string()]);
    }
    match (proxy_target.as_deref(), jump_login_host.as_deref()) {
        (Some("login"), Some(_)) => {
            let inner_id = resolve_identity(None, &args.name)?;
            let outer_proxy =
                bastion_compute_proxy_command(&args.name, &state.admin_username, &inner_id)?;
            cmd.args(["-o", &format!("ProxyCommand={}", outer_proxy)]);
        }
        (Some(target), _) => {
            let exe = self_exe_path()?;
            let proxy = format!(
                "{} bastion-proxy --cluster {} --target {}",
                exe, args.name, target
            );
            cmd.args(["-o", &format!("ProxyCommand={}", proxy)]);
        }
        (None, Some(login)) => {
            let jump = format!("{}@{}", connect_user, login);
            add_ssh_jump(&mut cmd, identity.as_deref(), &jump);
        }
        (None, None) => {}
    }

    let user_at_host = format!("{}@{}", connect_user, dest_host);
    for p in &parsed {
        match p {
            ScpPath::Local(s) => cmd.arg(s),
            ScpPath::Remote { path, .. } => cmd.arg(format!("{user_at_host}:{path}")),
        };
    }

    eprintln!(
        "==> scp {} (node={node}{})",
        args.paths.join(" "),
        if use_bastion { ", via Bastion" } else { "" }
    );
    let status = cmd.status().context("spawn scp")?;
    std::process::exit(status.code().unwrap_or(1));
}

fn resolve_scp_route(
    state: &ClusterState,
    node: &str,
    use_bastion: bool,
) -> Result<(Option<String>, Option<String>, String)> {
    let login_public = || -> Result<String> {
        state
            .login_public_ip
            .as_deref()
            .map(|s| s.to_string())
            .ok_or_else(|| {
                anyhow!(
                    "cluster '{}' has no login public IP and bastion is not enabled. \
                     Redeploy with --login-public-ip or --bastion.",
                    state.name
                )
            })
    };
    match (node, use_bastion) {
        ("login", true) => Ok((Some("login".into()), None, "127.0.0.1".into())),
        ("login", false) => {
            let h = login_public()?;
            Ok((None, None, h))
        }
        ("scheduler", true) => Ok((
            Some("scheduler".into()),
            None,
            state.scheduler_private_ip.clone(),
        )),
        ("scheduler", false) => {
            let h = login_public()?;
            Ok((None, Some(h), state.scheduler_private_ip.clone()))
        }
        (compute, true) => Ok((
            Some("login".into()),
            Some("127.0.0.1".into()),
            compute.to_string(),
        )),
        (compute, false) => {
            let h = login_public()?;
            Ok((None, Some(h), compute.to_string()))
        }
    }
}

fn logs(args: LogsArgs) -> Result<()> {
    let state = resolve_cluster(&args.name)?;
    let host = state.login_public_ip.as_deref().ok_or_else(|| {
        anyhow!(
            "cluster '{}' has no login public IP. Redeploy with --login-public-ip.",
            args.name
        )
    })?;
    let login_target = format!("{}@{}", state.admin_username, host);
    let log_path = "/var/log/azcluster/install.log";
    let tail_arg = if args.follow {
        format!("tail -F -n {} {}", args.tail, log_path)
    } else if args.tail == 0 {
        format!("cat {}", log_path)
    } else {
        format!("tail -n {} {}", args.tail, log_path)
    };
    let remote_cmd = match args.component.as_str() {
        "login" => tail_arg.clone(),
        "scheduler" => format!(
            "ssh -o StrictHostKeyChecking=accept-new {}@{} {}",
            state.admin_username,
            state.scheduler_private_ip,
            shell_quote(&tail_arg),
        ),
        other => format!(
            "ssh -o StrictHostKeyChecking=accept-new {}@{} {}",
            state.admin_username,
            other,
            shell_quote(&tail_arg),
        ),
    };
    let mut cmd = Command::new("ssh");
    cmd.args(["-A"]);
    let identity = resolve_identity(args.identity.as_deref(), &args.name)?;
    cmd.args(["-i", &identity.display().to_string()]);
    cmd.arg(&login_target).arg(&remote_cmd);
    let status = cmd.status().context("spawn ssh logs")?;
    std::process::exit(status.code().unwrap_or(1));
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn validate(args: ValidateArgs) -> Result<()> {
    let state = resolve_cluster(&args.name)?;
    let use_bastion = should_use_bastion(&state, false);
    let login_target = if use_bastion {
        format!("{}@127.0.0.1", state.admin_username)
    } else {
        let host = state.login_public_ip.as_deref().ok_or_else(|| {
            anyhow!(
                "cluster '{}' has no login public IP and bastion is not enabled. \
                 Redeploy with --login-public-ip or --bastion.",
                args.name
            )
        })?;
        format!("{}@{}", state.admin_username, host)
    };

    let part = args
        .partition
        .as_deref()
        .map(|p| format!(" --partition={p}"))
        .unwrap_or_default();

    let mut checks: Vec<(&str, String)> = vec![
        ("sinfo", "sinfo -h -o '%P %D %T %N'".into()),
        (
            "srun hostname",
            format!("timeout 60 srun{part} -N1 --time=1 hostname"),
        ),
    ];
    if !args.no_container {
        checks.push((
            "srun pyxis alpine",
            format!(
                "timeout 180 srun{part} -N1 --time=2 \
                 --container-image=docker://alpine:latest hostname"
            ),
        ));
    }
    if args.gpu {
        checks.push((
            "srun gpu nvidia-smi",
            format!("timeout 180 srun{part} -N1 --gres=gpu:1 --time=2 nvidia-smi -L"),
        ));
    }
    if args.multi_node {
        checks.push((
            "srun 2-node hostname",
            format!(
                "timeout 120 srun{part} -N2 --ntasks-per-node=1 --time=2 \
                 bash -c 'hostname; sleep 1'"
            ),
        ));
        if !args.no_container {
            checks.push((
                "srun 2-node pyxis alpine",
                format!(
                    "timeout 300 srun{part} -N2 --ntasks-per-node=1 --time=4 \
                     --container-image=docker://alpine:latest hostname"
                ),
            ));
        }
        if args.gpu {
            let script = "set -euo pipefail\n\
                 HPCX_DIR=$(ls -d /opt/hpcx-*-gcc-doca_ofed-ubuntu24.04-cuda*-x86_64 \
                 2>/dev/null | head -1)\n\
                 if [ -z \"$HPCX_DIR\" ]; then echo 'HPC-X not found' >&2; exit 1; fi\n\
                 if [ ! -x /opt/nccl-tests/build/all_reduce_perf ]; then \
                 echo 'nccl-tests not found' >&2; exit 1; fi\n\
                 source \"$HPCX_DIR/hpcx-init.sh\"; hpcx_load\n\
                 export NCCL_IB_HCA=mlx5_ib\n\
                 export NCCL_TOPO_FILE=/opt/microsoft/ndv5-topo.xml\n\
                 export UCX_NET_DEVICES=mlx5_ib0:1,mlx5_ib1:1,mlx5_ib2:1,mlx5_ib3:1,\
                 mlx5_ib4:1,mlx5_ib5:1,mlx5_ib6:1,mlx5_ib7:1\n\
                 timeout 300 srun --mpi=pmix -N2 --ntasks-per-node=8 \
                 --gpus-per-node=8 --exclusive --time=5 \
                 /opt/nccl-tests/build/all_reduce_perf -b 8M -e 64M -f 2 -g 1";
            let script = script.replace("\n                 ", "\n");
            let script = if part.is_empty() {
                script
            } else {
                script.replace("srun --mpi=pmix", &format!("srun{part} --mpi=pmix"))
            };
            let remote = format!("bash -lc {}", shell_quote(&script));
            checks.push(("srun 2-node nccl-allreduce (NDv5)", remote));
        }
    }

    let mut all_ok = true;
    for (label, remote) in &checks {
        eprintln!("==> [{label}] {remote}");
        let mut cmd = Command::new("ssh");
        cmd.args(["-A", "-o", "StrictHostKeyChecking=accept-new"]);
        let identity = resolve_identity(args.identity.as_deref(), &args.name)?;
        cmd.args(["-i", &identity.display().to_string()]);
        if use_bastion {
            let exe = self_exe_path()?;
            let proxy = format!(
                "{} bastion-proxy --cluster {} --target login",
                exe, args.name
            );
            cmd.args(["-o", &format!("ProxyCommand={}", proxy)]);
        }
        cmd.arg(&login_target).arg(remote);
        let st = cmd.status().context("spawn ssh validate")?;
        if !st.success() {
            eprintln!("==> [{label}] FAILED ({})", st);
            all_ok = false;
        } else {
            eprintln!("==> [{label}] OK");
        }
    }
    if !all_ok {
        bail!("one or more validation checks failed");
    }
    eprintln!("==> all checks passed");
    Ok(())
}

fn monitor(args: MonitorArgs) -> Result<()> {
    let state = resolve_cluster(&args.name)?;
    let grafana_name = format!("amg-{}", state.name);
    match arm_client()?.get_grafana_endpoint(&state.resource_group, &grafana_name) {
        Ok(url) if !url.is_empty() => {
            println!("{url}");
            Ok(())
        }
        _ => bail!(
            "Grafana endpoint not found for cluster '{}'. Was --monitoring enabled at deploy?",
            state.name
        ),
    }
}

fn timings(args: TimingsArgs) -> Result<()> {
    let runs = timings::list_for_cluster(&args.name, args.last)?;
    if runs.is_empty() {
        bail!(
            "no timing data for cluster '{}'. Deploy with this version first.",
            args.name
        );
    }
    if args.trend {
        let path = timings::deployments_dir(&args.name)?.join("trend.tsv");
        if path.exists() {
            let body = std::fs::read_to_string(&path)?;
            print!("{body}");
        }
        return Ok(());
    }
    for (i, t) in runs.iter().enumerate() {
        if i > 0 {
            println!();
        }
        timings::print_table(t);
    }
    Ok(())
}

fn purge_cache(args: PurgeCacheArgs) -> Result<()> {
    let n = cluster_resolver::purge_cache(args.name.as_deref())?;
    match (&args.name, n) {
        (Some(name), 0) => eprintln!("==> no cached entry for cluster '{name}'"),
        (Some(name), _) => eprintln!("==> purged cache for cluster '{name}'"),
        (None, 0) => eprintln!("==> cache already empty"),
        (None, _) => eprintln!("==> purged {n} cached cluster manifest(s)"),
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DeletedVault {
    name: String,
    location: String,
    deletion_date: String,
    scheduled_purge_date: String,
}

fn parse_deleted_vault(v: &serde_json::Value) -> Option<DeletedVault> {
    let name = v.get("name").and_then(|s| s.as_str())?.to_string();
    let props = v.get("properties")?;
    Some(DeletedVault {
        name,
        location: props
            .get("location")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string(),
        deletion_date: props
            .get("deletionDate")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string(),
        scheduled_purge_date: props
            .get("scheduledPurgeDate")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string(),
    })
}

fn filter_purge_kv_candidates(
    all: Vec<DeletedVault>,
    target_name: Option<&str>,
    target_location: Option<&str>,
) -> Vec<DeletedVault> {
    all.into_iter()
        .filter(|v| v.name.starts_with("kv-azc-"))
        .filter(|v| match target_name {
            Some(n) => v.name == n,
            None => true,
        })
        .filter(|v| match target_location {
            Some(l) => v.location.eq_ignore_ascii_case(l),
            None => true,
        })
        .collect()
}

fn purge_kv(args: PurgeKvArgs) -> Result<()> {
    if args.name.is_some() && args.location.is_none() {
        anyhow::bail!("--name requires --location (KV name is derived from sub|name|location)");
    }
    let arm = arm_client()?;
    let sub_id = arm.subscription_id().to_string();
    let target_kv_name = match (args.name.as_deref(), args.location.as_deref()) {
        (Some(n), Some(loc)) => Some(crypto::derive_kv_name(&sub_id, n, loc)),
        _ => None,
    };

    let raw = arm.list_deleted_vaults()?;
    let parsed: Vec<DeletedVault> = raw.iter().filter_map(parse_deleted_vault).collect();
    let candidates =
        filter_purge_kv_candidates(parsed, target_kv_name.as_deref(), args.location.as_deref());

    println!("Subscription: {sub_id}");
    if candidates.is_empty() {
        println!("(no matching soft-deleted azcluster Key Vaults found)");
        return Ok(());
    }

    let w_name = candidates.iter().map(|v| v.name.len()).max().unwrap_or(4);
    let w_loc = candidates
        .iter()
        .map(|v| v.location.len())
        .max()
        .unwrap_or(8);
    println!(
        "{:<wn$}  {:<wl$}  DELETED                   SCHEDULED PURGE",
        "NAME",
        "LOCATION",
        wn = w_name,
        wl = w_loc,
    );
    for v in &candidates {
        println!(
            "{:<wn$}  {:<wl$}  {:<25}  {}",
            v.name,
            v.location,
            v.deletion_date,
            v.scheduled_purge_date,
            wn = w_name,
            wl = w_loc,
        );
    }

    if args.dry_run {
        return Ok(());
    }

    let needs_explicit_all = args.name.is_none() && !args.all;
    if needs_explicit_all {
        anyhow::bail!(
            "refusing to purge {} vault(s) without --all or --name (re-run with --all to confirm scope)",
            candidates.len()
        );
    }

    if !args.yes {
        eprintln!(
            "\nAbout to PERMANENTLY purge {} Key Vault(s). This bypasses the 7-day soft-delete retention.",
            candidates.len()
        );
        eprint!("Type 'yes' to continue: ");
        use std::io::{stdin, stdout, Write};
        stdout().flush().ok();
        let mut buf = String::new();
        stdin().read_line(&mut buf)?;
        if buf.trim() != "yes" {
            anyhow::bail!("aborted");
        }
    }

    let mut failed = 0u32;
    for v in &candidates {
        eprintln!("==> purging {} ({})", v.name, v.location);
        match arm.purge_deleted_vault(&v.location, &v.name) {
            Ok(()) => eprintln!("    OK"),
            Err(e) => {
                failed += 1;
                eprintln!("    ERR: {e:#}");
            }
        }
    }

    if failed > 0 {
        anyhow::bail!("{failed} purge(s) failed");
    }
    eprintln!("==> purged {} vault(s)", candidates.len());
    Ok(())
}

fn list(args: ListArgs) -> Result<()> {
    let arm = arm_client()?;
    let sub_id = arm.subscription_id().to_string();
    let rgs = arm.list_resource_groups_by_tag(cluster_resolver::TAG_MANAGED, Some("true"))?;

    let mut rows: Vec<(String, String, String, String, String)> = Vec::with_capacity(rgs.len());
    for rg in &rgs {
        let rg_name = rg
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let location = rg
            .get("location")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let tags = rg.get("tags").cloned().unwrap_or(serde_json::json!({}));
        let cluster_name = tags
            .get(cluster_resolver::TAG_NAME)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let version = tags
            .get(cluster_resolver::TAG_VERSION)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let deployed_at = tags
            .get(cluster_resolver::TAG_DEPLOYED_AT)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if cluster_name.is_empty() {
            continue;
        }
        rows.push((cluster_name, location, rg_name, version, deployed_at));
    }
    rows.sort_by(|a, b| a.0.cmp(&b.0));

    if args.json {
        let arr: Vec<serde_json::Value> = rows
            .iter()
            .map(|(n, loc, rg, ver, ts)| {
                serde_json::json!({
                    "subscription": sub_id,
                    "name": n,
                    "location": loc,
                    "resource_group": rg,
                    "version": ver,
                    "deployed_at": ts,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&arr)?);
        return Ok(());
    }

    println!("Subscription: {sub_id}");
    if rows.is_empty() {
        println!("(no azcluster-managed clusters found)");
        return Ok(());
    }
    let w_name = rows.iter().map(|r| r.0.len()).max().unwrap_or(4).max(4);
    let w_loc = rows.iter().map(|r| r.1.len()).max().unwrap_or(8).max(8);
    let w_rg = rows.iter().map(|r| r.2.len()).max().unwrap_or(14).max(14);
    let w_ver = rows.iter().map(|r| r.3.len()).max().unwrap_or(7).max(7);
    println!(
        "{:<wn$}  {:<wl$}  {:<wr$}  {:<wv$}  DEPLOYED AT",
        "NAME",
        "LOCATION",
        "RESOURCE GROUP",
        "VERSION",
        wn = w_name,
        wl = w_loc,
        wr = w_rg,
        wv = w_ver,
    );
    for (n, loc, rg, ver, ts) in &rows {
        println!(
            "{:<wn$}  {:<wl$}  {:<wr$}  {:<wv$}  {}",
            n,
            loc,
            rg,
            ver,
            ts,
            wn = w_name,
            wl = w_loc,
            wr = w_rg,
            wv = w_ver,
        );
    }
    Ok(())
}

fn user_dispatch(args: UserArgs) -> Result<()> {
    match args.cmd {
        UserCmd::Add {
            cluster,
            username,
            uid,
            gid,
            gecos,
            shell,
            ssh_keys,
            admin,
            no_generate_keypair,
        } => {
            let state = resolve_cluster(&cluster)?;
            user::user_add(
                &state,
                &username,
                uid,
                gid,
                &gecos,
                &shell,
                &ssh_keys,
                admin,
                !no_generate_keypair,
            )
        }
        UserCmd::Remove { cluster, username } => {
            let state = resolve_cluster(&cluster)?;
            user::user_remove(&state, &username)
        }
        UserCmd::List { cluster } => {
            let state = resolve_cluster(&cluster)?;
            user::user_list(&state)
        }
        UserCmd::Setadmin { cluster, username } => {
            let state = resolve_cluster(&cluster)?;
            user::user_setadmin(&state, &username, true)
        }
        UserCmd::Unsetadmin { cluster, username } => {
            let state = resolve_cluster(&cluster)?;
            user::user_setadmin(&state, &username, false)
        }
        UserCmd::Sshkey { cmd } => match cmd {
            SshkeyCmd::Add {
                cluster,
                username,
                key_file,
            } => {
                let state = resolve_cluster(&cluster)?;
                user::sshkey_add(&state, &username, &key_file)
            }
            SshkeyCmd::Remove {
                cluster,
                username,
                key_file,
            } => {
                let state = resolve_cluster(&cluster)?;
                user::sshkey_remove(&state, &username, &key_file)
            }
            SshkeyCmd::List { cluster, username } => {
                let state = resolve_cluster(&cluster)?;
                user::sshkey_list(&state, &username)
            }
        },
    }
}
