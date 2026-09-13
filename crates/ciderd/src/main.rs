use anyhow::{ensure, Context, Result};
use ciderd::{config::Config, identity, platform, runtime};
use clap::{Parser, Subcommand};
use std::{
    io::{Read, Write},
    path::PathBuf,
    time::Duration,
};

#[derive(Parser)]
#[command(
    name = "ciderd",
    version,
    about = "macOS storage telemetry and latest-state HTTPS heartbeats"
)]
struct Cli {
    #[command(subcommand)]
    mode: Mode,
}

#[derive(Subcommand)]
enum Mode {
    /// Run in the foreground; launchd owns process lifetime.
    Run {
        #[arg(long)]
        config: PathBuf,
        /// Permit local development without root. Individual collectors may fail.
        #[arg(long)]
        allow_unprivileged: bool,
    },
    /// Check typed configuration without collecting or contacting the server.
    ValidateConfig {
        #[arg(long)]
        config: PathBuf,
    },
    /// Create a new local enrollment identity; never overwrite an existing one.
    Enroll {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        node_id: Option<String>,
    },
    /// Enroll with Orchard and securely store the matching node identity and credential.
    EnrollServer {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        name: String,
        #[arg(long)]
        enrollment_token_file: PathBuf,
    },
    /// Collect a bounded offline snapshot; no credentials or network delivery.
    Snapshot {
        #[arg(long)]
        config: Option<PathBuf>,
        #[arg(long,default_value_t=3,value_parser=clap::value_parser!(u64).range(1..=60))]
        seconds: u64,
    },
    #[command(hide = true)]
    Worker,
}

fn main() {
    if let Err(error) = execute(Cli::parse()) {
        eprintln!("ciderd: {error:#}");
        std::process::exit(1);
    }
}

fn execute(cli: Cli) -> Result<()> {
    match cli.mode {
        Mode::Worker => {
            // No Tokio runtime or daemon identity is created in the native worker.
            let mut input = Vec::new();
            std::io::stdin()
                .take(platform::MAX_WORKER_INPUT as u64 + 1)
                .read_to_end(&mut input)?;
            let request = platform::parse_worker_request(&input)?;
            let output = platform::worker(request)?;
            ensure!(
                output.len() <= platform::MAX_WORKER_OUTPUT,
                "worker output exceeded byte limit"
            );
            std::io::stdout().write_all(&output)?;
        }
        Mode::ValidateConfig { config } => {
            Config::load(&config)?;
            println!("Configuration valid.");
        }
        Mode::Enroll { config, node_id } => {
            let config = Config::load(&config)?;
            std::fs::create_dir_all(&config.node.state_directory)?;
            let node = identity::enroll(&config.node.identity_file, node_id.as_deref())?;
            println!("Enrolled {node}. Configure the matching node credential before running.");
        }
        Mode::EnrollServer { config, name, enrollment_token_file } => {
            let config = Config::load(&config)?;
            let rt = make_runtime()?;
            let result = rt.block_on(ciderd::orchard::enroll(&config, &name, &enrollment_token_file));
            rt.shutdown_timeout(Duration::from_secs(1));
            println!("Enrolled node {}. Identity and credential stored with owner-only permissions.", result?);
        }
        Mode::Snapshot { config, seconds } => {
            let config = match config {
                Some(path) => Config::load(&path)?,
                None => Config::parse(include_str!("../examples/ciderd.toml"))?,
            };
            let rt = make_runtime()?;
            let result = rt.block_on(runtime::snapshot(config, Duration::from_secs(seconds)));
            rt.shutdown_timeout(Duration::from_secs(1));
            serde_json::to_writer(std::io::stdout().lock(), &result?)?;
            println!();
        }
        Mode::Run {
            config,
            allow_unprivileged,
        } => {
            let path = config;
            let config = Config::load(&path)?;
            let executable = std::env::current_exe()?;
            runtime::validate_installation(&config, &executable, allow_unprivileged)?;
            // A root daemon's configuration has the same trust boundary as its executable.
            if platform::effective_uid() == 0 {
                let mut check = config.clone();
                check.node.identity_file = path;
                runtime::validate_installation(&check, &executable, false)?;
            }
            let rt = make_runtime()?;
            let result = rt.block_on(async {
                #[cfg(unix)]
                {
                    let mut term =
                        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                            .context("cannot install SIGTERM handler")?;
                    let shutdown = async move {
                        tokio::select! {_=term.recv()=>{},_=tokio::signal::ctrl_c()=>{}}
                    };
                    runtime::run(config, shutdown).await
                }
                #[cfg(not(unix))]
                {
                    runtime::run(config, async {
                        let _ = tokio::signal::ctrl_c().await;
                    })
                    .await
                }
            });
            rt.shutdown_timeout(Duration::from_secs(1));
            result?;
        }
    }
    Ok(())
}
fn make_runtime() -> Result<tokio::runtime::Runtime> {
    Ok(tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .max_blocking_threads(8)
        .enable_all()
        .build()?)
}
