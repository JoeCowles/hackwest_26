mod config;
mod daemon;
mod exports;
mod monitor;
mod mount;
mod sys;

use anyhow::Result;
use clap::{Parser, Subcommand};
use config::Config;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "orchard-nfs", version, about = "Node-side NFS provisioning and telemetry for Orchard")]
struct Cli {
    /// Path to the TOML configuration.
    #[arg(long, env = "ORCHARD_NFS_CONFIG", default_value = "config.toml", global = true)]
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
    /// Mount the export on this client and report the negotiated options. Requires root.
    Mount,
    /// Unmount the export. Requires root.
    Unmount,
    /// Sample NFS mounts and counters, emitting newline-delimited JSON.
    Monitor {
        /// Take a single sample and exit.
        #[arg(long)]
        once: bool,
    },
    /// Install the monitor as a launchd LaunchDaemon. Requires root.
    InstallDaemon,
    /// Remove the launchd LaunchDaemon. Requires root.
    UninstallDaemon,
    /// Report whether the LaunchDaemon is loaded.
    DaemonStatus,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
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
        Cmd::Monitor { once: true } => monitor::once(&Config::load(&cli.config)?).await,
        Cmd::Monitor { once: false } => monitor::run_loop(&Config::load(&cli.config)?).await,
        Cmd::InstallDaemon => daemon::install(&cli.config).await,
        Cmd::UninstallDaemon => daemon::uninstall().await,
        Cmd::DaemonStatus => daemon::status().await,
    }
}
