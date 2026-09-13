//! Deterministic filesystem and NFS rules evaluated on the central server.
use crate::{
    attention::{self, Condition},
    cider_wire::{Collection, Heartbeat, Metric, Resource},
    error::{ApiError, ApiResult},
    store,
};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{Row, Sqlite, Transaction};
use std::collections::BTreeMap;
use uuid::Uuid;

pub const POLICY_VERSION: &str = "1";
const MIB: f64 = 1024.0 * 1024.0;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct Track {
    hits: u32,
    clears: u32,
    finding_id: Option<String>,
    first_seen_at: Option<i64>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct State {
    boot_id: String,
    clock_id: String,
    monotonic_ns: u128,
    counters: BTreeMap<String, String>,
    epochs: BTreeMap<String, String>,
    features: BTreeMap<String, f64>,
    rules: BTreeMap<String, Track>,
    last_collection_id: String,
    last_observed_at: String,
}

#[derive(Clone, Copy)]
struct Rule {
    id: &'static str,
    severity: &'static str,
    summary: &'static str,
    open_intervals: u32,
    close_intervals: u32,
}

const NFS_RULES: &[Rule] = &[
    Rule {
        id: "nfs.ransomware_operation_mix.v1",
        severity: "critical",
        summary: "Ransomware-like NFS rename, read, and write activity",
        open_intervals: 3,
        close_intervals: 3,
    },
    Rule {
        id: "nfs.mass_deletion.v1",
        severity: "critical",
        summary: "High-rate NFS deletion activity",
        open_intervals: 3,
        close_intervals: 3,
    },
    Rule {
        id: "nfs.bulk_read_exfiltration.v1",
        severity: "warning",
        summary: "Sustained NFS bulk reads with little write activity",
        open_intervals: 6,
        close_intervals: 3,
    },
    Rule {
        id: "nfs.reconnaissance.v1",
        severity: "warning",
        summary: "NFS metadata traversal with little file I/O",
        open_intervals: 3,
        close_intervals: 3,
    },
    Rule {
        id: "nfs.storage_abuse.v1",
        severity: "warning",
        summary: "Sustained high-rate NFS writes with little read activity",
        open_intervals: 6,
        close_intervals: 3,
    },
    Rule {
        id: "nfs.rpc_instability.v1",
        severity: "warning",
        summary: "NFS RPC timeout or retry rate is elevated",
        open_intervals: 3,
        close_intervals: 3,
    },
];
const IO_RULES: &[Rule] = &[
    Rule {
        id: "macos.filesystem.bulk_write.v1",
        severity: "warning",
        summary: "Sustained high-rate local storage writes",
        open_intervals: 3,
        close_intervals: 3,
    },
    Rule {
        id: "macos.filesystem.bulk_read.v1",
        severity: "warning",
        summary: "Sustained high-rate local storage reads",
        open_intervals: 3,
        close_intervals: 3,
    },
    Rule {
        id: "macos.filesystem.bidirectional_churn.v1",
        severity: "critical",
        summary: "Sustained high-rate local read and write churn",
        open_intervals: 3,
        close_intervals: 3,
    },
];

fn internal(message: &str) -> ApiError {
    ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", message)
}
fn parse_time(value: &str) -> ApiResult<i64> {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|v| v.timestamp_millis())
        .map_err(|_| internal("Invalid security-rule timestamp"))
}
fn object_id(hb: &Heartbeat, resource: &Resource) -> ApiResult<String> {
    let namespace = Uuid::parse_str(&hb.node_id).map_err(|_| internal("Invalid node identity"))?;
    Ok(Uuid::new_v5(
        &namespace,
        format!("ciderd:{}", resource.resource_id).as_bytes(),
    )
    .to_string())
}
fn source_id(hb: &Heartbeat, resource: &Resource, collector: &str) -> ApiResult<String> {
    let namespace = Uuid::parse_str(&hb.node_id).map_err(|_| internal("Invalid node identity"))?;
    Ok(Uuid::new_v5(
        &namespace,
        format!("security-rules:{}:{collector}", resource.resource_id).as_bytes(),
    )
    .to_string())
}
fn counter(metric: &Metric) -> Option<u128> {
    if metric.kind != "counter"
        || metric.availability != "available"
        || metric.freshness.as_deref() != Some("live")
    {
        return None;
    }
    metric.value.as_ref()?.as_str()?.parse().ok()
}
fn number(metric: &Metric) -> Option<f64> {
    if metric.availability != "available" || metric.freshness.as_deref() != Some("live") {
        return None;
    }
    let value = metric.value.as_ref()?;
    value.as_f64().or_else(|| value.as_str()?.parse().ok())
}
fn bool_value(metric: &Metric) -> Option<bool> {
    if metric.availability != "available" || metric.freshness.as_deref() != Some("live") {
        return None;
    }
    metric.value.as_ref()?.as_bool()
}
fn metric_key(metric: &Metric) -> String {
    let attrs = serde_json::to_string(&metric.attributes).unwrap_or_default();
    format!("{}:{attrs}", metric.name)
}
fn op_key(metric: &Metric) -> Option<String> {
    (metric.name == "storage.nfs.client.operations_total")
        .then(|| {
            metric
                .attributes
                .get("operation")?
                .as_str()
                .map(str::to_owned)
        })
        .flatten()
}
fn rate(state: &State, metric: &Metric, seconds: f64) -> Option<f64> {
    let key = metric_key(metric);
    let old: u128 = state.counters.get(&key)?.parse().ok()?;
    let new = counter(metric)?;
    let epoch = metric.counter_epoch.as_deref()?;
    if state.epochs.get(&key).map(String::as_str) == Some(epoch) && new >= old {
        Some((new - old) as f64 / seconds)
    } else {
        None
    }
}
fn sum(features: &BTreeMap<String, f64>, names: &[&str]) -> f64 {
    names
        .iter()
        .map(|name| features.get(*name).copied().unwrap_or(0.0))
        .sum()
}
fn qualifies(rule: &str, f: &BTreeMap<String, f64>) -> bool {
    let read = f.get("read_ops_per_second").copied().unwrap_or(0.0);
    let write = f.get("write_ops_per_second").copied().unwrap_or(0.0);
    let rename = f.get("rename_ops_per_second").copied().unwrap_or(0.0);
    let deletion = sum(f, &["remove_ops_per_second", "rmdir_ops_per_second"]);
    let traversal = sum(
        f,
        &[
            "lookup_ops_per_second",
            "readdir_ops_per_second",
            "rdirplus_ops_per_second",
            "getattr_ops_per_second",
        ],
    );
    match rule {
        "nfs.ransomware_operation_mix.v1" => rename >= 10.0 && read >= 20.0 && write >= 20.0,
        "nfs.mass_deletion.v1" => deletion >= 50.0,
        "nfs.bulk_read_exfiltration.v1" => read >= 200.0 && write <= 10.0 && traversal >= 25.0,
        "nfs.reconnaissance.v1" => traversal >= 100.0 && read + write <= 10.0,
        "nfs.storage_abuse.v1" => write >= 200.0 && read <= 20.0,
        "nfs.rpc_instability.v1" => {
            sum(f, &["rpc_timeouts_per_second", "rpc_retries_per_second"]) >= 5.0
        }
        "macos.filesystem.bulk_write.v1" => {
            f.get("write_bytes_per_second").copied().unwrap_or(0.0) >= 256.0 * MIB
        }
        "macos.filesystem.bulk_read.v1" => {
            f.get("read_bytes_per_second").copied().unwrap_or(0.0) >= 512.0 * MIB
        }
        "macos.filesystem.bidirectional_churn.v1" => {
            f.get("read_bytes_per_second").copied().unwrap_or(0.0) >= 128.0 * MIB
                && f.get("write_bytes_per_second").copied().unwrap_or(0.0) >= 128.0 * MIB
        }
        _ => false,
    }
}

