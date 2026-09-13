//! Detector persistence is called only inside the accepted native heartbeat transaction.
use crate::{
    cider_wire::{Collection, Heartbeat, Resource},
    detection::{
        Continuity, Finding, Observation, POLICY_VERSION, Policy, SourceIdentity, SourceState,
    },
    error::{ApiError, ApiResult},
    store,
};
use axum::http::StatusCode;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sqlx::{Row, Sqlite, Transaction};
use std::collections::BTreeMap;
use uuid::Uuid;

fn encode(value: &impl Serialize) -> ApiResult<String> {
    serde_json::to_string(value).map_err(|_| internal("Cannot serialize detector state"))
}
fn decode<T: DeserializeOwned>(value: &str) -> ApiResult<T> {
    serde_json::from_str(value).map_err(|_| internal("Cannot decode detector state"))
}
fn internal(message: &str) -> ApiError {
    ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", message)
}
fn time(value: &str) -> ApiResult<i64> {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|v| v.timestamp_millis())
        .map_err(|_| internal("Invalid persisted detector timestamp"))
}
fn driver(resource: &Resource) -> bool {
    resource.resource_type == "controller"
        && resource.attributes.get("scope").and_then(Value::as_str) == Some("driver")
        && resource.attributes.get("source").and_then(Value::as_str) == Some("IOBlockStorageDriver")
}
fn identity(
    hb: &Heartbeat,
    resource: &Resource,
    direction: &str,
    attrs: BTreeMap<String, Value>,
) -> ApiResult<SourceIdentity> {
    let namespace =
        Uuid::parse_str(&hb.node_id).map_err(|_| internal("Invalid enrolled node identity"))?;
    let metric = format!("storage.device.{direction}_bytes_total");
    let key = encode(&json!([
        "storage.activity.high_rate.v1",
        resource.resource_id,
        "iokit.block",
        "driver",
        metric,
        attrs
    ]))?;
    Ok(SourceIdentity {
        source_id: Uuid::new_v5(&namespace, key.as_bytes()).to_string(),
        node_id: hb.node_id.clone(),
        object_id: Uuid::new_v5(
            &namespace,
            format!("ciderd:{}", resource.resource_id).as_bytes(),
        )
        .to_string(),
        resource_id: resource.resource_id.clone(),
        direction: direction.into(),
        metric,
        collector: "iokit.block".into(),
        scope: "driver".into(),
        attributes: attrs,
    })
}

async fn save_source(
    tx: &mut Transaction<'_, Sqlite>,
    state: &SourceState,
    support: &str,
    reason: Option<&str>,
    hb_identity: (&str, &str, &str),
    now: i64,
) -> ApiResult<()> {
    let encoded = encode(state)?;
    if encoded.len() > Policy::default().maximum_source_bytes {
        return Err(internal("Detector state exceeds bounded storage"));
    }
    let mut summary = serde_json::to_value(state.summary(now, true))
        .map_err(|_| internal("Cannot serialize detector summary"))?;
    summary["support_state"] = json!(support);
    summary["baseline"]["as_of"] = json!(store::timestamp(now));
    if let Some(reason) = reason {
        summary["support_reason"] = json!(reason);
        summary["observation"]["state"] = json!("unavailable");
        summary["observation"]["reason"] = json!("capacity_limited");
        summary["observation"]["rate_bytes_per_second"] = Value::Null;
    }
    sqlx::query("INSERT INTO detection_sources(source_id,node_id,object_id,resource_id,direction,active,support_state,state_json,summary_json,support_reason,boot_id,agent_generation,agent_session_id,updated_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(source_id) DO UPDATE SET active=excluded.active,support_state=excluded.support_state,state_json=excluded.state_json,summary_json=excluded.summary_json,support_reason=excluded.support_reason,boot_id=excluded.boot_id,agent_generation=excluded.agent_generation,agent_session_id=excluded.agent_session_id,updated_at=excluded.updated_at")
        .bind(&state.source.source_id).bind(&state.source.node_id).bind(&state.source.object_id).bind(&state.source.resource_id).bind(&state.source.direction)
        .bind(i64::from(state.active)).bind(support).bind(encoded).bind(encode(&summary)?).bind(reason)
        .bind(hb_identity.0).bind(hb_identity.1).bind(hb_identity.2).bind(now).execute(&mut **tx).await?;
    Ok(())
}
async fn save_finding(tx: &mut Transaction<'_, Sqlite>, finding: &Finding) -> ApiResult<()> {
    let ended = finding.ended_at.as_deref().map(time).transpose()?;
    sqlx::query("INSERT INTO detection_findings(finding_id,source_id,node_id,object_id,status,first_seen_at,updated_at,ended_at,finding_json) VALUES (?,?,?,?,?,?,?,?,?) ON CONFLICT(finding_id) DO UPDATE SET status=excluded.status,updated_at=excluded.updated_at,ended_at=excluded.ended_at,finding_json=excluded.finding_json")
        .bind(&finding.finding_id).bind(&finding.source_id).bind(&finding.node_id).bind(&finding.object_id).bind(&finding.status)
        .bind(time(&finding.first_seen_at)?).bind(time(&finding.updated_at)?).bind(ended).bind(encode(finding)?).execute(&mut **tx).await?;
    Ok(())
}

