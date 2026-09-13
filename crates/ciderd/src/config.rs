use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub nfs_quotas: NfsQuotas,
    pub node: NodeConfig,
    pub heartbeat: HeartbeatConfig,
    pub collection: CollectionConfig,
    pub limits: Limits,
    #[serde(default)]
    pub tools: Tools,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeConfig {
    pub identity_file: PathBuf,
    pub state_directory: PathBuf,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeartbeatConfig {
    pub endpoint: String,
    pub bearer_token_file: PathBuf,
    /// Optional private trust roots added to the normal platform trust store.
    #[serde(default)]
    pub ca_certificate_file: Option<PathBuf>,
    pub interval_seconds: u64,
    pub connect_timeout_seconds: u64,
    pub request_timeout_seconds: u64,
    pub maximum_backoff_seconds: u64,
    pub maximum_request_bytes: usize,
    pub maximum_response_bytes: usize,
    pub delivery_mode: String,
    pub follow_redirects: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CollectionConfig {
    pub io_seconds: u64,
    pub mount_inventory_seconds: u64,
    pub nfs_counters_seconds: u64,
    pub nfs_status_seconds: u64,
    pub capacity_seconds: u64,
    pub smart_seconds: u64,
    pub inventory_reconcile_seconds: u64,
    pub apfs_accounting_seconds: u64,
    pub jitter_percent: u32,
    pub nfs_path_refresh_enabled: bool,
    pub active_write_probes_enabled: bool,
    pub diagnostic_jobs_enabled: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    pub command_workers: usize,
    pub native_workers: usize,
    pub remote_workers: usize,
    pub workers_per_resource: usize,
    pub stdout_bytes: usize,
    pub stderr_bytes: usize,
    pub pending_events: usize,
    pub recent_sample_history: usize,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tools {
    pub smartctl: Option<PathBuf>,
}

impl Config {
    pub fn parse(input: &str) -> Result<Self> {
        let config: Self = toml::from_str(input).context("invalid configuration")?;
        config.validate()?;
        Ok(config)
    }

    pub fn load(path: &Path) -> Result<Self> {
        let input = crate::config::read_bounded(path, 65536)?;
        Self::parse(std::str::from_utf8(&input).context("configuration is not UTF-8")?)
    }

    pub fn validate(&self) -> Result<()> {
        self.nfs_quotas.validate()?;
        let h = &self.heartbeat;
        let url = reqwest::Url::parse(&h.endpoint).context("invalid heartbeat endpoint")?;
        ensure!(
            url.scheme() == "https" && url.host_str().is_some(),
            "heartbeat endpoint requires HTTPS"
        );
        ensure!(
            url.username().is_empty()
                && url.password().is_none()
                && url.fragment().is_none()
                && url.query().is_none(),
            "endpoint cannot contain credentials, a query, or a fragment"
        );
        ensure!(!h.follow_redirects, "heartbeat redirects are disabled");
        ensure!(
            h.delivery_mode == "latest",
            "only latest-state delivery is supported"
        );
        ensure!(
            (1..=3600).contains(&h.interval_seconds),
            "heartbeat interval must be 1..3600 seconds"
        );
        ensure!(
            (1..=300).contains(&h.request_timeout_seconds)
                && h.connect_timeout_seconds > 0
                && h.connect_timeout_seconds <= h.request_timeout_seconds,
            "invalid heartbeat deadlines"
        );
        ensure!(
            h.maximum_backoff_seconds >= h.interval_seconds && h.maximum_backoff_seconds <= 86400,
            "invalid maximum backoff"
        );
        ensure!(
            (4096..=16_777_216).contains(&h.maximum_request_bytes),
            "request byte limit must be 4096..16777216"
        );
        ensure!(
            (512..=65536).contains(&h.maximum_response_bytes),
            "response byte limit must be 512..65536"
        );
        for path in [
            &self.node.identity_file,
            &self.node.state_directory,
            &h.bearer_token_file,
        ] {
            ensure!(path.is_absolute(), "configuration paths must be absolute");
        }
        if let Some(path) = &self.tools.smartctl {
            ensure!(path.is_absolute(), "smartctl path must be absolute");
        }
        if let Some(path) = &h.ca_certificate_file {
            ensure!(path.is_absolute(), "CA certificate path must be absolute");
        }
        let c = &self.collection;
        for seconds in [
            c.io_seconds,
            c.mount_inventory_seconds,
            c.nfs_counters_seconds,
            c.nfs_status_seconds,
            c.capacity_seconds,
            c.smart_seconds,
            c.inventory_reconcile_seconds,
            c.apfs_accounting_seconds,
        ] {
            ensure!(
                (1..=201600).contains(&seconds),
                "collector interval must be 1..201600 seconds"
            );
        }
        ensure!(c.jitter_percent <= 50, "jitter must be 0..50 percent");
        ensure!(
            !c.active_write_probes_enabled && !c.diagnostic_jobs_enabled,
            "active probes and diagnostic jobs are not implemented"
        );
        let l = &self.limits;
        for count in [l.command_workers, l.native_workers, l.remote_workers] {
            ensure!((1..=32).contains(&count), "worker limits must be 1..32");
        }
        ensure!(
            l.workers_per_resource == 1,
            "exactly one worker per scope is required"
        );
        ensure!(
            (1024..=16_777_216).contains(&l.stdout_bytes)
                && (256..=1_048_576).contains(&l.stderr_bytes),
            "invalid command output limits"
        );
        ensure!(
            (1..=4096).contains(&l.pending_events),
            "pending_events must be 1..4096"
        );
        ensure!(
            l.recent_sample_history == 0,
            "historical delivery is not implemented"
        );
        Ok(())
    }
}

pub fn read_bounded(path: &Path, limit: usize) -> Result<Vec<u8>> {
    use std::io::Read;
    let file = std::fs::File::open(path).context("cannot open local configuration/state file")?;
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= limit, "local file exceeds byte limit");
    Ok(bytes)
}

pub fn seconds(n: u64) -> Duration {
    Duration::from_secs(n)
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NfsQuotas {
    pub targets: Vec<crate::quota::QuotaTarget>,
    pub interval_seconds: u64,
    pub timeout_seconds: u64,
}
impl Default for NfsQuotas {
    fn default() -> Self {
        Self {
            targets: Vec::new(),
            interval_seconds: 15,
            timeout_seconds: 3,
        }
    }
}
impl NfsQuotas {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.targets.len() <= 32,
            "at most 32 configured quota subjects"
        );
        ensure!(
            (15..=3600).contains(&self.interval_seconds),
            "quota interval must be 15..3600 seconds"
        );
        ensure!(
            (1..=10).contains(&self.timeout_seconds),
            "quota timeout must be 1..10 seconds"
        );
        let mut subjects = std::collections::BTreeSet::new();
        for t in &self.targets {
            t.validate()?;
            ensure!(
                subjects.insert((&t.server, &t.export_path, t.uid)),
                "duplicate quota subject"
            );
        }
        Ok(())
    }
}