fn feature_snapshot(state: &State, collection: &Collection) -> Option<BTreeMap<String, f64>> {
    if state.monotonic_ns == 0 || state.boot_id.is_empty() || state.clock_id != collection.clock_id
    {
        return None;
    }
    let elapsed = collection
        .finished_monotonic_ns
        .get()
        .checked_sub(state.monotonic_ns)? as f64
        / 1_000_000_000.0;
    if !(0.1..=300.0).contains(&elapsed) {
        return None;
    }
    let mut features = BTreeMap::new();
    for metric in &collection.metrics {
        let Some(value) = rate(state, metric, elapsed) else {
            continue;
        };
        if let Some(operation) = op_key(metric) {
            let key = format!(
                "{}_ops_per_second",
                if operation == "READ" {
                    "read"
                } else {
                    operation.as_str()
                }
            );
            *features.entry(key).or_default() += value;
        } else {
            let key = match metric.name.as_str() {
                "storage.nfs.client.rpc_timeouts_total" => Some("rpc_timeouts_per_second"),
                "storage.nfs.client.rpc_retries_total" => Some("rpc_retries_per_second"),
                "storage.device.read_bytes_total" => Some("read_bytes_per_second"),
                "storage.device.write_bytes_total" => Some("write_bytes_per_second"),
                _ => None,
            };
            if let Some(key) = key {
                *features.entry(key.into()).or_default() += value;
            }
        }
    }
    Some(features)
}

