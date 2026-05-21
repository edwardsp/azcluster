mod cluster_state;

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
    #[arg(long, default_value = "v0.1.0")]
    azcluster_version: String,
    #[arg(long, default_value = "edwardsp/azcluster")]
    azcluster_repo: String,
    #[arg(long, default_value = "2404")]
    ubuntu: String,
    #[arg(long, default_value_t = 2)]
    anf_size_tib: u32,
    #[arg(long, default_value = "Standard")]
    anf_tier: String,
    #[arg(long, default_value = "gpu")]
    compute_pool: String,
    #[arg(long, default_value = "Standard_ND96isr_H200_v5")]
    compute_sku: String,
    #[arg(long, default_value_t = 0)]
    compute_count: u32,
    #[arg(long)]
    template: Option<PathBuf>,
    #[arg(long, default_value_t = false)]
    what_if: bool,
}

#[derive(Args)]
struct ConnectArgs {
    name: String,
    #[arg(long, default_value_t = 8443)]
    local_port: u16,
    #[arg(long)]
    identity: Option<PathBuf>,
}

#[derive(Args)]
struct ScaleArgs {
    name: String,
    pool: String,
    count: u32,
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

    let params: Vec<(&str, String)> = vec![
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
        ("computePoolName", args.compute_pool.clone()),
        ("computeSku", args.compute_sku.clone()),
        ("computeCount", args.compute_count.to_string()),
    ];

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
        compute_vmss_name: pick("computeVmssName"),
    };
    let saved = state.save()?;
    eprintln!("==> saved cluster state -> {}", saved.display());
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
    let target = format!("{}@{}", state.admin_username, host);
    let forward = format!("{}:{}:8443", args.local_port, state.scheduler_private_ip);
    let mut cmd = Command::new("ssh");
    cmd.args(["-L", &forward]);
    if let Some(id) = &args.identity {
        cmd.args(["-i", &id.display().to_string()]);
    }
    cmd.arg(&target);
    eprintln!("==> ssh -L {forward} {target}");
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
