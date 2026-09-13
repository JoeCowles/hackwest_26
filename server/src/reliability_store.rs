//! Reliability persistence: every mutation shares the native receipt transaction.
use crate::{
    cider_wire::{Collection, Heartbeat, Resource},
    error::{ApiError, ApiResult},
    reliability::{Finding, Observation, Policy, SourceIdentity, SourceState},
};
use axum::http::StatusCode;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sqlx::{Row, Sqlite, Transaction};
use std::collections::BTreeMap;
use uuid::Uuid;

fn internal(message: &str) -> ApiError {
    ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", message)
}
fn encode(value: &impl Serialize) -> ApiResult<String> {
    serde_json::to_string(value).map_err(|_| internal("Cannot serialize reliability state"))
}
fn decode<T: DeserializeOwned>(value: &str) -> ApiResult<T> {
    serde_json::from_str(value).map_err(|_| internal("Cannot decode reliability state"))
}
fn time(value: &str) -> ApiResult<i64> {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|v| v.timestamp_millis())
        .map_err(|_| internal("Invalid reliability timestamp"))
}
fn supported(resource: &Resource, collector: &str) -> bool {
    match collector {
        "iokit.block" => {
            resource.resource_type == "controller"
                && resource.attributes.get("scope").and_then(Value::as_str) == Some("driver")
                && resource.attributes.get("source").and_then(Value::as_str)
                    == Some("IOBlockStorageDriver")
        }
        "smartctl" => matches!(resource.resource_type.as_str(), "physical_device" | "media"),
        _ => false,
    }
}
fn identity(hb: &Heartbeat, resource: &Resource, collector: &str) -> ApiResult<SourceIdentity> {
    let namespace = Uuid::parse_str(&hb.node_id).map_err(|_| internal("Invalid node identity"))?;
    let key = encode(&json!(["reliability", resource.resource_id, collector]))?;
    Ok(SourceIdentity {
        source_id: Uuid::new_v5(&namespace, key.as_bytes()).to_string(),
        node_id: hb.node_id.clone(),
        object_id: Uuid::new_v5(
            &namespace,
            format!("ciderd:{}", resource.resource_id).as_bytes(),
        )
        .to_string(),
        resource_id: resource.resource_id.clone(),
        collector: collector.into(),
        scope: if collector == "iokit.block" {
            "driver"
        } else if resource.resource_type == "media" {
            "nvme_namespace"
        } else {
            "device"
        }
        .into(),
    })
}
async fn save_source(
    tx: &mut Transaction<'_, Sqlite>,
    state: &SourceState,
    support: &str,
    reason: Option<&str>,
    context: (&str, &str, &str),
    now: i64,
) -> ApiResult<()> {
    let encoded = encode(state)?;
    if encoded.len() > Policy::default().maximum_source_bytes {
        return Err(internal("Reliability state exceeds bounded storage"));
    }
    let mut summary = state.summary(now, true);
    summary["support_state"] = json!(support);
    summary["acquisition"] =
        json!({"boot_id":context.0,"agent_generation":context.1,"agent_session_id":context.2});
    if let Some(reason) = reason {
        summary["support_reason"] = json!(reason);
        summary["observation"]["state"] = json!("unavailable");
        summary["observation"]["reason"] = json!("capacity_limited");
    }
    sqlx::query("INSERT INTO reliability_sources(source_id,node_id,object_id,resource_id,collector,active,support_state,state_json,summary_json,boot_id,agent_generation,agent_session_id,updated_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(source_id) DO UPDATE SET active=excluded.active,support_state=excluded.support_state,state_json=excluded.state_json,summary_json=excluded.summary_json,boot_id=excluded.boot_id,agent_generation=excluded.agent_generation,agent_session_id=excluded.agent_session_id,updated_at=excluded.updated_at")
        .bind(&state.source.source_id).bind(&state.source.node_id).bind(&state.source.object_id).bind(&state.source.resource_id).bind(&state.source.collector).bind(i64::from(state.active)).bind(support).bind(encoded).bind(encode(&summary)?).bind(context.0).bind(context.1).bind(context.2).bind(now).execute(&mut **tx).await?;
    Ok(())
}
async fn save_finding(tx: &mut Transaction<'_, Sqlite>, finding: &Finding) -> ApiResult<()> {
    sqlx::query("INSERT INTO reliability_findings(finding_id,source_id,node_id,object_id,status,first_seen_at,updated_at,ended_at,finding_json) VALUES (?,?,?,?,?,?,?,?,?) ON CONFLICT(finding_id) DO UPDATE SET status=excluded.status,updated_at=excluded.updated_at,ended_at=excluded.ended_at,finding_json=excluded.finding_json")
        .bind(&finding.finding_id).bind(&finding.source_id).bind(&finding.node_id).bind(&finding.object_id).bind(&finding.status).bind(time(&finding.first_seen_at)?).bind(time(&finding.updated_at)?).bind(finding.ended_at.as_deref().map(time).transpose()?).bind(encode(finding)?).execute(&mut **tx).await?;
    Ok(())
}
pub async fn observe_collection(
    tx: &mut Transaction<'_, Sqlite>,
    hb: &Heartbeat,
    resource: &Resource,
    collection: &Collection,
    now: i64,
) -> ApiResult<()> {
    if !supported(resource, &collection.collector) {
        return Ok(());
    }
    let source = identity(hb, resource, &collection.collector)?;
    let row =
        sqlx::query("SELECT state_json,support_state FROM reliability_sources WHERE source_id=?")
            .bind(&source.source_id)
            .fetch_optional(&mut **tx)
            .await?;
    let mut state = match &row {
        Some(row) => decode::<SourceState>(&row.get::<String, _>("state_json"))?,
        None => SourceState::new(source),
    };
    let admitted = if row
        .as_ref()
        .is_some_and(|r| r.get::<String, _>("support_state") == "supported")
        && state.active
    {
        true
    } else {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM reliability_sources WHERE active=1 AND support_state='supported'",
        )
        .fetch_one(&mut **tx)
        .await?;
        count < Policy::default().maximum_active_sources as i64
    };
    state.active = true;
    let max_stale = if collection.collector == "smartctl" {
        900.
    } else {
        15.
    };
    let stale = hb
        .collector_states
        .iter()
        .find(|s| s.resource_id == resource.resource_id && s.collector == collection.collector)
        .map(|s| s.stale_after_seconds as f64)
        .unwrap_or(max_stale);
    let age = hb
        .monotonic_ns
        .get()
        .checked_sub(collection.finished_monotonic_ns.get())
        .filter(|_| hb.clock_id == collection.clock_id)
        .map(|delta| {
            delta as f64 / 1e9
                + time(&hb.created_at)
                    .map(|t| t.abs_diff(now) as f64 / 1000.)
                    .unwrap_or(f64::MAX)
        })
        .unwrap_or(f64::MAX);
    let generation = hb.agent_generation.to_string();
    let mut findings = if admitted {
        state.observe(Observation {
            collection_id: collection.collection_id.clone(),
            boot_id: hb.boot_id.clone(),
            agent_generation: generation.clone(),
            agent_session_id: hb.agent_session_id.clone(),
            clock_id: collection.clock_id.clone(),
            source_generation: encode(&json!([
                resource.attributes.get("source_generation"),
                resource.attributes.get("registry_entry_id"),
                resource.attributes.get("mount_generation")
            ]))?,
            source_version: collection.source_version.clone(),
            adapter_version: collection.adapter_version.clone(),
            finished_monotonic_ns: collection.finished_monotonic_ns,
            observed_at: collection.finished_at.clone(),
            received_at_ms: now,
            age_at_receipt_seconds: age,
            stale_after_seconds: stale,
            status: collection.status.clone(),
            metrics: collection.metrics.clone(),
        })
    } else {
        state.interrupt("capacity_limited", now)
    };
    let oversized = encode(&state)?.len() > Policy::default().maximum_source_bytes;
    if oversized {
        findings.extend(state.interrupt("state_size_limit", now));
        state = SourceState::new(state.source.clone());
    }
    let support = if admitted && !oversized {
        "supported"
    } else {
        "capacity_limited"
    };
    let reason = if oversized {
        Some("state_size_limit")
    } else if !admitted {
        Some("active_source_limit")
    } else {
        None
    };
    save_source(
        tx,
        &state,
        support,
        reason,
        (&hb.boot_id, &generation, &hb.agent_session_id),
        now,
    )
    .await?;
    for finding in findings {
        save_finding(tx, &finding).await?;
    }
    Ok(())
}
pub async fn reconcile_sources(
    tx: &mut Transaction<'_, Sqlite>,
    node_id: &str,
    boot: &str,
    generation: &str,
    session: &str,
    resources: &BTreeMap<String, Resource>,
    now: i64,
) -> ApiResult<()> {
    let rows = sqlx::query("SELECT * FROM reliability_sources WHERE node_id=? AND active=1")
        .bind(node_id)
        .fetch_all(&mut **tx)
        .await?;
    for row in rows {
        let gone = !resources
            .get(&row.get::<String, _>("resource_id"))
            .is_some_and(|r| supported(r, &row.get::<String, _>("collector")));
        let changed = row.get::<String, _>("boot_id") != boot
            || row.get::<String, _>("agent_generation") != generation
            || row.get::<String, _>("agent_session_id") != session;
        if !gone && !changed {
            continue;
        }
        let mut state: SourceState = decode(&row.get::<String, _>("state_json"))?;
        let findings = state.interrupt(
            if gone {
                "source_removed"
            } else {
                "identity_changed"
            },
            now,
        );
        state.active = false;
        // Keep the actual acquisition identity on retained evidence; a heartbeat is not a new observation.
        save_source(
            tx,
            &state,
            &row.get::<String, _>("support_state"),
            None,
            (
                &row.get::<String, _>("boot_id"),
                &row.get::<String, _>("agent_generation"),
                &row.get::<String, _>("agent_session_id"),
            ),
            now,
        )
        .await?;
        for finding in findings {
            save_finding(tx, &finding).await?;
        }
    }
    Ok(())
}
fn age_observation(value: &mut Value, elapsed: f64, online: bool, active: bool) {
    if let Some(age) = value["age_seconds"].as_f64() {
        value["age_seconds"] = json!(age + elapsed);
    }
    let stale = value["age_seconds"]
        .as_f64()
        .is_some_and(|v| v > value["stale_after_seconds"].as_f64().unwrap_or(0.));
    if value["state"] == "current" && (stale || !online || !active) {
        value["state"] = json!("stale");
        value["reason"] = json!(if !active {
            "source_inactive"
        } else if !online {
            "owner_unavailable"
        } else {
            "observation_stale"
        });
    }
}
fn refresh_summary(row: &sqlx::sqlite::SqliteRow, now: i64) -> ApiResult<Value> {
    let mut value: Value = decode(&row.get::<String, _>("summary_json"))?;
    let online = row
        .get::<Option<i64>, _>("last_seen_at")
        .is_some_and(|v| now.saturating_sub(v) < 30_000)
        && row.get::<Option<i64>, _>("goodbye_at").is_none()
        && row.get::<Option<i64>, _>("revoked_at").is_none();
    let elapsed = now.saturating_sub(row.get::<i64, _>("updated_at")).max(0) as f64 / 1000.;
    let active = value["active"] == true && value["support_state"] == "supported";
    age_observation(&mut value["observation"], elapsed, online, active);
    if let Some(signals) = value["signals"].as_array_mut() {
        for signal in signals {
            age_observation(&mut signal["observation"], elapsed, online, active);
        }
    }
    Ok(value)
}
pub async fn source_summaries_filtered(
    tx: &mut Transaction<'_, Sqlite>,
    now: i64,
    node: Option<&str>,
    object: Option<&str>,
) -> ApiResult<Vec<Value>> {
    let (count,bytes):(i64,i64)=sqlx::query_as("SELECT COUNT(*),COALESCE(SUM(length(CAST(summary_json AS BLOB))),0) FROM (SELECT summary_json FROM reliability_sources WHERE (? IS NULL OR node_id=?) AND (? IS NULL OR object_id=?) ORDER BY source_id LIMIT 10001)").bind(node).bind(node).bind(object).bind(object).fetch_one(&mut **tx).await?;
    if count > 10000 {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "source_limit",
            "Source selection exceeds 10000 records; narrow the node_id or object_id filter",
        ));
    }
    if bytes > 32 * 1024 * 1024 {
        return Err(ApiError::unavailable());
    }
    let rows=sqlx::query("SELECT s.summary_json,s.updated_at,n.last_seen_at,n.goodbye_at,n.revoked_at FROM reliability_sources s JOIN nodes n ON n.node_id=s.node_id WHERE (? IS NULL OR s.node_id=?) AND (? IS NULL OR s.object_id=?) ORDER BY s.source_id LIMIT 10001")
        .bind(node).bind(node).bind(object).bind(object).fetch_all(&mut **tx).await?;
    if rows.len() > 10000 {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "source_limit",
            "Source selection exceeds 10000 records; narrow the node_id or object_id filter",
        ));
    }
    rows.iter().map(|row| refresh_summary(row, now)).collect()
}
/// Disk reads use only active sources, independently of retained historical-source limits.
pub async fn active_summaries(tx: &mut Transaction<'_, Sqlite>, now: i64) -> ApiResult<Vec<Value>> {
    let (count,bytes):(i64,i64)=sqlx::query_as("SELECT COUNT(*),COALESCE(SUM(length(CAST(summary_json AS BLOB))),0) FROM (SELECT summary_json FROM reliability_sources WHERE active=1 AND support_state='supported' ORDER BY source_id LIMIT 8193)").fetch_one(&mut **tx).await?;
    if count > 8192 || bytes > 32 * 1024 * 1024 {
        return Err(ApiError::unavailable());
    }
    let rows=sqlx::query("SELECT s.summary_json,s.updated_at,n.last_seen_at,n.goodbye_at,n.revoked_at FROM reliability_sources s JOIN nodes n ON n.node_id=s.node_id WHERE s.active=1 AND s.support_state='supported' ORDER BY s.source_id LIMIT 8193").fetch_all(&mut **tx).await?;
    if rows.len() > 8192 {
        return Err(ApiError::unavailable());
    }
    rows.iter().map(|row| refresh_summary(row, now)).collect()
}
pub async fn retain(tx: &mut Transaction<'_, Sqlite>, now: i64) -> ApiResult<()> {
    sqlx::query("DELETE FROM reliability_findings WHERE status<>'open' AND ended_at<?")
        .bind(now - Policy::default().closed_finding_retention_seconds as i64 * 1000)
        .execute(&mut **tx)
        .await?;
    Ok(())
}