fn update_counters(state: &mut State, hb: &Heartbeat, collection: &Collection) {
    state.boot_id = hb.boot_id.clone();
    state.clock_id = collection.clock_id.clone();
    state.monotonic_ns = collection.finished_monotonic_ns.get();
    state.last_collection_id = collection.collection_id.clone();
    state.last_observed_at = collection.finished_at.clone();
    for metric in &collection.metrics {
        if let Some(value) = counter(metric) {
            let key = metric_key(metric);
            state.counters.insert(key.clone(), value.to_string());
            state
                .epochs
                .insert(key, metric.counter_epoch.clone().unwrap_or_default());
        }
    }
}

async fn persist_finding(
    tx: &mut Transaction<'_, Sqlite>,
    hb: &Heartbeat,
    resource: &Resource,
    source: &str,
    scope: &str,
    collection: &Collection,
    rule: Rule,
    track: &mut Track,
    features: &BTreeMap<String, f64>,
    active: bool,
    now: i64,
) -> ApiResult<()> {
    let status = if active { "open" } else { "resolved" };
    let first = track.first_seen_at.unwrap_or(now);
    let finding_id = track
        .finding_id
        .get_or_insert_with(|| Uuid::new_v4().to_string())
        .clone();
    let ended = (!active).then_some(now);
    let evidence = json!({
        "collection_id": collection.collection_id, "observed_at": collection.finished_at,
        "features": features, "required_intervals": rule.open_intervals,
        "recovery_intervals": rule.close_intervals, "policy_version": POLICY_VERSION
    });
    let finding = json!({
        "finding_id":finding_id,"source_id":source,"node_id":hb.node_id,
        "object_id":object_id(hb,resource)?,"resource_id":resource.resource_id,"scope":scope,
        "rule_id":rule.id,"policy_version":POLICY_VERSION,"status":status,"severity":rule.severity,
        "summary":rule.summary,"first_seen_at":store::timestamp(first),"last_seen_at":collection.finished_at,
        "updated_at":store::timestamp(now),"ended_at":ended.map(store::timestamp),
        "reason":if active { Value::Null } else { json!("threshold_cleared") },"evidence":evidence
    });
    sqlx::query("INSERT INTO security_rule_findings(finding_id,source_id,node_id,object_id,rule_id,status,severity,first_seen_at,updated_at,ended_at,finding_json) VALUES(?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(finding_id) DO UPDATE SET status=excluded.status,updated_at=excluded.updated_at,ended_at=excluded.ended_at,finding_json=excluded.finding_json")
        .bind(&finding_id).bind(source).bind(&hb.node_id).bind(object_id(hb,resource)?).bind(rule.id).bind(status).bind(rule.severity)
        .bind(first).bind(now).bind(ended).bind(finding.to_string()).execute(&mut **tx).await?;
    attention::observe_condition(
        tx,
        &Condition {
            key: format!("security-rule:{source}:{}", rule.id),
            kind: "filesystem".into(),
            node_id: hb.node_id.clone(),
            object_id: Some(object_id(hb, resource)?),
            status: status.into(),
            severity: rule.severity.into(),
            observation_state: "current".into(),
            summary: rule.summary.into(),
            evidence: finding,
        },
        now,
    )
    .await?;
    if !active {
        track.finding_id = None;
        track.first_seen_at = None;
    }
    Ok(())
}

