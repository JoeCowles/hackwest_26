//! Sampling loop. Emits one JSON object per sample as newline-delimited JSON.
//!
//! Fields that could not be read are `null` with a state, never zero: an
//! unreadable mount is not an empty one, and a consumer must be able to tell
//! "zero free space" from "could not read free space".

use crate::config::Config;
use crate::mount::{NfsMount, list_nfs_mounts};
use crate::sys::{self, Bounded};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use std::io::Write;
use std::time::Duration;
use tracing::warn;

#[derive(Debug, Serialize)]
pub struct Sample {
    pub ts: DateTime<Utc>,
    pub host: String,
    pub mounts: Vec<MountSample>,
    /// `nfsstat -c -f JSON`, embedded verbatim. Node-wide, not per mount.
    pub nfs_client: Option<Value>,
    /// `nfsstat -s -f JSON`, embedded verbatim. All 21 per-operation counters are
    /// kept: the ratio between Read, Write, Remove and Rename is what separates
    /// bulk copy from mass deletion from encryption in place, and collapsing it
    /// here would throw that away before anything can use it.
    pub nfs_server: Option<Value>,
    /// APFS local snapshot count on the boot volume. A drop is a ransomware precursor.
    pub apfs_snapshots: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct MountSample {
    pub source: String,
    pub mount_point: String,
    pub options: String,
    pub state: MountState,
    pub capacity_kb: Option<u64>,
    pub used_kb: Option<u64>,
    pub available_kb: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MountState {
    Ok,
    NotResponding,
    Error,
}

/// Parse the data row of `df -k <path>`: 1024-blocks, used, available.
pub fn parse_df_k(out: &str) -> Option<(u64, u64, u64)> {
    let row = out.lines().nth(1)?;
    let mut f = row.split_whitespace().skip(1);
    let total = f.next()?.parse().ok()?;
    let used = f.next()?.parse().ok()?;
    let avail = f.next()?.parse().ok()?;
    Some((total, used, avail))
}

/// Count snapshot entries in `tmutil listlocalsnapshots` output.
pub fn parse_snapshot_count(out: &str) -> usize {
    out.lines()
        .filter(|l| l.trim_start().starts_with("com.apple."))
        .count()
}

async fn sample_mount(m: NfsMount, timeout: Duration) -> MountSample {
    let (state, cap) = match sys::run_bounded(timeout, "df", &["-k", &m.mount_point]).await {
        Ok(Bounded::TimedOut) => (MountState::NotResponding, None),
        Ok(Bounded::Finished(out)) if out.status.success() => {
            let text = String::from_utf8_lossy(&out.stdout);
            match parse_df_k(&text) {
                Some(c) => (MountState::Ok, Some(c)),
                None => (MountState::Error, None),
            }
        }
        _ => (MountState::Error, None),
    };
    MountSample {
        source: m.source,
        mount_point: m.mount_point,
        options: m.options,
        state,
        capacity_kb: cap.map(|c| c.0),
        used_kb: cap.map(|c| c.1),
        available_kb: cap.map(|c| c.2),
    }
}

async fn nfsstat_json(flag: &str, timeout: Duration) -> Option<Value> {
    match sys::run_bounded(timeout, "nfsstat", &[flag, "-f", "JSON"]).await {
        Ok(Bounded::Finished(out)) if out.status.success() => {
            serde_json::from_slice(&out.stdout).ok()
        }
        _ => None,
    }
}

async fn snapshot_count(timeout: Duration) -> Option<usize> {
    match sys::run_bounded(timeout, "tmutil", &["listlocalsnapshots", "/"]).await {
        Ok(Bounded::Finished(out)) if out.status.success() => {
            Some(parse_snapshot_count(&String::from_utf8_lossy(&out.stdout)))
        }
        _ => None,
    }
}

pub async fn take_sample(cfg: &Config) -> Result<Sample> {
    let timeout = Duration::from_secs(cfg.monitor.probe_timeout_secs);

    let mut mounts = Vec::new();
    for m in list_nfs_mounts().await.unwrap_or_default() {
        mounts.push(sample_mount(m, timeout).await);
    }

    let (nfs_client, nfs_server, apfs_snapshots) = tokio::join!(
        nfsstat_json("-c", timeout),
        nfsstat_json("-s", timeout),
        snapshot_count(timeout),
    );

    Ok(Sample {
        ts: Utc::now(),
        host: hostname(),
        mounts,
        nfs_client,
        nfs_server,
        apfs_snapshots,
    })
}

fn hostname() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".into())
}

async fn publish(cfg: &Config, client: &reqwest::Client, sample: &Sample) -> Result<()> {
    let line = serde_json::to_string(sample)?;

    if let Some(path) = &cfg.monitor.metrics_log {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .with_context(|| format!("opening {}", path.display()))?;
        writeln!(f, "{line}")?;
    }

    if let Some(url) = &cfg.monitor.metrics_endpoint {
        let res = client
            .post(url)
            .json(sample)
            .timeout(Duration::from_secs(10))
            .send()
            .await;
        match res {
            Ok(r) if r.status().is_success() => {}
            Ok(r) => warn!("POST {url} returned {}", r.status()),
            Err(e) => warn!("POST {url} failed: {e}"),
        }
    }
    Ok(())
}

pub async fn once(cfg: &Config) -> Result<()> {
    let s = take_sample(cfg).await?;
    println!("{}", serde_json::to_string(&s)?);
    Ok(())
}

pub async fn run_loop(cfg: &Config) -> Result<()> {
    let client = reqwest::Client::new();
    let interval = Duration::from_secs(cfg.monitor.sample_interval_secs);
    let mut shutdown = std::pin::pin!(shutdown_signal());

    loop {
        let s = take_sample(cfg).await?;
        println!("{}", serde_json::to_string(&s)?);
        if let Err(e) = publish(cfg, &client, &s).await {
            warn!("publish failed: {e:#}");
        }

        tokio::select! {
            _ = tokio::time::sleep(interval) => {}
            _ = &mut shutdown => {
                tracing::info!("shutting down");
                return Ok(());
            }
        }
    }
}

async fn shutdown_signal() {
    use tokio::signal::unix::{SignalKind, signal};
    let mut term = signal(SignalKind::terminate()).expect("install SIGTERM handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = term.recv() => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_macos_df_k() {
        let out = "Filesystem    1024-blocks      Used Available Capacity iused ifree %iused  Mounted on\n\
                   srv:/export    1953514584 421337152 1532177432    22%  ...\n";
        assert_eq!(parse_df_k(out), Some((1953514584, 421337152, 1532177432)));
    }

    #[test]
    fn df_without_data_row_is_none() {
        assert_eq!(parse_df_k("Filesystem 1024-blocks Used Available\n"), None);
        assert_eq!(parse_df_k(""), None);
    }

    #[test]
    fn counts_snapshots() {
        let out = "Snapshots for volume group containing disk /:\n\
                   com.apple.os.update-AAA\n\
                   com.apple.TimeMachine.2026-09-12-120000.local\n";
        assert_eq!(parse_snapshot_count(out), 2);
        assert_eq!(parse_snapshot_count("Snapshots for volume group containing disk /:\n"), 0);
    }

    #[test]
    fn mount_state_serializes_snake_case() {
        assert_eq!(
            serde_json::to_string(&MountState::NotResponding).unwrap(),
            "\"not_responding\""
        );
    }
}
