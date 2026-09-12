use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::OnceLock,
};

pub type Attributes = BTreeMap<String, Value>;

/// Canonical, nonnegative u128 encoded as a JSON decimal string.
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Decimal(u128);
impl Decimal {
    pub fn get(self) -> u128 {
        self.0
    }
}
impl From<u128> for Decimal {
    fn from(n: u128) -> Self {
        Self(n)
    }
}
impl From<u64> for Decimal {
    fn from(n: u64) -> Self {
        Self(n as u128)
    }
}
impl From<Decimal> for String {
    fn from(n: Decimal) -> Self {
        n.0.to_string()
    }
}
impl std::fmt::Display for Decimal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}
impl TryFrom<String> for Decimal {
    type Error = anyhow::Error;
    fn try_from(s: String) -> Result<Self> {
        ensure!(
            !s.is_empty()
                && s.bytes().all(|b| b.is_ascii_digit())
                && (s == "0" || !s.starts_with('0')),
            "noncanonical integer"
        );
        Ok(Self(s.parse().context("integer exceeds u128")?))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Metric {
    pub name: String,
    pub kind: String,
    pub unit: String,
    pub availability: String,
    pub attributes: Attributes,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub freshness: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub value_type: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub value: Option<Value>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub counter_epoch: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub reason: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub source_field: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_value"
    )]
    pub effective_at: Option<Value>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub extensions: Option<Attributes>,
}
fn present_value<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<Option<Value>, D::Error> {
    Value::deserialize(d).map(Some)
}

#[derive(Debug, Deserialize)]
pub struct MetricDefinition {
    pub name: String,
    pub kind: String,
    pub unit: String,
    pub value_type: String,
    pub scope: Vec<String>,
    pub collector: String,
    pub source_field: String,
    pub attributes: BTreeMap<String, Value>,
}
pub fn catalog() -> &'static BTreeMap<String, MetricDefinition> {
    static CATALOG: OnceLock<BTreeMap<String, MetricDefinition>> = OnceLock::new();
    CATALOG.get_or_init(|| {
        #[derive(Deserialize)]
        struct Catalog {
            metrics: Vec<MetricDefinition>,
        }
        let data: Catalog = serde_json::from_str(include_str!("../contract/metric-catalog.json"))
            .expect("embedded catalog");
        data.metrics
            .into_iter()
            .map(|m| (m.name.clone(), m))
            .collect()
    })
}

