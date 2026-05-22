mod cluster_state;
mod timings;

use anyhow::{anyhow, bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use cluster_state::ClusterState;
use serde::Serialize;
use std::path::PathBuf;
use std::process::{Command, Stdio};

#[derive(Parser)]
#[command(name = "azcluster", version = azcluster_core::VERSION, about = "Manage Slurm clusters on Azure")]
struct Cli {
    #[command(subcommand)]
    command: CliCommand,
}

#[derive(Subcommand)]
enum CliCommand {
    Version,
    Deploy(Box<DeployArgs>),
    Ssh(ConnectArgs),
    Tunnel(ConnectArgs),
    Scale(ScaleArgs),
    Status(StatusArgs),
    Delete(DeleteArgs),
    Exec(ExecArgs),
    Logs(LogsArgs),
    Validate(ValidateArgs),
    Monitor(MonitorArgs),
    Timings(TimingsArgs),
}

#[derive(Args)]
struct DeployArgs {
    #[arg(long)]
    name: String,
    #[arg(long)]
    location: String,
    #[arg(long)]
    resource_group: Option<String>,
    #[arg(long)]
    ssh_key: Option<PathBuf>,
    #[arg(long, default_value_t = false)]
    login_public_ip: bool,
    #[arg(long)]
    allowed_ssh_cidrs: Option<String>,
    #[arg(long, default_value = "v0.12.0")]
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
    /// Provision Slurm accounting (managed MySQL + slurmdbd) (default: on). [reserved, v0.13.x]
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
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct PoolSpec {
    name: String,
    sku: String,
    count: u32,
    #[serde(rename = "default")]
    is_default: bool,
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
}

#[derive(Args)]
struct ConnectArgs {
    name: String,
    #[arg(long, default_value_t = 8443)]
    local_port: u16,
    #[arg(long)]
    identity: Option<PathBuf>,
    /// Hop through login to the scheduler VM.
    #[arg(long, default_value_t = false)]
    scheduler: bool,
}

#[derive(Args)]
struct ExecArgs {
    name: String,
    #[arg(long)]
    identity: Option<PathBuf>,
    /// Hop through login to the scheduler VM.
    #[arg(long, default_value_t = false)]
    scheduler: bool,
    #[arg(trailing_var_arg = true, required = true)]
    cmd: Vec<String>,
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

#[derive(Serialize)]
struct ScaleRequest {
    count: u32,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        CliCommand::Version => {
            println!("azcluster {}", azcluster_core::VERSION);
            Ok(())
        }
        CliCommand::Deploy(args) => deploy(*args),
        CliCommand::Ssh(args) => ssh(args),
        CliCommand::Tunnel(args) => tunnel(args),
        CliCommand::Scale(args) => scale(args),
        CliCommand::Status(args) => status(args),
        CliCommand::Delete(args) => delete(args),
        CliCommand::Exec(args) => exec(args),
        CliCommand::Logs(args) => logs(args),
        CliCommand::Validate(args) => validate(args),
        CliCommand::Monitor(args) => monitor(args),
        CliCommand::Timings(args) => timings(args),
    }
}

fn resolve_template(explicit: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(p) = explicit {
        if !p.exists() {
            bail!("template {} not found", p.display());
        }
        return Ok(p);
    }
    for candidate in ["./bicep/main.bicep", "./assets/bicep/main.bicep"] {
        let p = PathBuf::from(candidate);
        if p.exists() {
            return Ok(p);
        }
    }
    bail!("no Bicep template found. Pass --template PATH or run from a checkout / extracted assets directory.")
}

fn resolve_ssh_key(explicit: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(p) = explicit {
        return Ok(p);
    }
    let home = std::env::var("HOME").context("HOME not set")?;
    for candidate in [".ssh/id_ed25519.pub", ".ssh/id_rsa.pub"] {
        let p = PathBuf::from(&home).join(candidate);
        if p.exists() {
            return Ok(p);
        }
    }
    bail!("no SSH public key found. Pass --ssh-key PATH.")
}

fn ensure_az() -> Result<()> {
    let ok = Command::new("az")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        bail!("az CLI not found in PATH");
    }
    let logged_in = Command::new("az")
        .args(["account", "show"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !logged_in {
        bail!("not logged in to Azure. Run: az login");
    }
    Ok(())
}

fn az_json(args: &[&str]) -> Result<serde_json::Value> {
    let out = Command::new("az")
        .args(args)
        .output()
        .with_context(|| format!("spawn az {}", args.join(" ")))?;
    if !out.status.success() {
        bail!(
            "az {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    serde_json::from_slice(&out.stdout).context("parse az JSON output")
}

fn deploy(args: DeployArgs) -> Result<()> {
    ensure_az()?;
    let template = resolve_template(args.template.clone())?;
    let ssh_key_path = resolve_ssh_key(args.ssh_key.clone())?;
    let ssh_key = std::fs::read_to_string(&ssh_key_path)
        .with_context(|| format!("read {}", ssh_key_path.display()))?;

    let sub_id = az_json(&["account", "show", "--query", "id", "-o", "json"])?
        .as_str()
        .ok_or_else(|| anyhow!("subscription id not a string"))?
        .to_string();

    let allowed_cidrs_json = match args.allowed_ssh_cidrs.as_deref() {
        Some(csv) if !csv.is_empty() => {
            serde_json::to_string(&csv.split(',').filter(|s| !s.is_empty()).collect::<Vec<_>>())?
        }
        _ => "[]".to_string(),
    };

    if let Some(rg) = args.resource_group.as_deref() {
        let status = Command::new("az")
            .args([
                "group",
                "create",
                "--name",
                rg,
                "--location",
                &args.location,
                "--tags",
                "azcluster=true",
                &format!("azcluster-name={}", args.name),
                "-o",
                "none",
            ])
            .status()?;
        if !status.success() {
            bail!("az group create failed");
        }
    }

    let deployment_name = format!("azcluster-{}-{}", args.name, utc_stamp());

    let pools: Vec<PoolSpec> = if args.pools.is_empty() {
        vec![PoolSpec {
            name: "gpu".into(),
            sku: "Standard_ND96isr_H200_v5".into(),
            count: 0,
            is_default: true,
        }]
    } else {
        args.pools
            .iter()
            .map(|s| parse_pool(s))
            .collect::<Result<_>>()?
    };
    let pools_json = serde_json::to_string(&pools).context("encode pools")?;

    let mut params: Vec<(&str, String)> = vec![
        ("clusterName", args.name.clone()),
        ("location", args.location.clone()),
        ("sshPublicKey", ssh_key.trim().to_string()),
        ("loginPublicIp", args.login_public_ip.to_string()),
        ("allowedSshCidrs", allowed_cidrs_json),
        ("azclusterVersion", args.azcluster_version.clone()),
        ("azclusterRepo", args.azcluster_repo.clone()),
        ("ubuntuSku", args.ubuntu.clone()),
        (
            "existingResourceGroup",
            args.resource_group.clone().unwrap_or_default(),
        ),
        ("anfSizeTiB", args.anf_size_tib.to_string()),
        ("anfServiceLevel", args.anf_tier.clone()),
        ("amlfsSizeTiB", args.amlfs_size_tib.to_string()),
        ("amlfsSkuName", args.amlfs_sku.clone()),
        ("amlfsZone", args.amlfs_zone.clone()),
        ("pools", pools_json),
        ("enableMonitoring", args.monitoring.to_string()),
        ("sharedStorageMode", args.shared_storage.clone()),
        ("enableAccounting", args.accounting.to_string()),
        (
            "grafanaLocation",
            args.grafana_location
                .clone()
                .unwrap_or_else(|| args.location.clone()),
        ),
    ];

    if args.monitoring {
        let (oid, ptype) = current_principal()?;
        eprintln!("==> deployer principal: {oid} ({ptype}) -> will receive Grafana Admin on AMG");
        params.push(("deployerPrincipalId", oid));
        params.push(("deployerPrincipalType", ptype));
    }

    let mut az_args: Vec<String> = vec![
        "deployment".into(),
        "sub".into(),
        if args.what_if {
            "what-if".into()
        } else {
            "create".into()
        },
        "--name".into(),
        deployment_name.clone(),
        "--location".into(),
        args.location.clone(),
        "--template-file".into(),
        template.display().to_string(),
        "--parameters".into(),
    ];
    for (k, v) in &params {
        az_args.push(format!("{k}={v}"));
    }

    eprintln!(
        "==> az deployment sub {} --name {}",
        if args.what_if { "what-if" } else { "create" },
        deployment_name
    );
    let status = Command::new("az")
        .args(&az_args)
        .status()
        .context("spawn az deployment")?;
    if !status.success() {
        bail!("az deployment failed");
    }

    if args.what_if {
        return Ok(());
    }

    let outputs = az_json(&[
        "deployment",
        "sub",
        "show",
        "--name",
        &deployment_name,
        "--query",
        "properties.outputs",
        "-o",
        "json",
    ])?;

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
        subscription_id: sub_id,
        resource_group: args
            .resource_group
            .clone()
            .unwrap_or_else(|| format!("rg-azcluster-{}", args.name)),
        location: args.location,
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
    };
    let saved = state.save()?;
    eprintln!("==> saved cluster state -> {}", saved.display());

    if let Err(e) = timings::capture(
        &args.name,
        &deployment_name,
        &state.resource_group,
        &args.shared_storage,
    ) {
        eprintln!("==> warning: timing capture failed: {e:#}");
    }

    if args.monitoring {
        if let Some(grafana_name) = pick("grafanaName") {
            import_dashboards(&state.resource_group, &grafana_name)?;
        } else {
            eprintln!("==> warning: monitoring enabled but grafanaName output missing; skipping dashboard import");
        }
    }

    Ok(())
}

fn current_principal() -> Result<(String, String)> {
    let user_type = az_json(&["account", "show", "--query", "user.type", "-o", "json"])?;
    let user_type_s = user_type.as_str().unwrap_or("user");
    if user_type_s == "user" {
        let v = az_json(&[
            "ad",
            "signed-in-user",
            "show",
            "--query",
            "id",
            "-o",
            "json",
        ])
        .context("az ad signed-in-user show (need 'User.Read' Graph permission)")?;
        let oid = v
            .as_str()
            .ok_or_else(|| anyhow!("signed-in-user id not a string"))?
            .to_string();
        Ok((oid, "User".into()))
    } else {
        let upn = az_json(&["account", "show", "--query", "user.name", "-o", "json"])?
            .as_str()
            .ok_or_else(|| anyhow!("user.name missing"))?
            .to_string();
        let v = az_json(&[
            "ad", "sp", "show", "--id", &upn, "--query", "id", "-o", "json",
        ])
        .context("az ad sp show for service principal")?;
        let oid = v
            .as_str()
            .ok_or_else(|| anyhow!("sp id not a string"))?
            .to_string();
        Ok((oid, "ServicePrincipal".into()))
    }
}

const DASHBOARDS: &[(&str, &str)] = &[
    (
        "azcluster-node-health",
        include_str!("../../../grafana/dashboards/node.json"),
    ),
    (
        "azcluster-slurm-scheduler",
        include_str!("../../../grafana/dashboards/slurm.json"),
    ),
    (
        "azcluster-gpu-ib",
        include_str!("../../../grafana/dashboards/gpu_ib.json"),
    ),
];

fn import_dashboards(resource_group: &str, grafana_name: &str) -> Result<()> {
    eprintln!(
        "==> importing {} Grafana dashboards into {}",
        DASHBOARDS.len(),
        grafana_name
    );
    let tmp_dir = std::env::temp_dir().join(format!("azcluster-dash-{}", std::process::id()));
    std::fs::create_dir_all(&tmp_dir).context("create tmp dashboard dir")?;
    for (slug, body) in DASHBOARDS {
        let dashboard: serde_json::Value =
            serde_json::from_str(body).with_context(|| format!("parse dashboard {slug}"))?;
        let envelope = serde_json::json!({
            "dashboard": dashboard,
            "overwrite": true,
            "folderId": 0,
        });
        let path = tmp_dir.join(format!("{slug}.json"));
        std::fs::write(&path, serde_json::to_vec(&envelope)?)
            .with_context(|| format!("write {}", path.display()))?;
        let definition_arg = format!("@{}", path.display());
        let mut imported = false;
        for attempt in 1..=10u32 {
            let output = Command::new("az")
                .args([
                    "grafana",
                    "dashboard",
                    "create",
                    "--name",
                    grafana_name,
                    "--resource-group",
                    resource_group,
                    "--definition",
                    &definition_arg,
                    "--overwrite",
                    "true",
                ])
                .output()
                .with_context(|| format!("spawn az grafana dashboard create for {slug}"))?;
            if output.status.success() {
                eprintln!("    imported {slug} (attempt {attempt})");
                imported = true;
                break;
            }
            let stderr = String::from_utf8_lossy(&output.stderr);
            let propagating = stderr.contains("NoRoleAssignedException")
                || stderr.contains("401")
                || stderr.contains("Unauthorized");
            if !propagating || attempt == 10 {
                eprintln!("    FAILED {slug}: {}", stderr.lines().last().unwrap_or(""));
                bail!("dashboard import {slug} failed after {attempt} attempt(s)");
            }
            eprintln!(
                "    waiting for Grafana Admin propagation (attempt {attempt}/10, sleeping 30s)..."
            );
            std::thread::sleep(std::time::Duration::from_secs(30));
        }
        if !imported {
            bail!("dashboard {slug} not imported");
        }
    }
    let _ = std::fs::remove_dir_all(&tmp_dir);
    Ok(())
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

fn ssh(args: ConnectArgs) -> Result<()> {
    let state = ClusterState::load(&args.name)?;
    let host = state.login_public_ip.as_deref().ok_or_else(|| {
        anyhow!(
            "cluster '{}' has no login public IP. Redeploy with --login-public-ip or use a jumpbox.",
            args.name
        )
    })?;
    let login_target = format!("{}@{}", state.admin_username, host);
    let forward = format!("{}:{}:8443", args.local_port, state.scheduler_private_ip);
    let mut cmd = Command::new("ssh");
    cmd.args(["-A", "-L", &forward]);
    if let Some(id) = &args.identity {
        cmd.args(["-i", &id.display().to_string()]);
    }
    if args.scheduler {
        let sched_target = format!("{}@{}", state.admin_username, state.scheduler_private_ip);
        cmd.args(["-J", &login_target, &sched_target]);
        eprintln!("==> ssh -J {login_target} {sched_target}");
    } else {
        cmd.arg(&login_target);
        eprintln!("==> ssh -L {forward} {login_target}");
    }
    let status = cmd.status().context("spawn ssh")?;
    std::process::exit(status.code().unwrap_or(1));
}

fn tunnel(args: ConnectArgs) -> Result<()> {
    let state = ClusterState::load(&args.name)?;
    let host = state.login_public_ip.as_deref().ok_or_else(|| {
        anyhow!(
            "cluster '{}' has no login public IP. Redeploy with --login-public-ip.",
            args.name
        )
    })?;
    let target = format!("{}@{}", state.admin_username, host);
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
    if let Some(id) = &args.identity {
        cmd.args(["-i", &id.display().to_string()]);
    }
    cmd.arg(&target);
    eprintln!(
        "==> tunnel localhost:{} -> {}:8443 (Ctrl-C to stop)",
        args.local_port, state.scheduler_private_ip
    );
    let status = cmd.status().context("spawn ssh")?;
    std::process::exit(status.code().unwrap_or(1));
}

fn scale(args: ScaleArgs) -> Result<()> {
    let state = ClusterState::load(&args.name)?;
    let url = format!("http://localhost:8443/v1/pools/{}/scale", args.pool);
    eprintln!(
        "==> POST {url} (requires `azcluster tunnel {}` to be running in another shell)",
        state.name
    );
    let res = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()?
        .post(&url)
        .json(&ScaleRequest { count: args.count })
        .send()?;
    let status = res.status();
    let body = res.text().unwrap_or_default();
    if !status.is_success() {
        bail!("scale request failed ({status}): {body}");
    }
    println!("{body}");
    Ok(())
}

fn status(args: StatusArgs) -> Result<()> {
    let state = ClusterState::load(&args.name)?;
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
        for vmss in &state.compute_vmss_names {
            print!("  {vmss}: ");
            std::io::Write::flush(&mut std::io::stdout()).ok();
            let out = Command::new("az")
                .args([
                    "vmss",
                    "show",
                    "--resource-group",
                    &state.resource_group,
                    "--name",
                    vmss,
                    "--query",
                    "sku.capacity",
                    "-o",
                    "tsv",
                ])
                .output();
            match out {
                Ok(o) if o.status.success() => {
                    println!("capacity={}", String::from_utf8_lossy(&o.stdout).trim())
                }
                Ok(o) => println!("ERR ({})", String::from_utf8_lossy(&o.stderr).trim()),
                Err(e) => println!("ERR ({e})"),
            }
        }
    }
    Ok(())
}

fn delete(args: DeleteArgs) -> Result<()> {
    ensure_az()?;
    let state = ClusterState::load(&args.name)?;
    if !args.yes {
        eprint!(
            "Delete resource group '{}' (cluster '{}')? Type cluster name to confirm: ",
            state.resource_group, state.name
        );
        std::io::Write::flush(&mut std::io::stderr()).ok();
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        if line.trim() != state.name {
            bail!("aborted");
        }
    }
    eprintln!(
        "==> az group delete --name {} --yes --no-wait",
        state.resource_group
    );
    let st = Command::new("az")
        .args([
            "group",
            "delete",
            "--name",
            &state.resource_group,
            "--yes",
            "--no-wait",
        ])
        .status()?;
    if !st.success() {
        bail!("az group delete failed");
    }
    let path = cluster_state::state_path(&state.name)?;
    if path.exists() {
        std::fs::remove_file(&path).ok();
        eprintln!("==> removed local state {}", path.display());
    }
    Ok(())
}

fn exec(args: ExecArgs) -> Result<()> {
    let state = ClusterState::load(&args.name)?;
    let host = state.login_public_ip.as_deref().ok_or_else(|| {
        anyhow!(
            "cluster '{}' has no login public IP. Redeploy with --login-public-ip.",
            args.name
        )
    })?;
    let login_target = format!("{}@{}", state.admin_username, host);
    let mut cmd = Command::new("ssh");
    if let Some(id) = &args.identity {
        cmd.args(["-i", &id.display().to_string()]);
    }
    if args.scheduler {
        let sched_target = format!("{}@{}", state.admin_username, state.scheduler_private_ip);
        cmd.args(["-J", &login_target, &sched_target]);
    } else {
        cmd.arg(&login_target);
    }
    cmd.arg("--");
    for part in &args.cmd {
        cmd.arg(part);
    }
    let status = cmd.status().context("spawn ssh exec")?;
    std::process::exit(status.code().unwrap_or(1));
}

fn logs(args: LogsArgs) -> Result<()> {
    let state = ClusterState::load(&args.name)?;
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
    if let Some(id) = &args.identity {
        cmd.args(["-i", &id.display().to_string()]);
    }
    cmd.arg(&login_target).arg(&remote_cmd);
    let status = cmd.status().context("spawn ssh logs")?;
    std::process::exit(status.code().unwrap_or(1));
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn validate(args: ValidateArgs) -> Result<()> {
    let state = ClusterState::load(&args.name)?;
    let host = state.login_public_ip.as_deref().ok_or_else(|| {
        anyhow!(
            "cluster '{}' has no login public IP. Redeploy with --login-public-ip.",
            args.name
        )
    })?;
    let login_target = format!("{}@{}", state.admin_username, host);

    let mut checks: Vec<(&str, String)> = vec![
        ("sinfo", "sinfo -h -o '%P %D %T %N'".into()),
        (
            "srun hostname",
            "timeout 60 srun -N1 --time=1 hostname".into(),
        ),
    ];
    if !args.no_container {
        checks.push((
            "srun pyxis alpine",
            "timeout 180 srun -N1 --time=2 --container-image=docker://alpine:latest hostname"
                .into(),
        ));
    }
    if args.gpu {
        checks.push((
            "srun gpu nvidia-smi",
            "timeout 180 srun -N1 --gres=gpu:1 --time=2 nvidia-smi -L".into(),
        ));
    }

    let mut all_ok = true;
    for (label, remote) in &checks {
        eprintln!("==> [{label}] {remote}");
        let mut cmd = Command::new("ssh");
        cmd.args(["-A", "-o", "StrictHostKeyChecking=accept-new"]);
        if let Some(id) = &args.identity {
            cmd.args(["-i", &id.display().to_string()]);
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
    let state = ClusterState::load(&args.name)?;
    let grafana_name = format!("amg-{}", state.name);
    let endpoint = az_json(&[
        "grafana",
        "show",
        "--name",
        &grafana_name,
        "--resource-group",
        &state.resource_group,
        "--query",
        "properties.endpoint",
        "-o",
        "json",
    ])
    .ok()
    .and_then(|v| v.as_str().map(String::from));
    match endpoint {
        Some(url) if !url.is_empty() => {
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
