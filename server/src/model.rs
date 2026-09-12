use crate::error::{ApiError, ApiResult};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Agent {
    pub version: String,
    #[serde(default)]
    pub os_build: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct EnrollmentRequest {
    pub enrollment_token: String,
    pub name: String,
    pub agent: Agent,
}

#[derive(Debug, Deserialize)]
pub struct TokenRequest {
    #[serde(default = "default_ttl")]
    pub expires_in_seconds: u64,
}
fn default_ttl() -> u64 {
    600
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Inventory {
    pub generation: u64,
    pub objects: Vec<InventoryObject>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct InventoryObject {
    pub local_id: String,
    pub kind: String,
    #[serde(default)]
    pub parents: Vec<String>,
    #[serde(default)]
    pub properties: BTreeMap<String, Value>,
}

#[derive(Debug, Deserialize)]
pub struct Heartbeat {
    pub boot_id: String,
    pub agent: Agent,
}

#[derive(Debug, Deserialize)]
pub struct Goodbye {
    pub boot_id: String,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Telemetry {
    pub schema_version: String,
    pub node_id: String,
    pub boot_id: String,
    pub sequence: u64,
    pub observed_at: DateTime<Utc>,
    pub sent_at: DateTime<Utc>,
    pub agent: Agent,
    pub inventory_generation: u64,
    pub samples: Vec<Sample>,
    #[serde(default)]
    pub events: Vec<CollectorEvent>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Sample {
    pub object_id: String,
    pub name: String,
    pub kind: String,
    #[serde(default)]
    pub value: Value,
    pub unit: String,
    pub state: String,
    pub source: String,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    pub observed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CollectorEvent {
    #[serde(default)]
    pub object_id: Option<String>,
    pub category: String,
    pub severity: String,
    pub occurred_at: DateTime<Utc>,
    pub source: String,
    pub summary: String,
    #[serde(default)]
    pub details: BTreeMap<String, Value>,
}

pub const STATES: &[&str] = &[
    "ok",
    "unsupported",
    "permission_denied",
    "stale",
    "timeout",
    "parse_error",
    "failed",
];
pub const OBJECT_KINDS: &[&str] = &[
    "device",
    "partition",
    "apfs_store",
    "apfs_container",
    "apfs_volume",
    "snapshot",
    "mount",
    "nfs_mount",
    "provider",
];

/// Metric names, kinds, and units are an explicit versioned ingest allowlist.
pub fn metric_definition(name: &str) -> Option<(&'static str, &'static str)> {
    Some(match name {
        "node_up" | "smart_healthy" => ("boolean", "boolean"),
        "node_boot_id" | "node_agent_version" | "node_os_version" | "node_os_build" => {
            ("string", "string")
        }
        "node_thermal_pressure" | "node_memory_pressure" | "nfs_mount_status" | "smart_status" => {
            ("enum", "enum")
        }
        "node_uptime_seconds"
        | "node_clock_offset_seconds"
        | "nfs_oldest_request_wait_seconds"
        | "snapshot_oldest_age_seconds"
        | "snapshot_newest_age_seconds" => ("gauge", "seconds"),
        "node_swap_in_bytes_total"
        | "node_swap_out_bytes_total"
        | "device_read_bytes_total"
        | "device_write_bytes_total" => ("counter", "bytes"),
        "device_read_ops_total" | "device_write_ops_total" | "nfs_operations_total" => {
            ("counter", "operations")
        }
        "device_read_errors_total"
        | "device_write_errors_total"
        | "nvme_media_errors_total"
        | "nfs_rpc_timeouts_total"
        | "nfs_protocol_errors_total" => ("counter", "errors"),
        "device_retries_total" | "nfs_retransmissions_total" => ("counter", "retries"),
        "device_read_time_ns_total" | "device_write_time_ns_total" => ("counter", "nanoseconds"),
        "capacity_bytes"
        | "used_bytes"
        | "free_bytes"
        | "available_bytes"
        | "apfs_reserve_bytes"
        | "apfs_quota_bytes"
        | "apfs_purgeable_bytes"
        | "quota_limit_bytes"
        | "quota_used_bytes"
        | "quota_available_bytes"
        | "quota_soft_limit_bytes"
        | "quota_hard_limit_bytes" => ("gauge", "bytes"),
        "files_total" | "files_free" | "files_used" | "quota_files_used" | "quota_files_limit" => {
            ("gauge", "files")
        }
        "snapshot_count" | "nfs_outstanding_request_entries" | "nvme_critical_warning" => {
            ("gauge", "count")
        }
        "device_temperature_celsius" | "device_warning_temperature_celsius" => ("gauge", "celsius"),
        "nvme_available_spare_percent"
        | "nvme_spare_threshold_percent"
        | "nvme_percentage_used" => ("gauge", "percent"),
        "quota_grace_expires_at" => ("string", "timestamp"),
        _ => return None,
    })
}

pub fn bounded(field: &str, value: &str, max: usize) -> ApiResult<()> {
    if value.trim().is_empty() || value.len() > max || value.chars().any(char::is_control) {
        return Err(ApiError::field(
            field,
            format!("Expected 1-{max} UTF-8 bytes without control characters"),
        ));
    }
    Ok(())
}

pub fn validate_agent(agent: &Agent) -> ApiResult<()> {
    bounded("agent.version", &agent.version, 64)?;
    if let Some(build) = &agent.os_build {
        bounded("agent.os_build", build, 64)?;
    }
    Ok(())
}

/// Deny file-level content and credential fields even inside additive metadata.
pub fn validate_privacy(value: &Value, path: &str) -> ApiResult<()> {
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                let normalized = key.to_ascii_lowercase().replace('-', "_");
                if [
                    "file_contents",
                    "file_content",
                    "file_path",
                    "filename",
                    "filenames",
                    "command_line",
                    "commandline",
                    "argv",
                    "username",
                    "user_name",
                    "password",
                    "authorization",
                    "access_token",
                ]
                .contains(&normalized.as_str())
                {
                    return Err(ApiError::field(
                        format!("{path}.{key}"),
                        "File-level data and secrets are not accepted",
                    ));
                }
                validate_privacy(value, &format!("{path}.{key}"))?;
            }
        }
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                validate_privacy(item, &format!("{path}[{i}]"))?;
            }
        }
        _ => {}
    }
    Ok(())
}

pub fn validate_inventory(inventory: &Inventory) -> ApiResult<()> {
    if inventory.generation == 0 || inventory.generation > i64::MAX as u64 {
        return Err(ApiError::field(
            "generation",
            "Must be a positive signed 64-bit integer",
        ));
    }
    if inventory.objects.len() > crate::MAX_OBJECTS {
        return Err(ApiError::field(
            "objects",
            "At most 2048 objects are allowed",
        ));
    }
    let mut ids = HashSet::new();
    for object in &inventory.objects {
        bounded("objects.local_id", &object.local_id, 128)?;
        if object.local_id == "node" || !ids.insert(object.local_id.clone()) {
            return Err(ApiError::field(
                "objects.local_id",
                "IDs must be unique; node is reserved for the implicit root",
            ));
        }
        if !OBJECT_KINDS.contains(&object.kind.as_str()) {
            return Err(ApiError::field(
                "objects.kind",
                "Unsupported storage object kind",
            ));
        }
    }
    let mut degrees = HashMap::new();
    let mut children: HashMap<&str, Vec<&str>> = HashMap::new();
    for object in &inventory.objects {
        let mut unique = HashSet::new();
        let mut degree = 0;
        for parent in &object.parents {
            if !unique.insert(parent) || (parent != "node" && !ids.contains(parent)) {
                return Err(ApiError::field(
                    "objects.parents",
                    "Parents must be unique and refer to this inventory or node",
                ));
            }
            if parent != "node" {
                degree += 1;
                children.entry(parent).or_default().push(&object.local_id);
            }
        }
        degrees.insert(object.local_id.as_str(), degree);
    }
    let mut queue: VecDeque<&str> = degrees
        .iter()
        .filter(|(_, degree)| **degree == 0)
        .map(|(id, _)| *id)
        .collect();
    let mut visited = 0;
    while let Some(id) = queue.pop_front() {
        visited += 1;
        if let Some(children) = children.get(id) {
            for child in children {
                let degree = degrees.get_mut(child).expect("validated inventory ID");
                *degree -= 1;
                if *degree == 0 {
                    queue.push_back(child);
                }
            }
        }
    }
    if visited != inventory.objects.len() {
        return Err(ApiError::field(
            "objects.parents",
            "Inventory graph contains a cycle",
        ));
    }
    Ok(())
}

pub fn counter(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| value.as_str()?.parse().ok())
}