pub async fn observe_collection(
    tx: &mut Transaction<'_, Sqlite>,
    hb: &Heartbeat,
    resource: &Resource,
    collection: &Collection,
    now: i64,
) -> ApiResult<()> {
    if collection.collector != "iokit.block" || !driver(resource) {
        return Ok(());
    }
    let generation = hb.agent_generation.to_string();
    let stale = hb
        .collector_states
        .iter()
        .find(|s| s.resource_id == resource.resource_id && s.collector == collection.collector)
        .map(|s| s.stale_after_seconds as f64)
        .unwrap_or(15.);
    // Acquisition age on the node monotonic clock plus absolute wall-clock skew/delay.
    let age = hb
        .monotonic_ns
        .get()
        .checked_sub(collection.finished_monotonic_ns.get())
        .filter(|_| hb.clock_id == collection.clock_id)
        .map(|v| {
            v as f64 / 1_000_000_000.
                + time(&hb.created_at)
                    .map(|t| t.abs_diff(now) as f64 / 1000.)
                    .unwrap_or(f64::MAX)
        })
        .unwrap_or(f64::MAX);
    for direction in ["read", "write"] {
        let name = format!("storage.device.{direction}_bytes_total");
        // Schema-2 validation requires empty dimensions for these byte counters
        // and rejects duplicate metric keys before reaching this transaction.
        let metric = collection.metrics.iter().find(|m| m.name == name);
        let source = identity(
            hb,
            resource,
            direction,
            metric.map(|m| m.attributes.clone()).unwrap_or_default(),
        )?;
        let row=sqlx::query("SELECT state_json,support_state,support_reason FROM detection_sources WHERE source_id=?")
            .bind(&source.source_id).fetch_optional(&mut **tx).await?;
        let mut state = match &row {
            Some(row) => decode::<SourceState>(&row.get::<String, _>("state_json"))?,
            None => SourceState::new(source),
        };
        let already_admitted = row
            .as_ref()
            .is_some_and(|r| r.get::<String, _>("support_state") == "supported")
            && state.active;
        let admitted = if already_admitted {
            true
        } else {
            let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM detection_sources WHERE active=1 AND support_state='supported'").fetch_one(&mut **tx).await?;
            count < Policy::default().maximum_active_sources as i64
        };
        state.active = true;
        let epoch = metric
            .and_then(|m| m.counter_epoch.clone())
            .or_else(|| state.continuity().map(|c| c.counter_epoch.clone()))
            .unwrap_or_default();
        let continuity = Continuity {
            boot_id: hb.boot_id.clone(),
            agent_generation: generation.clone(),
            agent_session_id: hb.agent_session_id.clone(),
            clock_id: collection.clock_id.clone(),
            counter_epoch: epoch,
            source_version: collection.source_version.clone(),
            adapter_version: collection.adapter_version.clone(),
            policy_version: POLICY_VERSION.into(),
        };
        let unavailable = if !admitted {
            Some("capacity_limited".into())
        } else if !matches!(collection.status.as_str(), "ok" | "partial") {
            Some(format!("collection_{}", collection.status))
        } else {
            match metric {
                None => Some("missing_metric".into()),
                Some(m) if m.availability != "available" => Some(m.availability.clone()),
                Some(m) if m.freshness.as_deref() != Some("live") => Some("nonlive_metric".into()),
                Some(m)
                    if m.kind != "counter"
                        || m.unit != "bytes"
                        || m.counter_epoch.as_deref().is_none_or(str::is_empty) =>
                {
                    Some("unsupported_metric".into())
                }
                _ => None,
            }
        };
        let counter = if unavailable.is_none() {
            metric
                .and_then(|m| m.value.as_ref())
                .and_then(Value::as_str)
                .and_then(|s| s.to_owned().try_into().ok())
        } else {
            None
        };
        let mut findings = state.observe(Observation {
            collection_id: collection.collection_id.clone(),
            continuity,
            finished_monotonic_ns: collection.finished_monotonic_ns,
            observed_at: collection.finished_at.clone(),
            received_at_ms: now,
            counter,
            unavailable_reason: unavailable,
            age_at_receipt_seconds: age,
            stale_after_seconds: stale,
        });
        let oversized = encode(&state)?.len() > Policy::default().maximum_source_bytes;
        if oversized {
            if let Some(finding) = state.enforce_size_limit(now) {
                findings.push(finding);
            }
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
    }
    Ok(())
}

pub async fn reconcile_sources(
    tx: &mut Transaction<'_, Sqlite>,
    node_id: &str,
    boot_id: &str,
    generation: &str,
    session_id: &str,
    resources: &BTreeMap<String, Resource>,
    now: i64,
) -> ApiResult<()> {
    let rows=sqlx::query("SELECT source_id,resource_id,boot_id,agent_generation,agent_session_id FROM detection_sources WHERE node_id=? AND active=1")
        .bind(node_id).fetch_all(&mut **tx).await?;
    for row in rows {
        let gone = !resources
            .get(&row.get::<String, _>("resource_id"))
            .is_some_and(driver);
        let changed = row.get::<String, _>("boot_id") != boot_id
            || row.get::<String, _>("agent_generation") != generation
            || row.get::<String, _>("agent_session_id") != session_id;
        if !gone && !changed {
            continue;
        }
        let row=sqlx::query("SELECT state_json,support_state,support_reason FROM detection_sources WHERE source_id=?").bind(row.get::<String,_>("source_id")).fetch_one(&mut **tx).await?;
        let mut state: SourceState = decode(&row.get::<String, _>("state_json"))?;
        let finding = state.interrupt(
            if gone {
                "source_removed"
            } else {
                "identity_changed"
            },
            now,
        );
        // Preserve continuity watermark until the first new collection proves its replacement.
        state.active = false;
        save_source(
            tx,
            &state,
            &row.get::<String, _>("support_state"),
            row.get::<Option<String>, _>("support_reason").as_deref(),
            (boot_id, generation, session_id),
            now,
        )
        .await?;
        if let Some(finding) = finding {
            save_finding(tx, &finding).await?;
        }
    }
    Ok(())
}

/// Read compact cached summaries only; baseline rings never enter a read response path.
pub async fn source_summaries(tx: &mut Transaction<'_, Sqlite>, now: i64) -> ApiResult<Vec<Value>> {
    source_summaries_filtered(tx, now, None, None).await
}

pub async fn source_summaries_filtered(
    tx: &mut Transaction<'_, Sqlite>,
    now: i64,
    node_id: Option<&str>,
    object_id: Option<&str>,
) -> ApiResult<Vec<Value>> {
    let rows=sqlx::query("SELECT s.summary_json,s.updated_at,n.last_seen_at,n.goodbye_at,n.revoked_at FROM detection_sources s JOIN nodes n ON n.node_id=s.node_id WHERE (? IS NULL OR s.node_id=?) AND (? IS NULL OR s.object_id=?) ORDER BY s.source_id LIMIT 10001")
        .bind(node_id).bind(node_id).bind(object_id).bind(object_id).fetch_all(&mut **tx).await?;
    if rows.len() > 10_000 {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "source_limit",
            "Source selection exceeds 10000 records; narrow the node_id or object_id filter",
        ));
    }
    rows.into_iter()
        .map(|row| refresh_summary(&row, now))
        .collect()
}