async fn evaluate_rules(
    tx: &mut Transaction<'_, Sqlite>,
    hb: &Heartbeat,
    resource: &Resource,
    collection: &Collection,
    source: &str,
    scope: &str,
    state: &mut State,
    rules: &[Rule],
    features: BTreeMap<String, f64>,
    now: i64,
) -> ApiResult<()> {
    for rule in rules {
        let active = qualifies(rule.id, &features);
        let track = state.rules.entry(rule.id.into()).or_default();
        if active {
            track.hits = track.hits.saturating_add(1);
            track.clears = 0;
        } else {
            track.hits = 0;
            track.clears = track.clears.saturating_add(1);
        }
        if track.finding_id.is_none() && track.hits >= rule.open_intervals {
            track.first_seen_at = Some(parse_time(&collection.finished_at)?);
            persist_finding(
                tx, hb, resource, source, scope, collection, *rule, track, &features, true, now,
            )
            .await?;
        } else if track.finding_id.is_some() && active {
            persist_finding(
                tx, hb, resource, source, scope, collection, *rule, track, &features, true, now,
            )
            .await?;
        } else if track.finding_id.is_some() && track.clears >= rule.close_intervals {
            persist_finding(
                tx, hb, resource, source, scope, collection, *rule, track, &features, false, now,
            )
            .await?;
        }
    }
    state.features = features;
    Ok(())
}

fn relevant(
    resource: &Resource,
    collection: &Collection,
) -> Option<(&'static str, &'static [Rule])> {
    if collection.collector == "nfsstat.client" && resource.resource_type == "nfs_client" {
        Some(("host_nfs_client", NFS_RULES))
    } else if collection.collector == "iokit.block"
        && resource.resource_type == "controller"
        && resource.attributes.get("scope").and_then(Value::as_str) == Some("driver")
    {
        Some(("macos_storage_driver", IO_RULES))
    } else {
        None
    }
}

pub async fn observe_collection(
    tx: &mut Transaction<'_, Sqlite>,
    hb: &Heartbeat,
    resource: &Resource,
    collection: &Collection,
    now: i64,
) -> ApiResult<()> {
    if matches!(
        collection.collector.as_str(),
        "nfs.mount_status" | "nfs.nstatus"
    ) && resource.resource_type == "nfs_mount"
    {
        return observe_mount(tx, hb, resource, collection, now).await;
    }
    if collection.collector == "apfs.snapshots" {
        return observe_snapshot_count(tx, hb, resource, collection, now).await;
    }
    let Some((scope, rules)) = relevant(resource, collection) else {
        return Ok(());
    };
    let source = source_id(hb, resource, &collection.collector)?;
    let row = sqlx::query("SELECT state_json FROM security_rule_sources WHERE source_id=?")
        .bind(&source)
        .fetch_optional(&mut **tx)
        .await?;
    let mut state: State = row
        .map(|r| serde_json::from_str(&r.get::<String, _>("state_json")))
        .transpose()
        .map_err(|_| internal("Cannot decode security-rule state"))?
        .unwrap_or_default();
    if collection.status == "ok" || collection.status == "partial" {
        let features = feature_snapshot(&state, collection);
        update_counters(&mut state, hb, collection);
        if let Some(features) = features {
            state.features = features.clone();
            save_source(tx, hb, resource, collection, &source, scope, &state, now).await?;
            evaluate_rules(
                tx, hb, resource, collection, &source, scope, &mut state, rules, features, now,
            )
            .await?;
        }
    }
    save_source(tx, hb, resource, collection, &source, scope, &state, now).await
}

