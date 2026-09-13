mod config;
mod exports;
mod mount;
mod sys;

use anyhow::Result;
use clap::{Parser, Subcommand};
use config::Config;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "simple-nfs-server",
    version,
    about = "Basic macOS NFS export and mount management"
)]
struct Cli {
    /// Path to the TOML configuration.
    #[arg(
        long,
        env = "SIMPLE_NFS_SERVER_CONFIG",
        default_value = "config.toml",
        global = true
    )]
    config: PathBuf,

    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Write the export to /etc/exports, enable and start nfsd. Requires root.
    SetupServer,
    /// Remove the export from /etc/exports. Requires root.
    TeardownServer {
        /// Also stop and disable nfsd, if no other exports remain.
        #[arg(long)]
        stop_nfsd: bool,
    },
    /// Mount the configured export and report observed mount flags. Requires root.
    Mount,
    /// Unmount the export. Requires root.
    Unmount,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with_target(false)
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    match cli.cmd {
        Cmd::SetupServer => exports::setup(&Config::load(&cli.config)?).await,
        Cmd::TeardownServer { stop_nfsd } => exports::teardown(stop_nfsd).await,
        Cmd::Mount => mount::mount(&Config::load(&cli.config)?).await,
        Cmd::Unmount => mount::unmount(&Config::load(&cli.config)?).await,
    }
}