fn refresh_summary(row: &sqlx::sqlite::SqliteRow, now: i64) -> ApiResult<Value> {
    let mut value: Value = decode(&row.get::<String, _>("summary_json"))?;
    let owner_online = row
        .get::<Option<i64>, _>("last_seen_at")
        .is_some_and(|seen| now.saturating_sub(seen) < 30_000)
        && row.get::<Option<i64>, _>("goodbye_at").is_none()
        && row.get::<Option<i64>, _>("revoked_at").is_none();
    let elapsed = now.saturating_sub(row.get::<i64, _>("updated_at")).max(0) as f64 / 1000.;
    if let Some(age) = value["observation"]["age_seconds"].as_f64() {
        value["observation"]["age_seconds"] = json!(age + elapsed);
    }
    let stale = value["observation"]["age_seconds"]
        .as_f64()
        .is_some_and(|age| {
            age > value["observation"]["stale_after_seconds"]
                .as_f64()
                .unwrap_or(15.)
                .min(15.)
        });
    if value["active"] == false
        || (value["observation"]["state"] == "current" && (!owner_online || stale))
    {
        value["observation"]["state"] = json!("stale");
        value["observation"]["reason"] = json!(if value["active"] == false {
            "source_inactive"
        } else if !owner_online {
            "owner_offline"
        } else {
            "observation_stale"
        });
        value["observation"]["rate_bytes_per_second"] = Value::Null;
    }
    Ok(value)
}

