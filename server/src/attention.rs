//! Durable operator episodes. Missing observations never establish recovery.
use crate::{
    error::{ApiError, ApiResult},
    store::{self, AppState},
};
use axum::http::StatusCode;
use serde_json::{Value, json};
use sqlx::{Row, Sqlite, Transaction, sqlite::SqliteRow};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;
const MAX_EPISODES: i64 = 10_000;
const RETENTION_MS: i64 = 30 * 24 * 60 * 60 * 1000;
pub struct Condition {
    pub key: String,
    pub kind: String,
    pub node_id: String,
    pub object_id: Option<String>,
    pub status: String,
    pub severity: String,
    pub observation_state: String,
    pub summary: String,
    pub evidence: Value,
}
fn decode(row: &SqliteRow) -> ApiResult<Value> {
    let evidence: Value = serde_json::from_str(row.get("evidence_json"))
        .map_err(|_| ApiError::conflict("Invalid persisted attention evidence"))?;
    let ack: Option<String> = row.get("ack_json");
    Ok(
        json!({"id":row.get::<String,_>("id"),"source_key":row.get::<String,_>("source_key"),"kind":row.get::<String,_>("kind"),"node_id":row.get::<String,_>("node_id"),"object_id":row.get::<Option<String>,_>("object_id"),"status":row.get::<String,_>("status"),"severity":row.get::<String,_>("severity"),"observation_state":row.get::<String,_>("observation_state"),"summary":row.get::<String,_>("summary"),"evidence":evidence,"revision":row.get::<String,_>("revision"),"first_seen_at":store::timestamp(row.get("first_seen_at")),"updated_at":store::timestamp(row.get("updated_at")),"resolved_at":row.get::<Option<i64>,_>("resolved_at").map(store::timestamp),"notification_intent":crate::notifications::intent_view(row.get("notification_intent_json")),"acknowledgement":ack.and_then(|a|serde_json::from_str::<Value>(&a).ok())}),
    )
}
pub async fn observe_condition(
    tx: &mut Transaction<'_, Sqlite>,
    c: &Condition,
    now: i64,
) -> ApiResult<Option<Value>> {
    if !matches!(c.status.as_str(), "open" | "resolved" | "interrupted")
        || !matches!(c.severity.as_str(), "info" | "warning" | "critical")
        || c.key.len() > 512
        || c.summary.len() > 2048
        || c.evidence.to_string().len() > 65536
    {
        return Err(ApiError::field(
            "condition",
            "Invalid or oversized condition",
        ));
    }
    let previous=sqlx::query("SELECT * FROM attention_episodes WHERE source_key=? ORDER BY (status='open') DESC,first_seen_at DESC,id DESC LIMIT 1").bind(&c.key).fetch_optional(&mut **tx).await?;
    let old = previous.as_ref().map(decode).transpose()?;
    let currently_open = old.as_ref().is_some_and(|v| v["status"] == "open");
    if !currently_open && c.status != "open" {
        return Ok(old);
    }
    if !currently_open {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM attention_episodes")
            .fetch_one(&mut **tx)
            .await?;
        if count >= MAX_EPISODES {
            sqlx::query("UPDATE notification_settings SET admission_limited=1 WHERE id=1")
                .execute(&mut **tx)
                .await?;
            return Ok(None);
        }
    }
    let fresh = matches!(c.observation_state.as_str(), "ok" | "current");
    let status = if c.status == "resolved" && fresh {
        "resolved"
    } else {
        "open"
    };
    let id = if currently_open {
        old.as_ref().unwrap()["id"].as_str().unwrap().to_owned()
    } else {
        Uuid::new_v4().to_string()
    };
    let transition = !currently_open
        || (fresh
            && old
                .as_ref()
                .is_some_and(|v| v["severity"] != c.severity || v["status"] != status));
    let revision = if transition {
        Uuid::new_v4().to_string()
    } else {
        old.as_ref().unwrap()["revision"].as_str().unwrap().into()
    };
    let severity = if !fresh && currently_open {
        old.as_ref().unwrap()["severity"].as_str().unwrap()
    } else {
        &c.severity
    };
    let ack = if transition {
        None
    } else {
        previous
            .as_ref()
            .and_then(|r| r.get::<Option<String>, _>("ack_json"))
    };
    let watch = if c.kind=="drive_removal" { c.evidence["usb_watch_id"].as_str() }
        else if c.evidence["rule_id"]=="usb_connection_absent" { c.evidence["watch_id"].as_str() }
        else { None };
    let correlated:Option<String> = if transition && status=="open" {
        if let (Some(watch),Some(epoch))=(watch,c.evidence["connection_epoch"].as_str().filter(|v|!v.is_empty())) {
            sqlx::query_scalar("SELECT id FROM attention_episodes WHERE node_id=? AND id!=? AND status='open' AND json_extract(evidence_json,'$.connection_epoch')=? AND ((kind='drive_removal' AND json_extract(evidence_json,'$.usb_watch_id')=?) OR source_key=?) ORDER BY first_seen_at,id LIMIT 1")
                .bind(&c.node_id).bind(&id).bind(epoch).bind(watch).bind(format!("device_presence:{watch}"))
                .fetch_optional(&mut **tx).await?
        } else { None }
    } else { None };
    let mut evidence=c.evidence.clone();
    if let Some(other)=&correlated {
        evidence["notification_correlation"]=json!({"episode_id":other,"reason":"same_observed_usb_disconnect"});
    } else if !transition {
        for key in ["notification_correlation","notification_transfer_pending","notification_transfer_from"] {
            if let Some(value)=old.as_ref().and_then(|v|v["evidence"].get(key)) {
                evidence[key]=value.clone();
            }
        }
    }
    let transferred=evidence["notification_transfer_pending"].clone();
    let resume_transfer=if fresh && c.status=="open" && ack.is_none() && transferred.is_object()
        && transferred["connection_epoch"]==c.evidence["connection_epoch"]
        && transferred["created_at"].as_i64().is_some_and(|t|t<=now && now-t<=24*60*60*1000) {
        sqlx::query_scalar::<_,bool>("SELECT enabled=1 AND revision=? FROM notification_settings WHERE id=1")
            .bind(transferred["settings_revision"].as_str().unwrap_or("")).fetch_one(&mut **tx).await?
    } else {false};
    if resume_transfer {
        evidence.as_object_mut().expect("transfer evidence object").remove("notification_transfer_pending");
        evidence.as_object_mut().expect("transfer evidence object").remove("notification_correlation");
        evidence["notification_transfer_from"]=transferred["episode_id"].clone();
    }
    // If the first of two correlated observations recovers before delivery,
    // preserve its still-needed notification on the other current concern.
    let transfer_pending:bool=if status=="resolved" {
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM attention_episodes e JOIN notification_settings s ON s.id=1 WHERE e.id=? AND s.enabled=1 AND (EXISTS(SELECT 1 FROM notification_outbox o WHERE o.episode_id=e.id AND o.state='queued' AND o.settings_revision=s.revision) OR (json_extract(e.notification_intent_json,'$.state')='pending_admission' AND json_extract(e.notification_intent_json,'$.settings_revision')=s.revision AND json_extract(e.notification_intent_json,'$.created_at')>=?)))")
            .bind(&id).bind(now-24*60*60*1000).fetch_one(&mut **tx).await?
    } else {false};
    sqlx::query("INSERT INTO attention_episodes(id,source_key,kind,node_id,object_id,status,severity,observation_state,summary,evidence_json,revision,first_seen_at,updated_at,resolved_at,ack_json) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET node_id=excluded.node_id,object_id=excluded.object_id,status=excluded.status,severity=excluded.severity,observation_state=excluded.observation_state,summary=excluded.summary,evidence_json=excluded.evidence_json,revision=excluded.revision,updated_at=excluded.updated_at,resolved_at=excluded.resolved_at,ack_json=excluded.ack_json")
 .bind(&id).bind(&c.key).bind(&c.kind).bind(&c.node_id).bind(&c.object_id).bind(status).bind(severity).bind(&c.observation_state).bind(&c.summary).bind(evidence.to_string()).bind(&revision).bind(now).bind(now).bind(if status=="resolved"{Some(now)}else{None}).bind(ack).execute(&mut **tx).await?;
    if transition {
        crate::notifications::cancel_intent(
            tx,
            Some(&id),
            if status == "resolved" {
                "condition_resolved"
            } else {
                "superseded"
            },
            now,
        )
        .await?;
    }
    if transition && status == "open" && correlated.is_none() && matches!(severity, "warning" | "critical") {
        crate::notifications::enqueue(tx, &id, &revision, now).await?;
    }
    if resume_transfer {crate::notifications::enqueue(tx,&id,&revision,now).await?;}
    if status == "resolved" {
        crate::notifications::cancel_intent(tx, Some(&id), "condition_resolved", now).await?;
        sqlx::query("UPDATE notification_outbox SET state='suppressed',reason='condition_resolved',updated_at=? WHERE episode_id=? AND state='queued'").bind(now).bind(&id).execute(&mut **tx).await?;
        if transfer_pending {
            if let Some(other)=sqlx::query("SELECT id,revision,source_key,evidence_json FROM attention_episodes WHERE node_id=? AND status='open' AND ack_json IS NULL AND json_extract(evidence_json,'$.notification_correlation.episode_id')=? ORDER BY first_seen_at,id LIMIT 1")
                .bind(&c.node_id).bind(&id).fetch_optional(&mut **tx).await? {
                let other_id:String=other.get("id");
                if other.get::<String,_>("source_key").starts_with("device_presence:") {
                    // Reconciliation/ingestion must re-evaluate the USB source;
                    // a stored current state can have become stale meanwhile.
                    let settings_revision:String=sqlx::query_scalar("SELECT revision FROM notification_settings WHERE id=1").fetch_one(&mut **tx).await?;
                    let other_evidence:Value=serde_json::from_str(other.get("evidence_json")).map_err(|_|ApiError::unavailable())?;
                    let pending=json!({"episode_id":id,"settings_revision":settings_revision,"created_at":now,
                        "connection_epoch":other_evidence["connection_epoch"]});
                    sqlx::query("UPDATE attention_episodes SET evidence_json=json_set(evidence_json,'$.notification_transfer_pending',json(?)),updated_at=? WHERE id=?")
                        .bind(pending.to_string()).bind(now).bind(&other_id).execute(&mut **tx).await?;
                } else {
                    // Physical removals are already committed event evidence.
                    crate::notifications::enqueue(tx,&other_id,&other.get::<String,_>("revision"),now).await?;
                    sqlx::query("UPDATE attention_episodes SET evidence_json=json_set(json_remove(evidence_json,'$.notification_correlation'),'$.notification_transfer_from',?),updated_at=? WHERE id=?")
                        .bind(&id).bind(now).bind(&other_id).execute(&mut **tx).await?;
                }
            }
        }
    }
    let row = sqlx::query("SELECT * FROM attention_episodes WHERE id=?")
        .bind(id)
        .fetch_one(&mut **tx)
        .await?;
    Ok(Some(decode(&row)?))
}
pub async fn observe_capacity(
    tx: &mut Transaction<'_, Sqlite>,
    key: &str,
    node: &str,
    object: Option<&str>,
    ratio: &Value,
    now: i64,
) -> ApiResult<()> {
    let value = ratio["value"]
        .as_f64()
        .filter(|v| v.is_finite() && (0.0..=1.0).contains(v));
    let fresh = ratio["state"] == "ok" && value.is_some();
    let pressure = fresh && value.unwrap() >= 0.9;
    observe_condition(
        tx,
        &Condition {
            key: key.into(),
            kind: "capacity".into(),
            node_id: node.into(),
            object_id: object.map(str::to_owned),
            status: if !fresh {
                "interrupted"
            } else if pressure {
                "open"
            } else {
                "resolved"
            }
            .into(),
            severity: if value.is_some_and(|v| v >= 0.95) {
                "critical"
            } else {
                "warning"
            }
            .into(),
            observation_state: if fresh {
                "ok"
            } else {
                ratio["state"]
                    .as_str()
                    .filter(|s| *s != "ok")
                    .unwrap_or("unknown")
            }
            .into(),
            summary: if pressure {
                "Observed capacity pressure"
            } else if fresh {
                "Capacity pressure recovered"
            } else {
                "Capacity observation unavailable; recovery unknown"
            }
            .into(),
            evidence: json!({"used_ratio":ratio,"warning_ratio":0.9,"critical_ratio":0.95}),
        },
        now,
    )
    .await?;
    Ok(())
}
pub async fn acknowledge(
    tx: &mut Transaction<'_, Sqlite>,
    id: &str,
    revision: &str,
    note: &str,
    actor: &str,
    now: i64,
) -> ApiResult<Value> {
    if note.len() > 2048 {
        return Err(ApiError::field("note", "Maximum 2048 bytes"));
    }
    let row = sqlx::query("SELECT * FROM attention_episodes WHERE id=?")
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_FOUND,
                "not_found",
                "Attention episode not found",
            )
        })?;
    if row.get::<String, _>("revision") != revision {
        return Err(ApiError::conflict("Attention revision changed"));
    }
    if row.get::<String, _>("status") != "open" {
        return Err(ApiError::conflict("Episode is already resolved"));
    }
    let ack = json!({"at":store::timestamp(now),"note":note,"actor_hash":store::fingerprint(actor.as_bytes())});
    sqlx::query("UPDATE attention_episodes SET ack_json=?,revision=?,updated_at=? WHERE id=?")
        .bind(ack.to_string())
        .bind(Uuid::new_v4().to_string())
        .bind(now)
        .bind(id)
        .execute(&mut **tx)
        .await?;
    crate::notifications::cancel_intent(tx, Some(id), "acknowledged", now).await?;
    sqlx::query("UPDATE notification_outbox SET state='suppressed',reason='acknowledged',updated_at=? WHERE episode_id=? AND state='queued'").bind(now).bind(id).execute(&mut **tx).await?;
    decode(
        &sqlx::query("SELECT * FROM attention_episodes WHERE id=?")
            .bind(id)
            .fetch_one(&mut **tx)
            .await?,
    )
}
pub async fn list(
    state: &AppState,
    query: &BTreeMap<String, String>,
    now: i64,
) -> ApiResult<(Value, Value)> {
    let mut tx = state.db.begin().await?;
    let result = list_snapshot(&mut tx, query, now).await?;
    tx.commit().await?;
    Ok(result)
}
async fn list_snapshot(
    tx: &mut Transaction<'_, Sqlite>,
    query: &BTreeMap<String, String>,
    now: i64,
) -> ApiResult<(Value, Value)> {
    for key in query.keys() {
        if !matches!(
            key.as_str(),
            "node_id" | "object_id" | "status" | "kind" | "severity" | "acknowledged" | "limit"
        ) {
            return Err(ApiError::field(key, "Unsupported query parameter"));
        }
    }
    if query
        .get("status")
        .is_some_and(|s| !matches!(s.as_str(), "open" | "resolved" | "all"))
    {
        return Err(ApiError::field("status", "Use open, resolved, or all"));
    }
    if query
        .get("acknowledged")
        .is_some_and(|v| !matches!(v.as_str(), "true" | "false"))
    {
        return Err(ApiError::field("acknowledged", "Use true or false"));
    }
    if query
        .get("severity")
        .is_some_and(|v| !matches!(v.as_str(), "info" | "warning" | "critical"))
    {
        return Err(ApiError::field(
            "severity",
            "Use info, warning, or critical",
        ));
    }
    if query.get("kind").is_some_and(|v| {
        !matches!(
            v.as_str(),
            "activity" | "reliability" | "capacity" | "node_loss" | "filesystem" | "drive_removal"
        )
    }) {
        return Err(ApiError::field("kind", "Unknown attention kind"));
    }
    let limit = query
        .get("limit")
        .map(|s| s.parse::<usize>())
        .transpose()
        .map_err(|_| ApiError::field("limit", "Invalid limit"))?
        .unwrap_or(100);
    if !(1..=500).contains(&limit) {
        return Err(ApiError::field("limit", "Use 1 through 500"));
    }
    let node = query.get("node_id");
    let object = query.get("object_id");
    let status = query.get("status").filter(|s| s.as_str() != "all");
    let kind = query.get("kind");
    let severity = query.get("severity");
    let ack = query.get("acknowledged").map(|s| i64::from(s == "true"));
    let predicate = "WHERE (?1 IS NULL OR node_id=?1) AND (?2 IS NULL OR object_id=?2) AND (?3 IS NULL OR status=?3) AND (?4 IS NULL OR kind=?4) AND (?5 IS NULL OR severity=?5) AND (?6 IS NULL OR (ack_json IS NOT NULL)=?6)";
    let (count,bytes):(i64,i64)=sqlx::query_as(&format!("SELECT COUNT(*),COALESCE(SUM(length(CAST(evidence_json AS BLOB))+length(CAST(summary AS BLOB))),0) FROM attention_episodes {predicate}"))
    .bind(node).bind(object).bind(status).bind(kind).bind(severity).bind(ack).fetch_one(&mut **tx).await?;
    if count > MAX_EPISODES || bytes > 32 * 1024 * 1024 {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "attention_limit",
            "Attention selection exceeds its bounded read budget; narrow filters",
        ));
    }
    let rows=sqlx::query(&format!("SELECT * FROM attention_episodes {predicate} ORDER BY CASE status WHEN 'open' THEN 0 ELSE 1 END,CASE severity WHEN 'critical' THEN 0 WHEN 'warning' THEN 1 ELSE 2 END,updated_at DESC,id DESC LIMIT 10001"))
    .bind(node).bind(object).bind(status).bind(kind).bind(severity).bind(ack).fetch_all(&mut **tx).await?;
    let actual_bytes: usize = rows
        .iter()
        .map(|r| r.get::<String, _>("evidence_json").len() + r.get::<String, _>("summary").len())
        .sum();
    if rows.len() > MAX_EPISODES as usize || actual_bytes > 32 * 1024 * 1024 {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "attention_limit",
            "Attention selection exceeds its bounded read budget; narrow filters",
        ));
    }
    let mut values = Vec::new();
    for row in rows {
        let v = decode(&row)?;
        if query.iter().all(|(k, w)| match k.as_str() {
            "limit" => true,
            "status" if w == "all" => true,
            "acknowledged" => v["acknowledgement"].is_object() == (w == "true"),
            _ => v[k].as_str() == Some(w),
        }) {
            values.push(v);
        }
    }
    let matched = values.len();

    let deliveries=sqlx::query("SELECT episode_id,state,attempts,provider_status,reason,updated_at FROM notification_outbox ORDER BY created_at DESC,id DESC LIMIT 1000").fetch_all(&mut **tx).await?;
    let mut latest = BTreeMap::new();
    for r in deliveries {
        latest.entry(r.get::<String,_>("episode_id")).or_insert_with(||json!({"state":r.get::<String,_>("state"),"attempts":r.get::<i64,_>("attempts"),"provider_status":r.get::<Option<String>,_>("provider_status"),"reason":r.get::<Option<String>,_>("reason"),"updated_at":store::timestamp(r.get("updated_at"))}));
    }
    let status_row=sqlx::query("SELECT last_reconciled_at,reconcile_attempted_at,reconcile_error FROM notification_settings WHERE id=1").fetch_one(&mut **tx).await?;
    let reconciliation = reconciliation_view(&status_row, now);
    for v in &mut values {
        v["reconciliation_state"] = reconciliation["state"].clone();
        v["notification"] = latest
            .get(v["id"].as_str().unwrap_or(""))
            .cloned()
            .unwrap_or(Value::Null);
        if v["notification_intent"].is_object() {
            let intent = &v["notification_intent"];
            v["notification"] = json!({"state":intent["state"],"attempts":0,"provider_status":null,"reason":intent["reason"],"updated_at":intent["updated_at"]});
        }
    }

    Ok((
        json!(values),
        json!({"matched":matched,"truncated":false,"maximum_episodes":MAX_EPISODES,"reconciliation":reconciliation}),
    ))
}
async fn reconciliation(state: &AppState, now: i64) -> ApiResult<Value> {
    let row=sqlx::query("SELECT last_reconciled_at,reconcile_attempted_at,reconcile_error FROM notification_settings WHERE id=1").fetch_one(&state.db).await?;
    Ok(reconciliation_view(&row, now))
}
fn reconciliation_view(row: &SqliteRow, now: i64) -> Value {
    let successful: Option<i64> = row.get("last_reconciled_at");
    let attempted: Option<i64> = row.get("reconcile_attempted_at");
    let error: Option<String> = row.get("reconcile_error");
    let state = if successful.is_none() {
        "unknown"
    } else if error.is_some() {
        "failed"
    } else if successful.is_some_and(|last| now.saturating_sub(last) > 15_000 || last > now) {
        "stale"
    } else {
        "current"
    };
    json!({"state":state,"last_successful_at":successful.map(store::timestamp),"last_attempted_at":attempted.map(store::timestamp),"error":error,"stale_after_seconds":15})
}
pub async fn summary(state: &AppState, now: i64) -> ApiResult<Value> {
    let counts=sqlx::query("SELECT COUNT(*) AS total,SUM(CASE WHEN status='open' THEN 1 ELSE 0 END) AS open,SUM(CASE WHEN status='open' AND ack_json IS NULL THEN 1 ELSE 0 END) AS unacknowledged FROM attention_episodes").fetch_one(&state.db).await?;
    let mut delivery = serde_json::Map::new();
    for row in sqlx::query("SELECT state,COUNT(*) AS n FROM notification_outbox GROUP BY state")
        .fetch_all(&state.db)
        .await?
    {
        delivery.insert(row.get("state"), json!(row.get::<i64, _>("n")));
    }
    let pending:i64=sqlx::query_scalar("SELECT COUNT(*) FROM attention_episodes WHERE json_extract(notification_intent_json,'$.state')='pending_admission'").fetch_one(&state.db).await?;
    delivery.insert("pending_admission".into(), json!(pending));
    Ok(
        json!({"total":counts.get::<i64,_>("total"),"open":counts.get::<Option<i64>,_>("open").unwrap_or(0),"unacknowledged":counts.get::<Option<i64>,_>("unacknowledged").unwrap_or(0),"notifications":crate::notifications::settings(state).await?,"delivery":delivery,"reconciliation":reconciliation(state,now).await?,"observed_at":store::timestamp(now)}),
    )
}
pub async fn reconcile(state: &AppState, now: i64) -> ApiResult<()> {
    let capacities = crate::read_api::attention_capacity_inputs(state).await?;
    let diagnostics = crate::diagnostics::attention_conditions(state, now).await?;
    let _guard = state.writer.lock().await;
    let mut tx = state.db.begin().await?;
    sqlx::query("DELETE FROM notification_outbox WHERE state NOT IN ('queued','sending','accepted') AND updated_at<?").bind(now-RETENTION_MS).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM attention_episodes WHERE status='resolved' AND resolved_at<? AND NOT EXISTS(SELECT 1 FROM notification_outbox WHERE episode_id=attention_episodes.id)").bind(now-RETENTION_MS).execute(&mut *tx).await?;
    sqlx::query("UPDATE notification_settings SET admission_limited=0 WHERE id=1")
        .execute(&mut *tx)
        .await?;
    // Reserve freed slots for oldest pending intents before new source transitions.
    crate::notifications::retry_pending(&mut tx, now).await?;
    let mut observed = BTreeSet::new();
    for table in ["detection_findings", "reliability_findings"] {
        let sources = if table == "detection_findings" {
            crate::detection_store::source_summaries_filtered(&mut tx, now, None, None).await?
        } else {
            crate::reliability_store::source_summaries_filtered(&mut tx, now, None, None).await?
        };
        let sources: BTreeMap<String, Value> = sources
            .into_iter()
            .filter_map(|v| v["source_id"].as_str().map(str::to_owned).map(|id| (id, v)))
            .collect();
        let rows = sqlx::query(&format!(
            "SELECT * FROM {table} ORDER BY updated_at DESC LIMIT 10000"
        ))
        .fetch_all(&mut *tx)
        .await?;
        if rows.len() == 10000 {
            sqlx::query("UPDATE notification_settings SET admission_limited=1 WHERE id=1")
                .execute(&mut *tx)
                .await?;
        }
        for row in rows {
            observed.insert(format!("{table}:{}", row.get::<String, _>("finding_id")));
            let mut finding: Value = serde_json::from_str(row.get("finding_json"))
                .map_err(|_| ApiError::conflict("Invalid finding evidence"))?;
            let status = row.get::<String, _>("status");
            let source = sources.get(&row.get::<String, _>("source_id"));
            let observation = source.and_then(|source| {
                source["signals"]
                    .as_array()
                    .and_then(|signals| {
                        signals
                            .iter()
                            .find(|v| v["rule_id"] == finding["rule_id"])
                            .map(|v| &v["observation"])
                    })
                    .or_else(|| source.get("observation"))
            });
            let observed = if status == "resolved" {
                "ok"
            } else if status == "interrupted" {
                "unknown"
            } else {
                match observation.and_then(|v| v["state"].as_str()) {
                    Some("current") => "ok",
                    Some(v) => v,
                    None => "unknown",
                }
            }
            .to_owned();
            if let Some(observation) = observation {
                finding["source_observation"] = observation.clone();
            }
            observe_condition(
                &mut tx,
                &Condition {
                    key: format!("{table}:{}", row.get::<String, _>("finding_id")),
                    kind: if table == "detection_findings" {
                        "activity"
                    } else {
                        "reliability"
                    }
                    .into(),
                    node_id: row.get("node_id"),
                    object_id: Some(row.get("object_id")),
                    observation_state: observed,
                    status,
                    severity: finding["severity"].as_str().unwrap_or("warning").into(),
                    summary: finding["summary"]
                        .as_str()
                        .unwrap_or("Storage finding")
                        .into(),
                    evidence: finding,
                },
                now,
            )
            .await?;
        }
    }
    for row in
        sqlx::query("SELECT node_id,last_seen_at,goodbye_at,revoked_at FROM nodes LIMIT 10000")
            .fetch_all(&mut *tx)
            .await?
    {
        let id: String = row.get("node_id");
        observed.insert(format!("node_loss:{id}"));
        let seen: Option<i64> = row.get("last_seen_at");
        let goodbye: Option<i64> = row.get("goodbye_at");
        let revoked: Option<i64> = row.get("revoked_at");
        let future = seen.is_some_and(|s| s > now);
        let age = seen.filter(|s| *s <= now).map(|s| now.saturating_sub(s));
        let offline = goodbye.is_some() || revoked.is_some() || age.is_some_and(|a| a >= 90_000);
        let status = if future {
            "interrupted"
        } else if offline {
            "open"
        } else if age.is_some_and(|a| a < 30_000) {
            "resolved"
        } else {
            "interrupted"
        };
        observe_condition(&mut tx,&Condition{key:format!("node_loss:{id}"),kind:"node_loss".into(),node_id:id,object_id:None,status:status.into(),severity:"critical".into(),observation_state:if status=="interrupted"{"unknown"}else{"ok"}.into(),summary:if offline{"Node observation lost; storage condition unknown"}else{"Node observation restored"}.into(),evidence:json!({"last_seen_at":seen.map(store::timestamp),"age_ms":age,"goodbye":goodbye.is_some(),"revoked":revoked.is_some(),"offline_after_seconds":90})},now).await?;
    }
    for c in capacities {
        if let (Some(key), Some(node)) = (c["key"].as_str(), c["node_id"].as_str()) {
            observed.insert(key.to_owned());
            observe_capacity(
                &mut tx,
                key,
                node,
                c["object_id"].as_str(),
                &c["capacity"]["used_ratio"],
                now,
            )
            .await?;
        }
    }
    for c in diagnostics {
        observed.insert(c.key.clone());
        observe_condition(&mut tx, &c, now).await?;
    }
    for c in crate::device_watch::conditions(&mut tx,now).await? {
        observed.insert(c.key.clone());
        observe_condition(&mut tx,&c,now).await?;
    }
    // Explicit drive and mount removals are events, not gauges that become unknown when absent.
    for row in sqlx::query("SELECT id,source_key FROM attention_episodes WHERE status='open' AND kind!='drive_removal' AND source_key NOT LIKE 'mount_removal:%' AND observation_state!='unknown'").fetch_all(&mut *tx).await? {
        if !observed.contains(&row.get::<String,_>("source_key")) {
            sqlx::query("UPDATE attention_episodes SET observation_state='unknown',updated_at=? WHERE id=?").bind(now).bind(row.get::<String,_>("id")).execute(&mut *tx).await?;
        }
    }
    crate::notifications::retry_pending(&mut tx, now).await?;
    sqlx::query("UPDATE notification_settings SET admission_limited=1 WHERE id=1 AND ((SELECT COUNT(*) FROM attention_episodes)>=10000 OR (SELECT COUNT(*) FROM notification_outbox WHERE state IN ('queued','sending','accepted'))>=1000)").execute(&mut *tx).await?;
    sqlx::query("UPDATE notification_settings SET last_reconciled_at=?,reconcile_attempted_at=?,reconcile_error=NULL WHERE id=1").bind(now).bind(now).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}

