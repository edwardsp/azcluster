mod checks;
mod metrics;
mod types;

use clap::Parser;
use std::path::PathBuf;
use std::process::ExitCode;
use types::{CheckOutcome, RealRunner, Severity};

#[derive(Parser)]
#[command(
    name = "azhealthcheck",
    version,
    about = "Node health checks for Slurm HealthCheckProgram on Azure compute nodes"
)]
struct Cli {
    /// Comma-separated list of checks to run. Default: all.
    /// Available: gpu_count, gpu_xid, network, kmsg, systemd.
    #[arg(long, value_delimiter = ',')]
    checks: Vec<String>,
    /// Services to check with the `systemd` check (comma-separated).
    #[arg(long, value_delimiter = ',')]
    services: Vec<String>,
    /// Emit JSON to stdout instead of human-readable lines.
    #[arg(long, default_value_t = false)]
    json: bool,
    /// Override sysfs root (default `/sys`). For testing.
    #[arg(long, default_value = "/sys")]
    sys_root: PathBuf,
    /// Override /dev root (default `/dev`). For testing.
    #[arg(long, default_value = "/dev")]
    dev_root: PathBuf,
    /// Write a Prometheus textfile collector exposition file
    /// (`azhealthcheck.prom`) into this directory after running checks.
    /// Intended for `node_exporter --collector.textfile.directory=<path>`.
    #[arg(long)]
    metrics_dir: Option<PathBuf>,
    /// Override the `host` label written into the metrics file. Defaults to
    /// the system hostname; falls back to `unknown` if both are unavailable.
    #[arg(long)]
    metrics_host: Option<String>,
}

const ALL_CHECKS: &[&str] = &["gpu_count", "gpu_xid", "network", "kmsg", "systemd"];

fn main() -> ExitCode {
    let cli = Cli::parse();
    let runner = RealRunner;
    let requested: Vec<String> = if cli.checks.is_empty() {
        ALL_CHECKS.iter().map(|s| s.to_string()).collect()
    } else {
        cli.checks.clone()
    };

    let mut outcomes: Vec<CheckOutcome> = Vec::with_capacity(requested.len());
    for c in &requested {
        let o = match c.as_str() {
            "gpu_count" => checks::gpu_count(&cli.sys_root, &cli.dev_root),
            "gpu_xid" => checks::gpu_xid(&runner),
            "network" => checks::network(&cli.sys_root),
            "kmsg" => checks::kmsg(&runner),
            "systemd" => checks::systemd(&runner, &cli.services),
            other => CheckOutcome::error(
                "unknown",
                format!(
                    "unknown check '{other}'; available: {}",
                    ALL_CHECKS.join(",")
                ),
                vec![],
            ),
        };
        outcomes.push(o);
    }

    let worst = outcomes
        .iter()
        .map(|o| o.severity)
        .max()
        .unwrap_or(Severity::Ok);

    if let Some(dir) = cli.metrics_dir.as_deref() {
        let host = cli
            .metrics_host
            .clone()
            .or_else(|| {
                std::fs::read_to_string("/etc/hostname")
                    .ok()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
            })
            .unwrap_or_else(|| "unknown".to_string());
        let body = metrics::render(&host, &outcomes, metrics::now_unix_seconds());
        if let Err(e) = metrics::write_atomic(dir, &body) {
            eprintln!("warning: failed to write metrics file: {e:#}");
        }
    }

    if cli.json {
        let body = serde_json::json!({
            "severity": worst,
            "checks": outcomes,
        });
        println!("{}", serde_json::to_string_pretty(&body).unwrap());
    } else {
        for o in &outcomes {
            let tag = match o.severity {
                Severity::Ok => "OK   ",
                Severity::Warning => "WARN ",
                Severity::Error => "ERROR",
            };
            println!("{tag} {}: {}", o.name, o.message);
            for f in &o.findings {
                println!("        - {f}");
            }
        }
    }

    ExitCode::from(worst.exit_code() as u8)
}
