//! Reads persisted observations without backfilling gaps or combining series.
use crate::{
    error::{ApiError, ApiResult},
    store::{self, AppState},
};
use axum::http::StatusCode;
use serde_json::{Value, json};
use sqlx::{Row, sqlite::SqliteRow};
use std::collections::{BTreeMap, BTreeSet};
const DAY: i64 = 86_400_000;
const MAX_POINTS: usize = 2000;
fn decode(s: &str) -> ApiResult<Value> {
    serde_json::from_str(s).map_err(|_| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            "Stored history is invalid",
        )
    })
}
fn instant(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|v| v.timestamp_millis())
}
fn too_large() -> ApiError {
    ApiError::new(
        StatusCode::BAD_REQUEST,
        "history_too_large",
        "Selection exceeds 2000 points; narrow the time window or choose a coarser resolution",
    )
}
fn identity(r: &SqliteRow, object: &str) -> ApiResult<Value> {
    Ok(
        json!({"object_id":object,"boot_id":r.get::<String,_>("boot_id"),"inventory_generation":r.get::<i64,_>("generation").to_string(),"metric":r.get::<String,_>("name"),"kind":r.get::<String,_>("kind"),"unit":r.get::<String,_>("unit"),"source":r.get::<String,_>("source"),"scope":r.get::<String,_>("scope"),"labels":decode(&r.get::<String,_>("labels_json"))?}),
    )
}
fn exact_value(r: &SqliteRow) -> ApiResult<Value> {
    let value = decode(&r.get::<String, _>("value_json"))?;
    if value.is_number()
        && (r.get::<String, _>("kind") == "counter" || r.get::<String, _>("unit") == "bytes")
    {
        return Ok(json!(value.to_string()));
    }
    Ok(value)
}
async fn exists(s: &AppState, object: &str) -> ApiResult<()> {
    if !sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM objects WHERE object_id=?)")
        .bind(object)
        .fetch_one(&s.db)
        .await?
    {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "not_found",
            "Object not found",
        ));
    }
    Ok(())
}
/// `from` and `to` are mandatory RFC3339 instants; endpoints are inclusive.
pub async fn read(
    s: &AppState,
    object: &str,
    q: &BTreeMap<String, String>,
    now: i64,
) -> ApiResult<(Value, Value)> {
    for key in q.keys() {
        if !["metric", "from", "to", "resolution"].contains(&key.as_str()) {
            return Err(ApiError::field(key, "Unknown history parameter"));
        }
    }
    let metric = q
        .get("metric")
        .filter(|m| {
            !m.is_empty()
                && m.len() <= 128
                && m.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.')
        })
        .ok_or_else(|| {
            ApiError::field(
                "metric",
                "Expected a stored metric name, at most 128 characters",
            )
        })?;
    let resolution = q.get("resolution").map(String::as_str).unwrap_or("raw");
    let (seconds, retention) = match resolution {
        "raw" => (0, DAY),
        "5m" => (300, 30 * DAY),
        "1h" => (3600, 365 * DAY),
        _ => return Err(ApiError::field("resolution", "Expected raw, 5m or 1h")),
    };
    let from = q
        .get("from")
        .and_then(|s| instant(s))
        .ok_or_else(|| ApiError::field("from", "Expected an RFC3339 date"))?;
    let to = q
        .get("to")
        .and_then(|s| instant(s))
        .ok_or_else(|| ApiError::field("to", "Expected an RFC3339 date"))?;
    if from >= to || to > now || from < now.saturating_sub(retention) {
        return Err(ApiError::field(
            "from",
            "Expected from < to <= now within the selected retention window",
        ));
    }
    exists(s, object).await?;
    let rows = if seconds == 0 {
        sqlx::query("SELECT * FROM metric_samples WHERE object_id=? AND name=? AND observed_at>=? AND observed_at<=? ORDER BY observed_at,sample_id LIMIT 2001").bind(object).bind(metric).bind(from).bind(to).fetch_all(&s.db).await?
    } else {
        sqlx::query("SELECT * FROM metric_rollups WHERE object_id=? AND name=? AND bucket_start>=? AND bucket_start<=? AND resolution_seconds=? ORDER BY bucket_start,boot_id,generation,source,scope,labels_json LIMIT 2001").bind(object).bind(metric).bind(from).bind(to).bind(seconds).fetch_all(&s.db).await?
    };
    if rows.len() > MAX_POINTS {
        return Err(too_large());
    }
    let mut series: BTreeMap<String, (Value, Vec<Value>, Option<i64>)> = BTreeMap::new();
    for r in &rows {
        let id = identity(r, object)?;
        let key = id.to_string();
        let at = r.get::<i64, _>(if seconds == 0 {
            "observed_at"
        } else {
            "bucket_start"
        });
        let entry = series.entry(key).or_insert((id, vec![], None));
        let elapsed = entry.2.map(|old| at - old);
        // Raw cadence is not persisted. Expose elapsed time and only flag a conservative
        // one-hour gap. Clients must also split on non-ok state and series boundaries.
        let gap = elapsed.is_some_and(|ms| {
            ms > if seconds == 0 {
                3_600_000
            } else {
                seconds * 1000
            }
        });
        let mut p = if seconds == 0 {
            json!({"observed_at":store::timestamp(at),"received_at":store::timestamp(r.get("received_at")),"value":exact_value(r)?,"state":r.get::<String,_>("state"),"derived_rate_per_second":r.get::<Option<f64>,_>("rate_per_second"),"derivation_state":r.get::<String,_>("derivation_state")})
        } else {
            let count = r.get::<i64, _>("sample_count");
            let valid = r.get::<i64, _>("valid_sample_count");
            let counts = decode(&r.get::<String, _>("state_counts_json"))?;
            let known: i64 = counts
                .as_object()
                .into_iter()
                .flat_map(|m| m.values())
                .filter_map(Value::as_i64)
                .sum();
            json!({"bucket_start":store::timestamp(at),"bucket_end":store::timestamp(at+seconds*1000),"min":r.get::<Option<f64>,_>("min"),"max":r.get::<Option<f64>,_>("max"),"mean":r.get::<Option<f64>,_>("mean"),"sample_count":count,"valid_sample_count":valid,"coverage":{"observed":valid,"expected":count},"state_counts":counts,"unclassified_state_count":(count-known).max(0),"state":if valid==0{"unavailable"}else if valid<count||counts["ok"].as_i64()!=Some(count){"partial"}else{"ok"}})
        };
        p["gap_before"] = json!(gap);
        p["elapsed_since_previous_seconds"] = json!(elapsed.map(|ms| ms as f64 / 1000.));
        entry.1.push(p);
        entry.2 = Some(at);
    }
    Ok((
        json!({"object_id":object,"metric":metric,"resolution":resolution,"from":store::timestamp(from),"to":store::timestamp(to),"series":series.into_values().map(|(identity,points,_)|json!({"identity":identity,"points":points})).collect::<Vec<_>>() }),
        json!({"selected_points":rows.len(),"max_points":MAX_POINTS,"retention_seconds":retention/1000,"backfilled":false,"raw_gap_threshold_seconds":if seconds==0{Some(3600)}else{None},"rollup_numeric_precision":"approximate_f64","rollup_counter_values":"rates_per_second","coverage_basis":"stored_samples_not_expected_cadence"}),
    ))
}
fn unknown(object: &str, reason: &str, basis: Value) -> Value {
    json!({"object_id":object,"state":"unknown","reasons":[reason],"estimated_exhaustion_at":null,"scenario_range":null,"basis":basis,"assumptions":["Capacity exhaustion only; no drive failure prediction","One storage object and one compatible observation series"]})
}
fn unsigned(r: &SqliteRow) -> Option<u128> {
    let v: Value = serde_json::from_str(&r.get::<String, _>("value_json")).ok()?;
    v.as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| v.to_string())
        .parse()
        .ok()
}
fn delta(v: u128, base: u128) -> f64 {
    if v >= base {
        (v - base) as f64
    } else {
        -((base - v) as f64)
    }
}
/// A conservative scenario using raw paired capacity observations in the last 24h.
pub async fn forecast(s: &AppState, object: &str, now: i64) -> ApiResult<Value> {
    let mut tx = s.db.begin().await?;
    let owner=sqlx::query("SELECT o.kind,o.active,o.generation,n.boot_id,n.inventory_generation,n.last_seen_at,n.goodbye_at,n.revoked_at FROM objects o JOIN nodes n ON n.node_id=o.node_id WHERE o.object_id=?").bind(object).fetch_optional(&mut *tx).await?.ok_or_else(||ApiError::new(StatusCode::NOT_FOUND,"not_found","Object not found"))?;
    if !["apfs_container", "apfs_volume", "mount", "nfs_mount"]
        .contains(&owner.get::<String, _>("kind").as_str())
    {
        return Ok(unknown(object, "ineligible_object", json!({})));
    }
    if owner.get::<i64, _>("active") != 1
        || owner.get::<Option<i64>, _>("revoked_at").is_some()
        || owner.get::<Option<i64>, _>("goodbye_at").is_some()
        || owner
            .get::<Option<i64>, _>("last_seen_at")
            .is_none_or(|at| at > now || now - at > 15000)
    {
        return Ok(unknown(object, "owner_unavailable", json!({})));
    }
    if owner.get::<i64, _>("generation") != owner.get::<i64, _>("inventory_generation") {
        return Ok(unknown(object, "identity_changed", json!({})));
    }
    let rows=sqlx::query("SELECT * FROM metric_samples WHERE object_id=? AND name IN ('used_bytes','capacity_bytes') AND observed_at>=? AND observed_at<=? ORDER BY observed_at,sample_id LIMIT 4001").bind(object).bind(now-DAY).bind(now).fetch_all(&mut *tx).await?;
    if rows.len() > 4000 {
        return Ok(unknown(
            object,
            "history_too_large",
            json!({"max_paired_points":2000}),
        ));
    }
    let mut series = BTreeSet::new();
    let mut pairs: BTreeMap<i64, (Option<u128>, Option<u128>)> = BTreeMap::new();
    for r in &rows {
        if r.get::<String, _>("boot_id")
            != owner
                .get::<Option<String>, _>("boot_id")
                .unwrap_or_default()
            || r.get::<i64, _>("generation") != owner.get::<i64, _>("generation")
        {
            return Ok(unknown(object, "identity_changed", json!({})));
        }
        if r.get::<String, _>("state") != "ok"
            || r.get::<String, _>("unit") != "bytes"
            || r.get::<String, _>("kind") != "gauge"
        {
            return Ok(unknown(object, "unusable_observation", json!({})));
        }
        let Some(value) = unsigned(r) else {
            return Ok(unknown(object, "invalid_capacity", json!({})));
        };
        let mut id = identity(r, object)?;
        id.as_object_mut().unwrap().remove("metric");
        series.insert(id.to_string());
        let pair = pairs.entry(r.get("observed_at")).or_default();
        let slot = if r.get::<String, _>("name") == "used_bytes" {
            &mut pair.0
        } else {
            &mut pair.1
        };
        if slot.replace(value).is_some() {
            return Ok(unknown(object, "ambiguous_series", json!({})));
        }
    }
    if series.len() > 1 {
        return Ok(unknown(object, "ambiguous_series", json!({})));
    }
    if pairs.len() < 8 {
        return Ok(unknown(
            object,
            "insufficient_data",
            json!({"sample_count":pairs.len(),"minimum_sample_count":8,"minimum_span_seconds":3600}),
        ));
    }
    if pairs
        .values()
        .any(|(used, total)| used.is_none() || total.is_none())
    {
        return Ok(unknown(object, "incompatible_observations", json!({})));
    }
    let points: Vec<_> = pairs
        .into_iter()
        .map(|(at, (used, total))| (at, used.unwrap(), total.unwrap()))
        .collect();
    let first = points[0];
    let last = *points.last().unwrap();
    let span = last.0 - first.0;
    let mut basis = json!({"sample_count":points.len(),"observed_from":store::timestamp(first.0),"observed_to":store::timestamp(last.0),"observation_span_seconds":span as f64/1000.,"capacity_bytes":last.2.to_string(),"used_bytes":last.1.to_string(),"series_identity":decode(series.first().unwrap())?});
    if span < 3_600_000 {
        return Ok(unknown(object, "insufficient_span", basis));
    }
    if points.windows(2).any(|p| p[1].0 - p[0].0 > 3_600_000) {
        return Ok(unknown(object, "history_gap", basis));
    }
    if points.iter().any(|p| p.2 != first.2) {
        return Ok(unknown(object, "capacity_changed", basis));
    }
    if first.2 == 0 || points.iter().any(|p| p.1 > p.2) {
        return Ok(unknown(object, "invalid_capacity", basis));
    }
    let last_rows: Vec<_> = rows
        .iter()
        .filter(|r| r.get::<i64, _>("observed_at") == last.0)
        .collect();
    for r in last_rows {
        let source = r.get::<String, _>("source");
        if source.starts_with("ciderd:") {
            let mut labels = decode(&r.get::<String, _>("labels_json"))?;
            let clock = labels["ciderd_clock_id"].clone();
            let epoch = labels["ciderd_counter_epoch"].clone();
            if let Some(m) = labels.as_object_mut() {
                m.remove("ciderd_clock_id");
                m.remove("ciderd_counter_epoch");
            }
            let latest=sqlx::query("SELECT sample_json,received_at,observed_at FROM latest_samples WHERE object_id=? AND name=? AND source=? AND scope=? AND labels_json=?").bind(object).bind(r.get::<String,_>("name")).bind(&source).bind(r.get::<String,_>("scope")).bind(labels.to_string()).fetch_optional(&mut *tx).await?;
            let Some(latest) = latest else {
                return Ok(unknown(object, "freshness_unavailable", basis));
            };
            let raw = decode(&latest.get::<String, _>("sample_json"))?;
            let age = raw["ciderd"]["age_at_receipt_seconds"]
                .as_f64()
                .unwrap_or(f64::INFINITY)
                + (now - latest.get::<i64, _>("received_at")).max(0) as f64 / 1000.;
            let policy = raw["ciderd"]["stale_after_seconds"].as_u64().unwrap_or(90) as f64;
            if age < 0.
                || age > policy
                || !age.is_finite()
                || latest.get::<i64, _>("received_at") > now
                || latest.get::<i64, _>("observed_at") != last.0
                || raw["sample"]["state"] != "ok"
                || raw["ciderd"]["source_metric"]["availability"] != "available"
                || raw["ciderd"]["source_metric"]["freshness"] != "live"
                || raw["ciderd"]["retained_after_failure"] == true
                || raw["ciderd"]["clock_id"] != clock
                || raw["ciderd"]["counter_epoch"] != epoch
                || raw["boot_id"] != json!(owner.get::<Option<String>, _>("boot_id"))
                || raw["ciderd"]["original_inventory_generation"]
                    != json!(owner.get::<i64, _>("generation").to_string())
            {
                return Ok(unknown(object, "stale_capacity", basis));
            }
        } else if now - last.0 > 90000 {
            return Ok(unknown(object, "stale_capacity", basis));
        }
    }
    tx.commit().await?;
    if last.1 >= last.2 {
        return Ok(unknown(object, "capacity_exhausted", basis));
    }
    let x: Vec<_> = points
        .iter()
        .map(|p| (p.0 - first.0) as f64 / 1000.)
        .collect();
    let y: Vec<_> = points.iter().map(|p| delta(p.1, first.1)).collect();
    let n = x.len() as f64;
    let mx = x.iter().sum::<f64>() / n;
    let my = y.iter().sum::<f64>() / n;
    let slope = x
        .iter()
        .zip(&y)
        .map(|(x, y)| (x - mx) * (y - my))
        .sum::<f64>()
        / x.iter().map(|x| (x - mx).powi(2)).sum::<f64>();
    if !slope.is_finite() || slope <= 0. || last.1 <= first.1 {
        return Ok(unknown(object, "no_reliable_growth", basis));
    }
    let residual = x
        .iter()
        .zip(&y)
        .map(|(x, y)| (y - (my + slope * (x - mx))).powi(2))
        .sum::<f64>();
    let variance = y.iter().map(|y| (y - my).powi(2)).sum::<f64>();
    let r2 = 1. - residual / variance;
    let slopes: Vec<_> = points
        .windows(2)
        .map(|p| delta(p[1].1, p[0].1) / ((p[1].0 - p[0].0) as f64 / 1000.))
        .collect();
    let min = slopes.iter().copied().fold(f64::INFINITY, f64::min);
    let max = slopes.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    if !r2.is_finite() || r2 < 0.9 || min <= 0. || max / min > 3. {
        return Ok(unknown(object, "unstable_growth", basis));
    }
    let remaining = (last.2 - last.1) as f64;
    let low = min.min(slope) * 0.75;
    let high = max.max(slope) * 1.25;
    let date = |rate: f64| -> Option<String> {
        let ms = remaining / rate * 1000.;
        if !ms.is_finite() || ms > 365. * DAY as f64 {
            return None;
        }
        let at = last.0.checked_add(ms.round() as i64)?;
        if at <= now {
            return None;
        }
        chrono::DateTime::from_timestamp_millis(at).map(|_| store::timestamp(at))
    };
    let (Some(estimate), Some(early), Some(late)) = (date(slope), date(high), date(low)) else {
        return Ok(unknown(object, "unreliable_horizon", basis));
    };
    basis["growth_bytes_per_second"] = json!(slope);
    basis["fit_r_squared"] = json!(r2);
    basis["remaining_bytes"] = json!((last.2 - last.1).to_string());
    basis["scenario_growth_bytes_per_second"] = json!({"low":low,"high":high});
    Ok(
        json!({"object_id":object,"state":"estimated","reasons":[],"estimated_exhaustion_at":estimate,"scenario_range":{"earliest_at":early,"latest_at":late},"basis":basis,"assumptions":["Capacity exhaustion only; no drive failure prediction","Capacity and recent positive growth continue unchanged","Scenario range uses observed interval growth with a 25 percent margin; it is not a statistical confidence interval","Raw observations cover at least one hour; no more than one hour separates samples","No allowance for workload changes, cleanup, snapshots or capacity additions"]}),
    )
}
