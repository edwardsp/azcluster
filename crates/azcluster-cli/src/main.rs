use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
use serde::Serialize;

#[derive(Parser)]
#[command(name = "azcluster", version = azcluster_core::VERSION, about = "Manage Slurm clusters on Azure")]
struct Cli {
    #[arg(
        long,
        env = "AZCLUSTER_URL",
        default_value = "http://localhost:8443",
        global = true
    )]
    scheduler_url: String,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Version,
    Scale {
        pool: String,
        count: u32,
    },
}

#[derive(Serialize)]
struct ScaleRequest {
    count: u32,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Version => {
            println!("azcluster {}", azcluster_core::VERSION);
        }
        Command::Scale { pool, count } => scale(&cli.scheduler_url, &pool, count)?,
    }
    Ok(())
}

fn scale(base_url: &str, pool: &str, count: u32) -> Result<()> {
    let url = format!(
        "{}/v1/pools/{}/scale",
        base_url.trim_end_matches('/'),
        pool
    );
    let res = reqwest::blocking::Client::new()
        .post(&url)
        .json(&ScaleRequest { count })
        .send()?;

    let status = res.status();
    let body = res.text().unwrap_or_default();
    if !status.is_success() {
        bail!("scale request failed ({status}): {body}");
    }
    println!("{body}");
    Ok(())
}