impl Metric {
    /// Build a reading before adapters attach its catalog dimensions. Collection
    /// validation is the strict boundary for these partially built readings.
    pub fn reading(name: &str, value: Value, freshness: &str, epoch: Option<&str>) -> Result<Self> {
        let def = catalog().get(name).context("unknown metric")?;
        let metric = Self {
            name: name.into(),
            kind: def.kind.clone(),
            unit: def.unit.clone(),
            availability: "available".into(),
            attributes: Attributes::new(),
            freshness: Some(freshness.into()),
            value_type: Some(def.value_type.clone()),
            value: Some(value),
            counter_epoch: epoch.map(Into::into),
            reason: None,
            source_field: Some(def.source_field.clone()),
            effective_at: None,
            extensions: None,
        };
        metric.validate_fields(false)?;
        Ok(metric)
    }
    pub fn reading_with_attributes(
        name: &str,
        value: Value,
        freshness: &str,
        epoch: Option<&str>,
        attributes: Attributes,
    ) -> Result<Self> {
        let mut metric = Self::reading(name, value, freshness, epoch)?;
        metric.attributes = attributes;
        metric.validate()?;
        Ok(metric)
    }
    pub fn integer(name: &str, n: u128, freshness: &str, epoch: Option<&str>) -> Result<Self> {
        Self::reading(name, Value::String(n.to_string()), freshness, epoch)
    }
    pub fn key(&self) -> String {
        format!(
            "{}:{}",
            self.name,
            serde_json::to_string(&self.attributes).expect("JSON attributes")
        )
    }
    pub fn validate(&self) -> Result<()> {
        self.validate_fields(true)
    }
    fn validate_fields(&self, require_attributes: bool) -> Result<()> {
        let def = catalog().get(&self.name).context("metric not in catalog")?;
        ensure!(
            self.kind == def.kind && self.unit == def.unit,
            "metric kind/unit mismatch"
        );
        ensure!(
            [
                "available",
                "unsupported",
                "permission_denied",
                "timeout",
                "failed",
                "parse_error",
                "not_collected"
            ]
            .contains(&self.availability.as_str()),
            "invalid availability"
        );
        if let Some(f) = &self.freshness {
            ensure!(
                ["live", "cached", "stale", "unknown"].contains(&f.as_str()),
                "invalid freshness"
            );
        }
        ensure!(self.attributes.len() <= 32, "too many metric attributes");
        if require_attributes {
            ensure!(
                self.attributes.keys().eq(def.attributes.keys()),
                "missing or unexpected metric attributes"
            );
        }
        for (k, v) in &self.attributes {
            let rule = def.attributes.get(k).context("unknown metric attribute")?;
            ensure!(
                v.is_string() || v.is_boolean() || v.is_number(),
                "invalid attribute value"
            );
            if let Value::Array(values) = rule {
                ensure!(values.contains(v), "attribute outside allowlist");
            } else {
                self.validate_dynamic_attribute(k, v)?;
            }
            if let Some(s) = v.as_str() {
                check_text(s, 128)?;
            }
            check_json(v)?;
        }
        check_optional_text(self.reason.as_deref(), 4096)?;
        check_optional_text(self.source_field.as_deref(), 4096)?;
        if let Some(epoch) = &self.counter_epoch {
            check_id(epoch)?;
        }
        if let Some(time) = &self.effective_at {
            if !time.is_null() {
                check_time(
                    time.as_str()
                        .context("effective_at must be a timestamp or null")?,
                )?;
            }
        }
        check_extensions(self.extensions.as_ref(), 16)?;
        if self.availability != "available" {
            ensure!(
                self.value.is_none() && self.value_type.is_none() && self.freshness.is_none(),
                "unavailable measurement has measurement fields"
            );
            return Ok(());
        }
        ensure!(self.freshness.is_some(), "available metric lacks freshness");
        ensure!(
            self.value_type.as_deref() == Some(def.value_type.as_str()),
            "metric value type mismatch"
        );
        let value = self
            .value
            .as_ref()
            .context("available metric lacks value")?;
        match def.value_type.as_str() {
            "integer" => {
                let integer = value.as_str().context("integer must be a string")?;
                if self.kind != "counter" && integer.starts_with('-') {
                    let magnitude = &integer[1..];
                    ensure!(
                        !magnitude.is_empty()
                            && !magnitude.starts_with('0')
                            && magnitude.bytes().all(|b| b.is_ascii_digit()),
                        "noncanonical signed integer"
                    );
                    integer
                        .parse::<i128>()
                        .context("integer gauge below i128 minimum")?;
                } else {
                    Decimal::try_from(integer.to_owned())?;
                }
            }
            "number" => ensure!(
                value.as_f64().is_some_and(f64::is_finite),
                "invalid numeric gauge"
            ),
            "boolean" => ensure!(value.is_boolean(), "invalid Boolean state"),
            "string" => check_text(value.as_str().context("invalid string state")?, 4096)?,
            _ => bail!("unknown value type"),
        }
        if self.kind == "counter" {
            check_id(
                self.counter_epoch
                    .as_deref()
                    .context("counter lacks epoch")?,
            )?;
        }
        Ok(())
    }
    fn validate_dynamic_attribute(&self, key: &str, value: &Value) -> Result<()> {
        let value = value
            .as_str()
            .context("metric dimension must be a string")?;
        let allowed = match (self.name.as_str(), key) {
            ("storage.nfs.client.operations_total", "operation") => {
                let family = self
                    .attributes
                    .get("family")
                    .and_then(Value::as_str)
                    .context("NFS operation lacks family")?;
                let operations: &[&str] = match family {
                    "v3_procedure" => NFS_V3_OPERATIONS,
                    "v4_rpc" => &["null", "compound"],
                    "v4_operation" => NFS_V4_OPERATIONS,
                    "v4_callback" => NFS_V4_CALLBACK_OPERATIONS,
                    _ => bail!("unknown NFS operation family"),
                };
                // Preserve the supplied synthetic READ series exactly. Actual
                // adapters emit the lowercase Apple source names below.
                (family == "v3_procedure" && value == "READ") || operations.contains(&value)
            }
            (
                "storage.nfs.client.cache_hits_total" | "storage.nfs.client.cache_misses_total",
                "cache",
            ) => NFS_CACHE_FAMILIES.contains(&value),
            _ => bail!("metric dimension has no enabled adapter allowlist"),
        };
        ensure!(allowed, "metric dimension outside adapter allowlist");
        Ok(())
    }
}