/// Record an unsuccessful worker attempt without disclosing the underlying error.
pub async fn record_reconcile_failure(state: &AppState, now: i64) -> ApiResult<()> {
    let _guard = state.writer.lock().await;
    sqlx::query("UPDATE notification_settings SET reconcile_attempted_at=?,reconcile_error='reconciliation_failed' WHERE id=1").bind(now).execute(&state.db).await?;
    Ok(())
}

#[cfg(test)]
mod snapshot_tests {
    use super::*;
    #[tokio::test]
    async fn bounded_attention_selection_uses_one_snapshot_across_concurrent_write() {
        let d = tempfile::tempdir().unwrap();
        let state = AppState::open(&d.path().join("test.db"), "admin")
            .await
            .unwrap();
        let mut tx = state.db.begin().await.unwrap();
        observe_condition(
            &mut tx,
            &Condition {
                key: "one".into(),
                kind: "capacity".into(),
                node_id: "n".into(),
                object_id: None,
                status: "open".into(),
                severity: "warning".into(),
                observation_state: "ok".into(),
                summary: "warning".into(),
                evidence: json!({"small":true}),
            },
            1000,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        let mut read = state.db.begin().await.unwrap();
        let bytes: i64 =
            sqlx::query_scalar("SELECT SUM(length(evidence_json)) FROM attention_episodes")
                .fetch_one(&mut *read)
                .await
                .unwrap();
        assert!(bytes < 100);
        // A different connection commits a selection larger than the read budget.
        let large = json!({"payload":"x".repeat(32*1024*1024)}).to_string();
        sqlx::query("UPDATE attention_episodes SET evidence_json=?")
            .bind(large)
            .execute(&state.db)
            .await
            .unwrap();
        let (rows, meta) = list_snapshot(&mut read, &Default::default(), 2000)
            .await
            .unwrap();
        assert_eq!(meta["matched"], 1);
        assert_eq!(rows[0]["evidence"], json!({"small":true}));
        read.commit().await.unwrap();
        let error = list(&state, &Default::default(), 2000).await.unwrap_err();
        assert_eq!(error.status, StatusCode::SERVICE_UNAVAILABLE);
    }
}