async fn save_source(
    tx: &mut Transaction<'_, Sqlite>,
    hb: &Heartbeat,
    resource: &Resource,
    collection: &Collection,
    source: &str,
    scope: &str,
    state: &State,
    now: i64,
) -> ApiResult<()> {
    let rules:Vec<Value>=state.rules.iter().map(|(id,t)|json!({"rule_id":id,"state":if t.finding_id.is_some(){"open"}else{"quiet"},"qualifying_intervals":t.hits,"recovery_intervals":t.clears,"finding_id":t.finding_id})).collect();
    let summary = json!({"source_id":source,"node_id":hb.node_id,"object_id":object_id(hb,resource)?,"resource_id":resource.resource_id,
        "collector":collection.collector,"scope":scope,"active":true,"policy_version":POLICY_VERSION,"features":state.features,"rules":rules,
        "observation":{"state":if collection.status=="ok"||collection.status=="partial"{"current"}else{"unavailable"},"status":collection.status,
            "collection_id":collection.collection_id,"observed_at":collection.finished_at,"received_at":store::timestamp(now)}});
    let encoded =
        serde_json::to_string(state).map_err(|_| internal("Cannot encode security-rule state"))?;
    sqlx::query("INSERT INTO security_rule_sources(source_id,node_id,object_id,resource_id,collector,scope,active,state_json,summary_json,updated_at) VALUES(?,?,?,?,?,?,1,?,?,?) ON CONFLICT(source_id) DO UPDATE SET active=1,state_json=excluded.state_json,summary_json=excluded.summary_json,updated_at=excluded.updated_at")
        .bind(source).bind(&hb.node_id).bind(object_id(hb,resource)?).bind(&resource.resource_id).bind(&collection.collector).bind(scope).bind(encoded).bind(summary.to_string()).bind(now).execute(&mut **tx).await?;
    Ok(())
}

async fn observe_mount(
    tx: &mut Transaction<'_, Sqlite>,
    hb: &Heartbeat,
    resource: &Resource,
    collection: &Collection,
    now: i64,
) -> ApiResult<()> {
    let source = source_id(hb, resource, "nfs.mount")?;
    let row = sqlx::query("SELECT state_json FROM security_rule_sources WHERE source_id=?")
        .bind(&source)
        .fetch_optional(&mut **tx)
        .await?;
    let mut state: State = row
        .map(|r| serde_json::from_str(&r.get::<String, _>("state_json")))
        .transpose()
        .map_err(|_| internal("Cannot decode mount rule state"))?
        .unwrap_or_default();
    if let Some(value) = collection
        .metrics
        .iter()
        .find(|m| m.name == "storage.nfs.mount.dead")
        .and_then(bool_value)
    {
        state.features.insert("dead".into(), f64::from(value));
    }
    if let Some(value) = collection
        .metrics
        .iter()
        .find(|m| m.name == "storage.nfs.mount.not_responding")
        .and_then(bool_value)
    {
        state
            .features
            .insert("not_responding".into(), f64::from(value));
    }
    if let Some(value) = collection
        .metrics
        .iter()
        .find(|m| m.name == "storage.nfs.mount.oldest_request_age_seconds")
        .and_then(number)
    {
        state
            .features
            .insert("oldest_request_age_seconds".into(), value);
    }
    let features = state.features.clone();
    let dead = features.get("dead").copied().unwrap_or(0.0) > 0.0;
    let unresponsive = features.get("not_responding").copied().unwrap_or(0.0) > 0.0;
    let oldest = features
        .get("oldest_request_age_seconds")
        .copied()
        .unwrap_or(0.0);
    let rule = Rule {
        id: "nfs.mount_unhealthy.v1",
        severity: if dead { "critical" } else { "warning" },
        summary: "NFS mount is unresponsive or has stalled requests",
        open_intervals: 1,
        close_intervals: 2,
    };
    let active = dead || unresponsive || oldest >= 30.0;
    save_source(
        tx,
        hb,
        resource,
        collection,
        &source,
        "nfs_mount",
        &state,
        now,
    )
    .await?;
    let track = state.rules.entry(rule.id.into()).or_default();
    if active {
        track.hits = track.hits.saturating_add(1);
        track.clears = 0;
    } else {
        track.hits = 0;
        track.clears = track.clears.saturating_add(1);
    }
    if active {
        if track.first_seen_at.is_none() {
            track.first_seen_at = Some(parse_time(&collection.finished_at)?);
        }
        persist_finding(
            tx,
            hb,
            resource,
            &source,
            "nfs_mount",
            collection,
            rule,
            track,
            &features,
            true,
            now,
        )
        .await?;
    } else if track.finding_id.is_some() && track.clears >= rule.close_intervals {
        persist_finding(
            tx,
            hb,
            resource,
            &source,
            "nfs_mount",
            collection,
            rule,
            track,
            &features,
            false,
            now,
        )
        .await?;
    }
    save_source(
        tx,
        hb,
        resource,
        collection,
        &source,
        "nfs_mount",
        &state,
        now,
    )
    .await
}