// Apple nfsstat source labels, lowercased with spaces replaced by underscores.
// https://github.com/apple-oss-distributions/NFS/blob/main/nfsstat/nfsstat.c
pub const NFS_V3_OPERATIONS: &[&str] = &[
    "null", "getattr", "setattr", "lookup", "access", "readlink", "read", "write", "create",
    "mkdir", "symlink", "mknod", "remove", "rmdir", "rename", "link", "readdir", "rdirplus",
    "fsstat", "fsinfo", "pathconf", "commit",
];
pub const NFS_V4_OPERATIONS: &[&str] = &[
    "access",
    "close",
    "commit",
    "create",
    "delegpurge",
    "delegreturn",
    "getattr",
    "getfh",
    "link",
    "lock",
    "lockt",
    "locku",
    "lookup",
    "lookupp",
    "nverify",
    "open",
    "openattr",
    "open_conf",
    "open_dgrd",
    "putfh",
    "putpubfh",
    "putrootfh",
    "read",
    "readdir",
    "readlink",
    "remove",
    "rename",
    "renew",
    "restorefh",
    "savefh",
    "secinfo",
    "setattr",
    "setclientid",
    "confirm",
    "verify",
    "write",
    "rel_lkowner",
    "backchannelctl",
    "bindconntosess",
    "exchangeid",
    "createsession",
    "destroysession",
    "freestateid",
    "getdirdeleg",
    "getdevinfo",
    "getdevlist",
    "layoutcommit",
    "layoutget",
    "layoutreturn",
    "secinfononame",
    "sequence",
    "setssv",
    "teststateid",
    "wantdeleg",
    "destroyclientid",
    "reclaimcompl",
];
pub const NFS_V4_CALLBACK_OPERATIONS: &[&str] = &[
    "cb_getattr",
    "cb_recall",
    "cb_layoutrecall",
    "cb_notify",
    "cb_pushdeleg",
    "cb_recallany",
    "cb_recallobjavail",
    "cb_recallslot",
    "cb_sequence",
    "cb_wantcancelled",
    "cb_notifylock",
    "cb_notifydevid",
];
pub const NFS_CACHE_FAMILIES: &[&str] = &[
    "accs", "attr", "biod", "bior", "biorl", "biow", "dire", "lkup",
];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capability {
    pub support: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub checked_at: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub reason: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub source: Option<String>,
}
impl Capability {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            ["supported", "unsupported", "unknown"].contains(&self.support.as_str()),
            "invalid capability support"
        );
        if let Some(time) = &self.checked_at {
            check_time(time)?;
        }
        check_optional_text(self.reason.as_deref(), 4096)?;
        if let Some(source) = &self.source {
            check_id(source)?;
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Resource {
    pub resource_id: String,
    pub resource_type: String,
    pub revision: Decimal,
    pub observed_at: String,
    pub identity_confidence: String,
    pub attributes: Attributes,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub capabilities: Option<BTreeMap<String, Capability>>,
}
impl Resource {
    pub fn new(id: String, kind: &str, attributes: Attributes) -> Self {
        Self {
            resource_id: id,
            resource_type: kind.into(),
            revision: 1u64.into(),
            observed_at: now(),
            identity_confidence: "host_local".into(),
            attributes,
            capabilities: None,
        }
    }
    pub fn validate(&self) -> Result<()> {
        check_id(&self.resource_id)?;
        ensure!(
            [
                "host",
                "controller",
                "physical_device",
                "media",
                "virtual_device",
                "apfs_container",
                "filesystem",
                "snapshot",
                "mount",
                "nfs_endpoint",
                "nfs_export",
                "nfs_client",
                "process",
                "network_interface",
                "storage_pool",
                "provider_resource"
            ]
            .contains(&self.resource_type.as_str()),
            "invalid resource type"
        );
        check_time(&self.observed_at)?;
        ensure!(
            ["strong", "host_local", "weak", "unknown"]
                .contains(&self.identity_confidence.as_str()),
            "invalid identity confidence"
        );
        check_attributes(&self.attributes)?;
        if let Some(capabilities) = &self.capabilities {
            ensure!(capabilities.len() <= 256, "too many capabilities");
            for capability in capabilities.values() {
                capability.validate()?;
            }
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Relationship {
    pub relationship_id: String,
    pub revision: Decimal,
    pub observed_at: String,
    pub from_resource_id: String,
    pub to_resource_id: String,
    pub relation: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub attributes: Option<Attributes>,
}
impl Relationship {
    pub fn validate(&self) -> Result<()> {
        for id in [
            &self.relationship_id,
            &self.from_resource_id,
            &self.to_resource_id,
        ] {
            check_id(id)?;
        }
        check_time(&self.observed_at)?;
        ensure!(
            [
                "attached_to",
                "contains",
                "backed_by",
                "snapshot_of",
                "mounts",
                "accesses",
                "observed_by",
                "exported_from"
            ]
            .contains(&self.relation.as_str()),
            "invalid relationship kind"
        );
        if let Some(attributes) = &self.attributes {
            check_attributes(attributes)?;
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CollectionError {
    pub domain: String,
    pub code: String,
    pub message: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub exit_code: Option<u8>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub signal: Option<i32>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub reasons: Option<Vec<String>>,
}
impl CollectionError {
    pub fn validate(&self) -> Result<()> {
        check_text(&self.domain, 64)?;
        check_text(&self.code, 64)?;
        check_text(&self.message, 4096)?;
        if let Some(signal) = self.signal {
            ensure!(signal > 0, "invalid signal");
        }
        if let Some(reasons) = &self.reasons {
            ensure!(reasons.len() <= 32, "too many error reasons");
            for reason in reasons {
                check_text(reason, 4096)?;
            }
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Collection {
    pub collection_id: String,
    pub resource_id: String,
    pub collector: String,
    pub adapter_version: String,
    pub source_version: String,
    pub started_at: String,
    pub finished_at: String,
    pub clock_id: String,
    pub started_monotonic_ns: Decimal,
    pub finished_monotonic_ns: Decimal,
    pub status: String,
    pub metrics: Vec<Metric>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub worker_state: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub error: Option<CollectionError>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub exit_code: Option<u8>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub raw_artifact_sha256: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub extensions: Option<Attributes>,
}
impl Collection {
    pub fn validate(&self) -> Result<()> {
        for id in [&self.collection_id, &self.resource_id, &self.clock_id] {
            check_id(id)?;
        }
        check_collector(&self.collector)?;
        check_text(&self.adapter_version, 64)?;
        check_text(&self.source_version, 256)?;
        ensure!(self.metrics.len() <= 4096, "metric count exceeds limit");
        ensure!(
            self.finished_monotonic_ns >= self.started_monotonic_ns,
            "reversed sample time"
        );
        chrono::DateTime::parse_from_rfc3339(&self.started_at)?;
        chrono::DateTime::parse_from_rfc3339(&self.finished_at)?;
        ensure!(
            [
                "ok",
                "partial",
                "unsupported",
                "permission_denied",
                "timeout",
                "failed",
                "parse_error",
                "skipped",
                "gone"
            ]
            .contains(&self.status.as_str()),
            "invalid collection status"
        );
        if let Some(phase) = &self.worker_state {
            ensure!(
                [
                    "exited",
                    "timed_out_pending_exit",
                    "not_started",
                    "late_result"
                ]
                .contains(&phase.as_str()),
                "invalid collection worker state"
            );
        }
        if let Some(error) = &self.error {
            error.validate()?;
        }
        check_artifact(self.raw_artifact_sha256.as_deref())?;
        check_extensions(self.extensions.as_ref(), 32)?;
        let mut keys = BTreeSet::new();
        for metric in &self.metrics {
            metric.validate()?;
            ensure!(keys.insert(metric.key()), "duplicate metric key");
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Event {
    pub event_id: String,
    pub resource_id: String,
    pub observed_at: String,
    pub source: String,
    pub category: String,
    pub severity: String,
    pub message: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub details: Option<Attributes>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub raw_artifact_sha256: Option<String>,
}
impl Event {
    pub fn validate(&self) -> Result<()> {
        for id in [&self.event_id, &self.resource_id, &self.source] {
            check_id(id)?;
        }
        check_time(&self.observed_at)?;
        ensure!(
            [
                "media",
                "integrity",
                "capacity",
                "availability",
                "performance",
                "collector",
                "lifecycle",
                "diagnostic"
            ]
            .contains(&self.category.as_str()),
            "invalid event category"
        );
        ensure!(
            ["info", "warning", "error", "critical"].contains(&self.severity.as_str()),
            "invalid event severity"
        );
        check_text(&self.message, 4096)?;
        if let Some(details) = &self.details {
            check_attributes(details)?;
        }
        check_artifact(self.raw_artifact_sha256.as_deref())
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tombstone {
    pub entity_type: String,
    pub entity_id: String,
    pub revision: Decimal,
    pub observed_at: String,
    pub reason: String,
}
impl Tombstone {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            ["resource", "relationship"].contains(&self.entity_type.as_str()),
            "invalid tombstone entity type"
        );
        check_id(&self.entity_id)?;
        check_time(&self.observed_at)?;
        check_text(&self.reason, 4096)
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentInfo {
    pub version: String,
    pub target: String,
    pub os_version: String,
    pub os_build: String,
    pub delivery_mode: String,
    pub heartbeat_interval_seconds: u64,
    pub quarantined_workers: usize,
    pub discarded_samples_total: Decimal,
    pub dropped_events_total: Decimal,
    pub payload_limited: bool,
}
impl AgentInfo {
    pub fn validate(&self) -> Result<()> {
        check_text(&self.version, 64)?;
        for value in [&self.target, &self.os_version, &self.os_build] {
            check_text(value, 96)?;
        }
        ensure!(self.delivery_mode == "latest", "invalid delivery mode");
        ensure!(
            (1..=3600).contains(&self.heartbeat_interval_seconds),
            "invalid heartbeat interval"
        );
        ensure!(
            self.quarantined_workers <= 4096,
            "too many quarantined workers"
        );
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InventoryInfo {
    pub revision: Decimal,
    pub included: bool,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerPhase {
    Idle,
    Running,
    TimedOutPendingExit,
    Disabled,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CollectorState {
    pub collector: String,
    pub resource_id: String,
    pub phase: WorkerPhase,
    pub poll_interval_seconds: u64,
    pub stale_after_seconds: u64,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub last_attempt_id: Option<String>,
}
impl CollectorState {
    pub fn validate(&self) -> Result<()> {
        check_collector(&self.collector)?;
        check_id(&self.resource_id)?;
        ensure!(
            (1..=604800).contains(&self.poll_interval_seconds)
                && (1..=604800).contains(&self.stale_after_seconds),
            "invalid collector timing policy"
        );
        if let Some(id) = &self.last_attempt_id {
            check_id(id)?;
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Heartbeat {
    pub schema_version: String,
    pub message_type: String,
    pub node_id: String,
    pub boot_id: String,
    pub agent_session_id: String,
    pub agent_generation: Decimal,
    pub sequence: Decimal,
    pub created_at: String,
    pub clock_id: String,
    pub monotonic_ns: Decimal,
    pub agent: AgentInfo,
    pub inventory: InventoryInfo,
    pub resources: Vec<Resource>,
    pub relationships: Vec<Relationship>,
    pub collections: Vec<Collection>,
    pub collector_states: Vec<CollectorState>,
    pub events: Vec<Event>,
    pub tombstones: Vec<Tombstone>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub extensions: Option<Attributes>,
}
impl Heartbeat {
    pub fn validate(&self) -> Result<()> {
        self.validate_inner(None)
    }
    /// Validate omitted inventory references against the receiver's cached graph.
    pub fn validate_with_resources(
        &self,
        known_resources: &BTreeMap<String, Resource>,
    ) -> Result<()> {
        self.validate_inner(Some(known_resources))
    }
    fn validate_inner(&self, known_resources: Option<&BTreeMap<String, Resource>>) -> Result<()> {
        ensure!(
            self.schema_version == "2.0" && self.message_type == "heartbeat",
            "unsupported heartbeat contract"
        );
        ensure!(
            self.agent_generation.get() <= u64::MAX as u128
                && self.sequence.get() <= u64::MAX as u128,
            "agent generation or sequence exceeds u64"
        );
        for id in [
            &self.node_id,
            &self.boot_id,
            &self.agent_session_id,
            &self.clock_id,
        ] {
            check_id(id)?;
        }
        chrono::DateTime::parse_from_rfc3339(&self.created_at)?;
        self.agent.validate()?;
        check_extensions(self.extensions.as_ref(), 32)?;
        ensure!(
            self.resources.len() <= 4096
                && self.relationships.len() <= 8192
                && self.collections.len() <= 4096
                && self.collector_states.len() <= 4096
                && self.events.len() <= 4096
                && self.tombstones.len() <= 8192,
            "heartbeat record limit exceeded"
        );
        ensure!(
            self.inventory.included
                || (self.resources.is_empty()
                    && self.relationships.is_empty()
                    && self.tombstones.is_empty()),
            "inventory records present while included=false"
        );
        let mut resources: BTreeMap<&str, &Resource> = known_resources
            .into_iter()
            .flat_map(|known| known.iter().map(|(id, resource)| (id.as_str(), resource)))
            .collect();
        let mut ids = BTreeSet::new();
        for resource in &self.resources {
            resource.validate()?;
            ensure!(ids.insert(&resource.resource_id), "duplicate resource IDs");
            resources.insert(resource.resource_id.as_str(), resource);
        }
        let resolve = |id: &str| -> Result<()> {
            if self.inventory.included || known_resources.is_some() {
                ensure!(resources.contains_key(id), "unresolved resource reference");
            }
            Ok(())
        };
        let mut relationship_ids = BTreeSet::new();
        for relationship in &self.relationships {
            relationship.validate()?;
            ensure!(
                relationship_ids.insert(&relationship.relationship_id),
                "duplicate relationship IDs"
            );
            resolve(&relationship.from_resource_id)?;
            resolve(&relationship.to_resource_id)?;
        }
        let mut collections = BTreeMap::new();
        for c in &self.collections {
            c.validate()?;
            resolve(&c.resource_id)?;
            if c.clock_id == self.clock_id {
                ensure!(
                    c.finished_monotonic_ns <= self.monotonic_ns,
                    "sample occurs after heartbeat on same clock"
                );
            }
            ensure!(
                collections.insert(c.collection_id.as_str(), c).is_none(),
                "duplicate collection IDs"
            );
            if let Some(r) = resources.get(c.resource_id.as_str()) {
                for m in &c.metrics {
                    ensure!(
                        catalog()[&m.name].scope.contains(&r.resource_type),
                        "metric scope mismatch"
                    );
                }
            }
        }
        let mut scopes = BTreeSet::new();
        for state in &self.collector_states {
            state.validate()?;
            resolve(&state.resource_id)?;
            ensure!(
                scopes.insert((&state.collector, &state.resource_id)),
                "duplicate collector scope"
            );
            if let Some(id) = &state.last_attempt_id {
                let c = collections
                    .get(id.as_str())
                    .context("dangling last_attempt_id")?;
                ensure!(
                    c.resource_id == state.resource_id && c.collector == state.collector,
                    "collector reference mismatch"
                );
                if state.phase == WorkerPhase::TimedOutPendingExit {
                    ensure!(
                        c.status == "timeout",
                        "quarantined worker does not reference timeout"
                    );
                }
            } else {
                ensure!(
                    state.phase != WorkerPhase::TimedOutPendingExit,
                    "quarantined worker lacks timeout attempt"
                );
            }
        }
        let mut event_ids = BTreeSet::new();
        for event in &self.events {
            event.validate()?;
            resolve(&event.resource_id)?;
            ensure!(event_ids.insert(&event.event_id), "duplicate event IDs");
        }
        let mut tombstone_ids = BTreeSet::new();
        for tombstone in &self.tombstones {
            tombstone.validate()?;
            ensure!(
                tombstone_ids.insert((&tombstone.entity_type, &tombstone.entity_id)),
                "duplicate tombstone IDs"
            );
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Acknowledgement {
    pub schema_version: String,
    pub agent_session_id: String,
    pub accepted_sequence: Decimal,
    #[serde(deserialize_with = "required_nullable_revision")]
    pub inventory_revision: Option<Decimal>,
    pub request_inventory: bool,
    pub server_received_at: String,
}
fn required_nullable_revision<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<Option<Decimal>, D::Error> {
    Option::<Decimal>::deserialize(d)
}
impl Acknowledgement {
    pub fn validate_for(&self, heartbeat: &Heartbeat) -> Result<()> {
        ensure!(
            self.schema_version == "2.0"
                && self.agent_session_id == heartbeat.agent_session_id
                && self.accepted_sequence == heartbeat.sequence,
            "acknowledgement does not match sent heartbeat"
        );
        check_id(&self.agent_session_id)?;
        ensure!(
            self.accepted_sequence.get() <= u64::MAX as u128,
            "acknowledgement sequence exceeds u64"
        );
        if !self.request_inventory {
            ensure!(
                self.inventory_revision == Some(heartbeat.inventory.revision),
                "acknowledged inventory revision does not match"
            );
        }
        chrono::DateTime::parse_from_rfc3339(&self.server_received_at)
            .context("invalid server timestamp")?;
        Ok(())
    }
}

pub fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
pub fn check_id(id: &str) -> Result<()> {
    ensure!(
        !id.is_empty() && id.chars().count() <= 512 && !id.chars().any(char::is_control),
        "invalid identifier"
    );
    Ok(())
}
pub fn attrs(value: Value) -> Attributes {
    match value {
        Value::Object(m) => m.into_iter().collect(),
        _ => BTreeMap::new(),
    }
}

fn check_text(value: &str, maximum: usize) -> Result<()> {
    ensure!(
        value.chars().count() <= maximum,
        "text exceeds schema limit"
    );
    Ok(())
}
fn check_optional_text(value: Option<&str>, maximum: usize) -> Result<()> {
    if let Some(value) = value {
        check_text(value, maximum)?;
    }
    Ok(())
}
fn check_time(value: &str) -> Result<()> {
    chrono::DateTime::parse_from_rfc3339(value).context("invalid timestamp")?;
    Ok(())
}
fn check_collector(value: &str) -> Result<()> {
    ensure!(!value.is_empty(), "empty collector ID");
    check_text(value, 96)
}
fn check_artifact(value: Option<&str>) -> Result<()> {
    if let Some(value) = value {
        ensure!(
            value.len() == 64
                && value
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "invalid artifact SHA-256"
        );
    }
    Ok(())
}
fn check_json(value: &Value) -> Result<()> {
    match value {
        Value::Number(n) => ensure!(
            n.as_f64().is_some_and(f64::is_finite),
            "nonfinite JSON number"
        ),
        Value::Array(values) => {
            for value in values {
                check_json(value)?;
            }
        }
        Value::Object(values) => {
            for value in values.values() {
                check_json(value)?;
            }
        }
        _ => {}
    }
    Ok(())
}
fn check_attributes(attributes: &Attributes) -> Result<()> {
    for value in attributes.values() {
        check_json(value)?;
    }
    Ok(())
}
fn check_extensions(attributes: Option<&Attributes>, maximum: usize) -> Result<()> {
    if let Some(attributes) = attributes {
        ensure!(attributes.len() <= maximum, "too many extension properties");
        check_attributes(attributes)?;
    }
    Ok(())
}
fn optional_non_null<'de, D, T>(d: D) -> std::result::Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d)?
        .map(Some)
        .ok_or_else(|| serde::de::Error::custom("optional field cannot be null"))
}

/// Reject duplicate object keys before any identity or counter is interpreted.
pub fn parse_json<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    struct Checked;
    impl<'de> Deserialize<'de> for Checked {
        fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
            struct Visitor;
            impl<'de> serde::de::Visitor<'de> for Visitor {
                type Value = Checked;
                fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                    f.write_str("unambiguous JSON")
                }
                fn visit_map<A: serde::de::MapAccess<'de>>(
                    self,
                    mut a: A,
                ) -> std::result::Result<Checked, A::Error> {
                    let mut keys = BTreeSet::new();
                    while let Some(key) = a.next_key::<String>()? {
                        if !keys.insert(key) {
                            return Err(serde::de::Error::custom("duplicate JSON key"));
                        }
                        a.next_value::<Checked>()?;
                    }
                    Ok(Checked)
                }
                fn visit_seq<A: serde::de::SeqAccess<'de>>(
                    self,
                    mut a: A,
                ) -> std::result::Result<Checked, A::Error> {
                    while a.next_element::<Checked>()?.is_some() {}
                    Ok(Checked)
                }
                fn visit_bool<E: serde::de::Error>(
                    self,
                    _: bool,
                ) -> std::result::Result<Checked, E> {
                    Ok(Checked)
                }
                fn visit_i64<E: serde::de::Error>(self, _: i64) -> std::result::Result<Checked, E> {
                    Ok(Checked)
                }
                fn visit_u64<E: serde::de::Error>(self, _: u64) -> std::result::Result<Checked, E> {
                    Ok(Checked)
                }
                fn visit_f64<E: serde::de::Error>(self, _: f64) -> std::result::Result<Checked, E> {
                    Ok(Checked)
                }
                fn visit_str<E: serde::de::Error>(
                    self,
                    _: &str,
                ) -> std::result::Result<Checked, E> {
                    Ok(Checked)
                }
                fn visit_unit<E: serde::de::Error>(self) -> std::result::Result<Checked, E> {
                    Ok(Checked)
                }
            }
            d.deserialize_any(Visitor)
        }
    }
    serde_json::from_slice::<Checked>(bytes).context("invalid or ambiguous JSON")?;
    serde_json::from_slice(bytes).context("invalid JSON record")
}

pub fn object() -> Value {
    Value::Object(Map::new())
}