pub fn validate_time(field: &str, time: DateTime<Utc>, now: DateTime<Utc>) -> ApiResult<()> {
    let age = now.signed_duration_since(time).num_milliseconds();
    if !(-300_000..=86_400_000).contains(&age) {
        return Err(ApiError::field(
            field,
            "Observation must be within the last 24 hours and at most 300 seconds in the future",
        ));
    }
    Ok(())
}

pub fn validate_telemetry(batch: &Telemetry) -> ApiResult<()> {
    if batch.schema_version != "1.0" {
        return Err(ApiError::field(
            "schema_version",
            "Supported version is 1.0",
        ));
    }
    bounded("boot_id", &batch.boot_id, 128)?;
    validate_agent(&batch.agent)?;
    if batch.inventory_generation > i64::MAX as u64 {
        return Err(ApiError::field(
            "inventory_generation",
            "Integer is too large",
        ));
    }
    if batch.samples.len() > crate::MAX_SAMPLES || batch.events.len() > crate::MAX_EVENTS {
        return Err(ApiError::field(
            "samples/events",
            "At most 2048 samples and 128 events are allowed",
        ));
    }
    let now = Utc::now();
    validate_time("observed_at", batch.observed_at, now)?;
    validate_time("sent_at", batch.sent_at, now)?;
    let mut unique = HashSet::new();
    for (i, sample) in batch.samples.iter().enumerate() {
        let field = format!("samples[{i}]");
        let Some((kind, unit)) = metric_definition(&sample.name) else {
            return Err(ApiError::field(
                format!("{field}.name"),
                "Metric is not in the version 1.0 catalog",
            ));
        };
        if sample.kind != kind || sample.unit != unit {
            return Err(ApiError::field(
                &field,
                format!("Expected kind={kind}, unit={unit}"),
            ));
        }
        if !STATES.contains(&sample.state.as_str()) {
            return Err(ApiError::field(
                format!("{field}.state"),
                "Unknown observation state",
            ));
        }
        bounded(&format!("{field}.source"), &sample.source, 128)?;
        validate_time(&format!("{field}.observed_at"), sample.observed_at, now)?;
        if let Some(scope) = &sample.scope {
            if ![
                "node",
                "device",
                "container",
                "volume",
                "mount",
                "filesystem",
                "user",
                "group",
                "object",
            ]
            .contains(&scope.as_str())
            {
                return Err(ApiError::field(
                    format!("{field}.scope"),
                    "Unknown observation scope",
                ));
            }
        }
        if sample.labels.len() > 16 {
            return Err(ApiError::field(
                format!("{field}.labels"),
                "At most 16 labels",
            ));
        }
        for (key, value) in &sample.labels {
            bounded("label.key", key, 64)?;
            bounded("label.value", value, 128)?;
        }
        if !sample.value.is_null() {
            let valid = match kind {
                "counter" => counter(&sample.value).is_some(),
                "gauge" => sample.value.as_f64().is_some_and(f64::is_finite),
                "boolean" => sample.value.is_boolean(),
                "string" | "enum" => sample
                    .value
                    .as_str()
                    .is_some_and(|v| v.len() <= 512 && !v.chars().any(char::is_control)),
                _ => false,
            };
            if !valid {
                return Err(ApiError::field(
                    format!("{field}.value"),
                    "Value does not match metric kind",
                ));
            }
            if kind == "gauge"
                && sample.name != "node_clock_offset_seconds"
                && sample.value.as_f64().is_some_and(|v| v < 0.0)
            {
                return Err(ApiError::field(
                    format!("{field}.value"),
                    "This metric cannot be negative",
                ));
            }
        } else if sample.state == "ok" {
            return Err(ApiError::field(
                format!("{field}.value"),
                "An ok observation requires a value",
            ));
        }
        let identity = (
            &sample.object_id,
            &sample.name,
            &sample.source,
            &sample.scope,
            serde_json::to_string(&sample.labels).unwrap(),
            sample.observed_at,
        );
        if !unique.insert(identity) {
            return Err(ApiError::field(
                &field,
                "Duplicate series and timestamp within batch",
            ));
        }
    }
    for event in &batch.events {
        if ![
            "inventory",
            "availability",
            "storage",
            "security",
            "collector",
            "alert",
        ]
        .contains(&event.category.as_str())
            || !["info", "warning", "critical"].contains(&event.severity.as_str())
        {
            return Err(ApiError::field("events", "Invalid category or severity"));
        }
        bounded("events.source", &event.source, 128)?;
        bounded("events.summary", &event.summary, 1024)?;
        validate_time("events.occurred_at", event.occurred_at, now)?;
    }
    Ok(())
}