/// Existing core reads need current warning evidence only, independent of historical coverage limits.
pub async fn current_warning_summaries(
    tx: &mut Transaction<'_, Sqlite>,
    now: i64,
) -> ApiResult<Vec<Value>> {
    let rows=sqlx::query("SELECT s.summary_json,s.updated_at,n.last_seen_at,n.goodbye_at,n.revoked_at FROM detection_sources s JOIN nodes n ON n.node_id=s.node_id WHERE s.active=1 AND s.support_state='supported' AND json_extract(s.summary_json,'$.episode.state')='open' ORDER BY s.source_id LIMIT 8193")
        .fetch_all(&mut **tx).await?;
    if rows.len() > 8192 {
        return Err(ApiError::unavailable());
    }
    let summaries: Vec<Value> = rows
        .into_iter()
        .map(|row| refresh_summary(&row, now))
        .collect::<ApiResult<_>>()?;
    Ok(summaries
        .into_iter()
        .filter(|v| v["observation"]["state"] == "current")
        .collect())
}

pub async fn rebaseline(
    tx: &mut Transaction<'_, Sqlite>,
    source_id: &str,
    expected_revision: u64,
    reason: &str,
    actor_hash: &str,
    now: i64,
) -> ApiResult<Value> {
    let row = sqlx::query("SELECT * FROM detection_sources WHERE source_id=?")
        .bind(source_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_FOUND,
                "not_found",
                "Detector source not found",
            )
        })?;
    let mut state: SourceState = decode(&row.get::<String, _>("state_json"))?;
    if state.baseline_revision != expected_revision {
        return Err(ApiError::conflict(
            "Baseline revision changed; refresh source status",
        ));
    }
    if !state.active || row.get::<String, _>("support_state") != "supported" {
        return Err(ApiError::conflict("Source is inactive or capacity limited"));
    }
    let finding = state
        .rebaseline(reason, now)
        .map_err(|message| ApiError::field("reason", message))?;
    save_source(
        tx,
        &state,
        "supported",
        None,
        (
            &row.get::<String, _>("boot_id"),
            &row.get::<String, _>("agent_generation"),
            &row.get::<String, _>("agent_session_id"),
        ),
        now,
    )
    .await?;
    if let Some(finding) = finding {
        save_finding(tx, &finding).await?;
    }
    sqlx::query("INSERT INTO detection_rebaseline_audit(source_id,actor_hash,reason,previous_revision,new_revision,created_at) VALUES (?,?,?,?,?,?)")
        .bind(source_id).bind(actor_hash).bind(reason).bind(expected_revision.to_string()).bind(state.baseline_revision.to_string()).bind(now).execute(&mut **tx).await?;
    store::change(tx,"detection_rebaseline",Some(&state.source.node_id),json!({"source_id":source_id,"previous_revision":expected_revision.to_string(),"baseline_revision":state.baseline_revision.to_string(),"reason":reason})).await?;
    let mut summary = serde_json::to_value(state.summary(now, true))
        .map_err(|_| internal("Cannot serialize detector summary"))?;
    summary["baseline"]["as_of"] = json!(store::timestamp(now));
    Ok(summary)
}

pub async fn retain(tx: &mut Transaction<'_, Sqlite>, now: i64) -> ApiResult<()> {
    sqlx::query("DELETE FROM detection_findings WHERE status<>'open' AND ended_at<?")
        .bind(now - Policy::default().closed_finding_retention_seconds as i64 * 1000)
        .execute(&mut **tx)
        .await?;
    Ok(())
}
