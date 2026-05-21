use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "azcluster", version = azcluster_core::VERSION, about = "Manage Slurm clusters on Azure")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Version,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Version => {
            println!("azcluster {}", azcluster_core::VERSION);
        }
    }
    Ok(())
}