async fn observe_snapshot_count(
    tx: &mut Transaction<'_, Sqlite>,
    hb: &Heartbeat,
    resource: &Resource,
    collection: &Collection,
    now: i64,
) -> ApiResult<()> {
    let Some(count) = collection
        .metrics
        .iter()
        .find(|m| m.name == "storage.apfs.snapshot_count")
        .and_then(number)
    else {
        return Ok(());
    };
    let source = source_id(hb, resource, &collection.collector)?;
    let row = sqlx::query("SELECT state_json FROM security_rule_sources WHERE source_id=?")
        .bind(&source)
        .fetch_optional(&mut **tx)
        .await?;
    let mut state: State = row
        .map(|r| serde_json::from_str(&r.get::<String, _>("state_json")))
        .transpose()
        .map_err(|_| internal("Cannot decode snapshot rule state"))?
        .unwrap_or_default();
    let previous = state.features.get("snapshot_count").copied();
    state.features = BTreeMap::from([
        ("snapshot_count".into(), count),
        ("previous_snapshot_count".into(), previous.unwrap_or(count)),
        (
            "snapshots_removed".into(),
            previous.map(|p| (p - count).max(0.0)).unwrap_or(0.0),
        ),
    ]);
    let rule = Rule {
        id: "macos.apfs.snapshot_loss.v1",
        severity: "critical",
        summary: "APFS snapshot count decreased",
        open_intervals: 1,
        close_intervals: 1,
    };
    let active = previous.is_some_and(|p| count < p);
    save_source(
        tx,
        hb,
        resource,
        collection,
        &source,
        "apfs_volume",
        &state,
        now,
    )
    .await?;
    let track = state.rules.entry(rule.id.into()).or_default();
    if active {
        track.hits = 1;
        track.first_seen_at = Some(parse_time(&collection.finished_at)?);
        persist_finding(
            tx,
            hb,
            resource,
            &source,
            "apfs_volume",
            collection,
            rule,
            track,
            &state.features,
            true,
            now,
        )
        .await?;
    }
    save_source(
        tx,
        hb,
        resource,
        collection,
        &source,
        "apfs_volume",
        &state,
        now,
    )
    .await
}

pub async fn source_summaries(
    tx: &mut Transaction<'_, Sqlite>,
    node: Option<&str>,
    object: Option<&str>,
) -> ApiResult<Vec<Value>> {
    let rows=sqlx::query("SELECT summary_json FROM security_rule_sources WHERE (? IS NULL OR node_id=?) AND (? IS NULL OR object_id=?) ORDER BY updated_at DESC,source_id LIMIT 10001")
        .bind(node).bind(node).bind(object).bind(object).fetch_all(&mut **tx).await?;
    if rows.len() > 10000 {
        return Err(ApiError::unavailable());
    }
    rows.into_iter()
        .map(|r| {
            serde_json::from_str(&r.get::<String, _>("summary_json"))
                .map_err(|_| internal("Cannot decode security-rule summary"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nfs_rules_require_the_documented_operation_shapes() {
        let ransomware = BTreeMap::from([
            ("rename_ops_per_second".into(), 10.0),
            ("read_ops_per_second".into(), 20.0),
            ("write_ops_per_second".into(), 20.0),
        ]);
        assert!(qualifies("nfs.ransomware_operation_mix.v1", &ransomware));
        assert!(!qualifies("nfs.mass_deletion.v1", &ransomware));

        let recon = BTreeMap::from([
            ("lookup_ops_per_second".into(), 60.0),
            ("getattr_ops_per_second".into(), 40.0),
            ("read_ops_per_second".into(), 5.0),
        ]);
        assert!(qualifies("nfs.reconnaissance.v1", &recon));
        assert!(!qualifies("nfs.bulk_read_exfiltration.v1", &recon));
    }

    #[test]
    fn local_io_rules_use_fixed_binary_byte_thresholds() {
        let features = BTreeMap::from([
            ("read_bytes_per_second".into(), 128.0 * MIB),
            ("write_bytes_per_second".into(), 256.0 * MIB),
        ]);
        assert!(qualifies("macos.filesystem.bulk_write.v1", &features));
        assert!(qualifies(
            "macos.filesystem.bidirectional_churn.v1",
            &features
        ));
        assert!(!qualifies("macos.filesystem.bulk_read.v1", &features));
    }
}
