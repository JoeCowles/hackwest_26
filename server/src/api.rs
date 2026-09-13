use crate::{
    HEARTBEAT_SECONDS,
    error::{ApiError, ApiResult},
    model::*,
    store::{self, AppState},
};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, State, rejection::JsonRejection},
    http::{HeaderMap, HeaderValue, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, post, put},
};
use chrono::{DateTime, Utc};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use sqlx::{Row, Sqlite, Transaction};
use std::collections::HashMap;
use subtle::ConstantTimeEq;
use uuid::Uuid;

type Body = Result<Json<Value>, JsonRejection>;

/// Section 14 read/monitoring endpoints are deliberately not registered yet.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/v1/enrollment-tokens", post(create_token))
        .route("/api/v1/nodes/enroll", post(enroll))
        .route("/api/v1/nodes/{node_id}/inventory", put(inventory))
        .route("/api/v1/nodes/{node_id}/telemetry", post(telemetry))
        .route("/api/v1/nodes/{node_id}/heartbeat", post(heartbeat))
        .route("/api/v1/nodes/{node_id}/goodbye", post(goodbye))
        .route("/api/v1/nodes/{node_id}/credential", delete(revoke))
        .fallback(|| async {
            ApiError::new(
                StatusCode::NOT_FOUND,
                "not_found",
                "Route is not implemented",
            )
        })
        .method_not_allowed_fallback(|| async {
            ApiError::new(
                StatusCode::METHOD_NOT_ALLOWED,
                "method_not_allowed",
                "Method is not supported",
            )
        })
        .layer(DefaultBodyLimit::max(crate::MAX_BODY_BYTES))
        .layer(middleware::from_fn_with_state(state.clone(), backpressure))
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .with_state(state)
}

async fn backpressure(
    State(state): State<AppState>,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    let Ok(_permit) = state.permits.clone().try_acquire_owned() else {
        return ApiError::unavailable().into_response();
    };
    next.run(request).await
}

fn parse<T: DeserializeOwned>(body: Body) -> ApiResult<(T, Value)> {
    let Json(value) = body.map_err(|error| {
        if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
            ApiError::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "payload_too_large",
                "Maximum JSON body size is 1048576 bytes",
            )
        } else {
            ApiError::field("body", "Expected a valid application/json request body")
        }
    })?;
    let parsed = serde_path_to_error::deserialize(value.clone())
        .map_err(|error| ApiError::field(error.path().to_string(), error.inner().to_string()))?;
    Ok((parsed, value))
}

fn unauthorized() -> ApiError {
    ApiError::new(
        StatusCode::UNAUTHORIZED,
        "unauthenticated",
        "A valid credential is required",
    )
}

enum Role<'a> {
    Admin,
    Node(&'a str),
    Enrollment(&'a str),
}

/// Check identity and atomically consume a replay ID in the mutation transaction.
async fn authorize(
    state: &AppState,
    tx: &mut Transaction<'_, Sqlite>,
    headers: &HeaderMap,
    role: Role<'_>,
) -> ApiResult<String> {
    let hash = match role {
        Role::Enrollment(token) => {
            let hash = store::fingerprint(token.as_bytes());
            let available: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM enrollment_tokens WHERE token_hash=? AND used_at IS NULL AND expires_at>?")
                .bind(&hash).bind(Utc::now().timestamp_millis()).fetch_one(&mut **tx).await?;
            if available != 1 {
                return Err(unauthorized());
            }
            hash
        }
        Role::Admin | Role::Node(_) => {
            let token = headers
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.strip_prefix("Bearer "))
                .filter(|s| !s.is_empty())
                .ok_or_else(unauthorized)?;
            let hash = store::fingerprint(token.as_bytes());
            let expected = match role {
                Role::Admin => state.admin_hash.clone(),
                Role::Node(node) => sqlx::query_scalar::<_, String>(
                    "SELECT credential_hash FROM nodes WHERE node_id=? AND revoked_at IS NULL",
                )
                .bind(node)
                .fetch_optional(&mut **tx)
                .await?
                .ok_or_else(unauthorized)?,
                _ => unreachable!(),
            };
            if !bool::from(hash.as_bytes().ct_eq(expected.as_bytes())) {
                return Err(unauthorized());
            }
            hash
        }
    };
    if let Role::Node(node) = role {
        let native: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM cider_nodes WHERE node_id=?")
            .bind(node).fetch_one(&mut **tx).await?;
        if native != 0 {
            return Err(ApiError::conflict("This node uses ciderd ingestion; do not mix ingestion protocols"));
        }
    }
    let request_id = headers
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| Uuid::parse_str(v).ok())
        .ok_or_else(|| ApiError::field("X-Request-ID", "A fresh UUID is required"))?
        .to_string();
    let time = headers
        .get("x-request-timestamp")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| DateTime::parse_from_rfc3339(v).ok())
        .ok_or_else(|| {
            ApiError::field("X-Request-Timestamp", "An RFC3339 timestamp is required")
        })?;
    if (Utc::now().timestamp_millis() - time.timestamp_millis()).abs() > 300_000 {
        return Err(ApiError::field(
            "X-Request-Timestamp",
            "Timestamp must be within 300 seconds of server time",
        ));
    }
    let result = sqlx::query("INSERT OR IGNORE INTO request_replays(credential_hash,request_id,expires_at) VALUES (?,?,?)")
        .bind(hash).bind(&request_id).bind(Utc::now().timestamp_millis() + 600_000).execute(&mut **tx).await?;
    if result.rows_affected() == 0 {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "request_replayed",
            "Use a fresh request ID for this transport attempt",
        ));
    }
    Ok(request_id)
}

fn success(status: StatusCode, data: Value, request_id: String, change_id: i64) -> Response {
    let mut response = (
        status,
        Json(json!({"data":data,"meta":{
            "api_version":"1","server_time":store::timestamp(Utc::now().timestamp_millis()),
            "request_id":request_id,"snapshot_cursor":change_id.to_string(),"next_cursor":null
        }})),
    )
        .into_response();
    response.headers_mut().insert(
        "x-request-id",
        HeaderValue::from_str(&request_id).expect("UUID is a valid header"),
    );
    response
        .headers_mut()
        .insert("cache-control", HeaderValue::from_static("no-store"));
    response
}

async fn create_token(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Body,
) -> ApiResult<Response> {
    let (request, _): (TokenRequest, _) = parse(body)?;
    let _guard = state.writer.lock().await;
    let mut tx = state.db.begin().await?;
    let request_id = authorize(&state, &mut tx, &headers, Role::Admin).await?;
    let token = store::issue_token(&mut tx, request.expires_in_seconds).await?;
    // Never publish the enrollment token in the change log.
    let cursor = store::change(&mut tx, "enrollment.created", None, json!({})).await?;
    tx.commit().await?;
    Ok(success(StatusCode::CREATED, token, request_id, cursor))
}

async fn enroll(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Body,
) -> ApiResult<Response> {
    let (request, _): (EnrollmentRequest, _) = parse(body)?;
    bounded("name", &request.name, 128)?;
    validate_agent(&request.agent)?;
    let _guard = state.writer.lock().await;
    let mut tx = state.db.begin().await?;
    let request_id = authorize(
        &state,
        &mut tx,
        &headers,
        Role::Enrollment(&request.enrollment_token),
    )
    .await?;
    let now = Utc::now().timestamp_millis();
    sqlx::query("UPDATE enrollment_tokens SET used_at=? WHERE token_hash=?")
        .bind(now)
        .bind(store::fingerprint(request.enrollment_token.as_bytes()))
        .execute(&mut *tx)
        .await?;
    let node_id = Uuid::new_v4().to_string();
    let credential = store::secret("node");
    sqlx::query(
        "INSERT INTO nodes(node_id,name,agent_json,credential_hash,enrolled_at) VALUES (?,?,?,?,?)",
    )
    .bind(&node_id)
    .bind(request.name)
    .bind(serde_json::to_string(&request.agent).unwrap())
    .bind(store::fingerprint(credential.as_bytes()))
    .bind(now)
    .execute(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO objects(object_id,node_id,local_id,kind,parents_json,properties_json,generation) VALUES (?,?,'node','node','[]','{}',0)")
        .bind(&node_id).bind(&node_id).execute(&mut *tx).await?;
    let cursor = store::change(
        &mut tx,
        "node.enrolled",
        Some(&node_id),
        json!({"node_id":node_id}),
    )
    .await?;
    tx.commit().await?;
    Ok(success(
        StatusCode::CREATED,
        json!({"node_id":node_id,"node_object_id":node_id,
        "credential":credential,"heartbeat_interval_seconds":HEARTBEAT_SECONDS,"inventory_generation":"0"}),
        request_id,
        cursor,
    ))
}

fn object_mapping(node_id: &str, inventory: &Inventory) -> Vec<Value> {
    let namespace = Uuid::parse_str(node_id).expect("server-issued node UUID");
    let mut objects = vec![json!({"local_id":"node","object_id":node_id})];
    objects.extend(inventory.objects.iter().map(|o| json!({"local_id":o.local_id,"object_id":Uuid::new_v5(&namespace,o.local_id.as_bytes()).to_string()})));
    objects
}

async fn inventory(
    State(state): State<AppState>,
    Path(node_id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> ApiResult<Response> {
    let (inventory, raw): (Inventory, _) = parse(body)?;
    validate_inventory(&inventory)?;
    validate_privacy(&raw, "body")?;
    let _guard = state.writer.lock().await;
    let mut tx = state.db.begin().await?;
    let request_id = authorize(&state, &mut tx, &headers, Role::Node(&node_id)).await?;
    let node = store::node_row(&mut tx, &node_id).await?;
    let current: i64 = node.get("inventory_generation");
    let generation = inventory.generation as i64;
    if generation < current {
        return Err(ApiError::conflict(format!(
            "Inventory generation is older than current generation {current}"
        )));
    }
    let fingerprint = store::fingerprint(raw.to_string().as_bytes());
    let mapping = object_mapping(&node_id, &inventory);
    if generation == current {
        let old = sqlx::query("SELECT fingerprint,change_id FROM inventory_generations WHERE node_id=? AND generation=?")
            .bind(&node_id).bind(generation).fetch_one(&mut *tx).await?;
        if old.get::<String, _>("fingerprint") != fingerprint {
            return Err(ApiError::conflict(
                "Generation already exists with different content",
            ));
        }
        tx.commit().await?;
        return Ok(success(
            StatusCode::OK,
            json!({"inventory_generation":generation.to_string(),"duplicate":true,"objects":mapping}),
            request_id,
            old.get("change_id"),
        ));
    }
    let ids: HashMap<String, String> = mapping
        .iter()
        .map(|v| {
            (
                v["local_id"].as_str().unwrap().to_owned(),
                v["object_id"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    sqlx::query("UPDATE objects SET active=0 WHERE node_id=? AND kind!='node'")
        .bind(&node_id)
        .execute(&mut *tx)
        .await?;
    for object in &inventory.objects {
        let parents: Vec<&str> = if object.parents.is_empty() {
            vec![&node_id]
        } else {
            object.parents.iter().map(|p| ids[p].as_str()).collect()
        };
        sqlx::query("INSERT INTO objects(object_id,node_id,local_id,kind,parents_json,properties_json,generation,active) VALUES (?,?,?,?,?,?,?,1) ON CONFLICT(object_id) DO UPDATE SET kind=excluded.kind,parents_json=excluded.parents_json,properties_json=excluded.properties_json,generation=excluded.generation,active=1")
            .bind(&ids[&object.local_id]).bind(&node_id).bind(&object.local_id).bind(&object.kind)
            .bind(serde_json::to_string(&parents).unwrap()).bind(serde_json::to_string(&object.properties).unwrap())
            .bind(generation).execute(&mut *tx).await?;
    }
    sqlx::query("UPDATE objects SET generation=? WHERE object_id=?")
        .bind(generation)
        .bind(&node_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE nodes SET inventory_generation=? WHERE node_id=?")
        .bind(generation)
        .bind(&node_id)
        .execute(&mut *tx)
        .await?;
    let cursor = store::change(
        &mut tx,
        "inventory.updated",
        Some(&node_id),
        json!({"inventory_generation":generation.to_string()}),
    )
    .await?;
    sqlx::query("INSERT INTO inventory_generations(node_id,generation,fingerprint,inventory_json,received_at,change_id) VALUES (?,?,?,?,?,?)")
        .bind(&node_id).bind(generation).bind(fingerprint).bind(raw.to_string()).bind(Utc::now().timestamp_millis()).bind(cursor).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(success(
        StatusCode::OK,
        json!({"inventory_generation":generation.to_string(),"duplicate":false,"objects":mapping}),
        request_id,
        cursor,
    ))
}

async fn telemetry(
    State(state): State<AppState>,
    Path(node_id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> ApiResult<Response> {
    let (batch, raw): (Telemetry, _) = parse(body)?;
    validate_telemetry(&batch)?;
    validate_privacy(&raw, "body")?;
    let _guard = state.writer.lock().await;
    let mut tx = state.db.begin().await?;
    let request_id = authorize(&state, &mut tx, &headers, Role::Node(&node_id)).await?;
    if batch.node_id != node_id {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "forbidden",
            "Payload node_id must match the credential and route",
        ));
    }
    let fingerprint = store::fingerprint(raw.to_string().as_bytes());
    let sequence = batch.sequence.to_string();
    if let Some(old) =
        sqlx::query("SELECT * FROM batches WHERE node_id=? AND boot_id=? AND sequence=?")
            .bind(&node_id)
            .bind(&batch.boot_id)
            .bind(&sequence)
            .fetch_optional(&mut *tx)
            .await?
    {
        if old.get::<String, _>("fingerprint") != fingerprint {
            return Err(ApiError::conflict(
                "Batch identity already exists with different content",
            ));
        }
        tx.commit().await?;
        return Ok(success(
            StatusCode::ACCEPTED,
            json!({"accepted_sequence":sequence,"duplicate":true,
            "stored_samples":old.get::<i64,_>("sample_count"),"stored_events":old.get::<i64,_>("event_count"),
            "received_at":store::timestamp(old.get("received_at")),"heartbeat_interval_seconds":HEARTBEAT_SECONDS}),
            request_id,
            old.get("change_id"),
        ));
    }
    let node = store::node_row(&mut tx, &node_id).await?;
    if batch.inventory_generation as i64 != node.get::<i64, _>("inventory_generation") {
        return Err(ApiError::conflict(
            "Telemetry generation must match current inventory",
        ));
    }
    let rows = sqlx::query("SELECT object_id,kind FROM objects WHERE node_id=? AND active=1")
        .bind(&node_id)
        .fetch_all(&mut *tx)
        .await?;
    let objects: HashMap<String, String> = rows
        .into_iter()
        .map(|r| (r.get("object_id"), r.get("kind")))
        .collect();
    if batch
        .samples
        .iter()
        .any(|s| !objects.contains_key(&s.object_id))
        || batch.events.iter().any(|e| {
            e.object_id
                .as_ref()
                .is_some_and(|o| !objects.contains_key(o))
        })
    {
        return Err(ApiError::conflict(
            "Every object_id must belong to this node's active inventory",
        ));
    }
    let now = Utc::now().timestamp_millis();
    let mut count = 0;
    let current_boot = node.get::<Option<String>, _>("boot_id");
    let is_current_boot = current_boot.as_deref().is_none_or(|id| id == batch.boot_id)
        || batch.observed_at.timestamp_millis() >= node.get::<i64, _>("boot_observed_at");
    let mut ordered_samples: Vec<_> = batch.samples.iter().collect();
    ordered_samples.sort_by_key(|sample| sample.observed_at);
    for sample in ordered_samples {
        let scope = sample
            .scope
            .as_deref()
            .unwrap_or_else(|| store::default_scope(&objects[&sample.object_id]));
        let labels = serde_json::to_string(&sample.labels).unwrap();
        let at = sample.observed_at.timestamp_millis();
        let overlap = sqlx::query("SELECT value_json,state FROM metric_samples WHERE object_id=? AND boot_id=? AND generation=? AND name=? AND source=? AND scope=? AND labels_json=? AND observed_at=?")
            .bind(&sample.object_id).bind(&batch.boot_id).bind(batch.inventory_generation as i64).bind(&sample.name)
            .bind(&sample.source).bind(scope).bind(&labels).bind(at).fetch_optional(&mut *tx).await?;
        if let Some(old) = overlap {
            if old.get::<String, _>("value_json") != sample.value.to_string()
                || old.get::<String, _>("state") != sample.state
            {
                return Err(ApiError::conflict(
                    "Series timestamp already exists with a different value or state",
                ));
            }
            continue;
        }
        let previous = sqlx::query("SELECT name,state,value_json,observed_at FROM metric_samples WHERE object_id=? AND boot_id=? AND generation=? AND name=? AND source=? AND scope=? AND labels_json=? AND observed_at<? ORDER BY observed_at DESC LIMIT 1")
            .bind(&sample.object_id).bind(&batch.boot_id).bind(batch.inventory_generation as i64).bind(&sample.name)
            .bind(&sample.source).bind(scope).bind(&labels).bind(at).fetch_optional(&mut *tx).await?;
        let (rate, derivation) = if sample.kind == "counter" {
            store::counter_rate(&sample.value, &sample.state, at, previous.as_ref())
        } else {
            (None, "unavailable")
        };
        let numeric = if sample.state == "ok" {
            sample
                .value
                .as_f64()
                .or_else(|| counter(&sample.value).map(|n| n as f64))
        } else {
            None
        };
        sqlx::query("INSERT INTO metric_samples(node_id,object_id,boot_id,generation,name,kind,unit,state,source,scope,labels_json,value_json,numeric_value,observed_at,received_at,rate_per_second,derivation_state) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)")
            .bind(&node_id).bind(&sample.object_id).bind(&batch.boot_id).bind(batch.inventory_generation as i64)
            .bind(&sample.name).bind(&sample.kind).bind(&sample.unit).bind(&sample.state).bind(&sample.source)
            .bind(scope).bind(&labels).bind(sample.value.to_string()).bind(numeric).bind(at).bind(now).bind(rate).bind(derivation)
            .execute(&mut *tx).await?;
        let latest = json!({"sample":sample,"scope":scope,"boot_id":batch.boot_id,"inventory_generation":batch.inventory_generation.to_string(),
            "received_at":store::timestamp(now),"derived_rate_per_second":rate,"derivation_state":derivation});
        if is_current_boot {
            sqlx::query("INSERT INTO latest_samples(object_id,name,source,scope,labels_json,sample_json,observed_at,received_at) VALUES (?,?,?,?,?,?,?,?) ON CONFLICT(object_id,name,source,scope,labels_json) DO UPDATE SET sample_json=excluded.sample_json,observed_at=excluded.observed_at,received_at=excluded.received_at WHERE excluded.observed_at>latest_samples.observed_at OR json_extract(excluded.sample_json,'$.boot_id')!=json_extract(latest_samples.sample_json,'$.boot_id') OR json_extract(excluded.sample_json,'$.inventory_generation')!=json_extract(latest_samples.sample_json,'$.inventory_generation')")
            .bind(&sample.object_id).bind(&sample.name).bind(&sample.source).bind(scope).bind(&labels).bind(latest.to_string()).bind(at).bind(now)
            .execute(&mut *tx).await?;
        }
        // Late observations also change the immediately following counter interval.
        if sample.kind == "counter" {
            let next = sqlx::query("SELECT sample_id,name,state,value_json,observed_at FROM metric_samples WHERE object_id=? AND boot_id=? AND generation=? AND name=? AND source=? AND scope=? AND labels_json=? AND observed_at>? ORDER BY observed_at LIMIT 1")
                .bind(&sample.object_id).bind(&batch.boot_id).bind(batch.inventory_generation as i64).bind(&sample.name)
                .bind(&sample.source).bind(scope).bind(&labels).bind(at).fetch_optional(&mut *tx).await?;
            if let Some(next) = next {
                let inserted = sqlx::query("SELECT name,state,value_json,observed_at FROM metric_samples WHERE object_id=? AND boot_id=? AND generation=? AND name=? AND source=? AND scope=? AND labels_json=? AND observed_at=?")
                    .bind(&sample.object_id).bind(&batch.boot_id).bind(batch.inventory_generation as i64).bind(&sample.name)
                    .bind(&sample.source).bind(scope).bind(&labels).bind(at).fetch_one(&mut *tx).await?;
                let next_value: Value = serde_json::from_str(&next.get::<String, _>("value_json"))
                    .expect("stored JSON value");
                let next_at: i64 = next.get("observed_at");
                let (rate, derivation) = store::counter_rate(
                    &next_value,
                    &next.get::<String, _>("state"),
                    next_at,
                    Some(&inserted),
                );
                sqlx::query("UPDATE metric_samples SET rate_per_second=?,derivation_state=? WHERE sample_id=?")
                    .bind(rate).bind(derivation).bind(next.get::<i64,_>("sample_id")).execute(&mut *tx).await?;
                sqlx::query("UPDATE latest_samples SET sample_json=json_set(sample_json,'$.derived_rate_per_second',?,'$.derivation_state',?) WHERE object_id=? AND name=? AND source=? AND scope=? AND labels_json=? AND observed_at=? AND json_extract(sample_json,'$.boot_id')=? AND json_extract(sample_json,'$.inventory_generation')=?")
                    .bind(rate).bind(derivation).bind(&sample.object_id).bind(&sample.name).bind(&sample.source).bind(scope).bind(&labels)
                    .bind(next_at).bind(&batch.boot_id).bind(batch.inventory_generation.to_string()).execute(&mut *tx).await?;
            }
        }
        count += 1;
    }
    for event in &batch.events {
        sqlx::query("INSERT INTO events(node_id,object_id,event_json,occurred_at,received_at) VALUES (?,?,?,?,?)")
            .bind(&node_id).bind(&event.object_id).bind(serde_json::to_string(event).unwrap())
            .bind(event.occurred_at.timestamp_millis()).bind(now).execute(&mut *tx).await?;
    }
    sqlx::query("UPDATE nodes SET last_seen_at=?,goodbye_at=NULL,agent_json=CASE WHEN ? THEN ? ELSE agent_json END,boot_id=CASE WHEN ?>=boot_observed_at THEN ? ELSE boot_id END,boot_observed_at=MAX(boot_observed_at,?) WHERE node_id=?")
        .bind(now).bind(is_current_boot).bind(serde_json::to_string(&batch.agent).unwrap()).bind(batch.observed_at.timestamp_millis())
        .bind(&batch.boot_id).bind(batch.observed_at.timestamp_millis()).bind(&node_id).execute(&mut *tx).await?;
    let cursor = store::change(
        &mut tx,
        "telemetry.accepted",
        Some(&node_id),
        json!({"node_id":node_id,"stored_samples":count,"stored_events":batch.events.len()}),
    )
    .await?;
    sqlx::query("INSERT INTO batches(node_id,boot_id,sequence,fingerprint,raw_json,received_at,sample_count,event_count,change_id) VALUES (?,?,?,?,?,?,?,?,?)")
        .bind(&node_id).bind(&batch.boot_id).bind(&sequence).bind(fingerprint).bind(raw.to_string())
        .bind(now).bind(count).bind(batch.events.len() as i64).bind(cursor).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(success(
        StatusCode::ACCEPTED,
        json!({"accepted_sequence":sequence,"duplicate":false,"stored_samples":count,
        "stored_events":batch.events.len(),"received_at":store::timestamp(now),"heartbeat_interval_seconds":HEARTBEAT_SECONDS}),
        request_id,
        cursor,
    ))
}

async fn heartbeat(
    State(state): State<AppState>,
    Path(node_id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> ApiResult<Response> {
    let (heartbeat, _): (Heartbeat, _) = parse(body)?;
    bounded("boot_id", &heartbeat.boot_id, 128)?;
    validate_agent(&heartbeat.agent)?;
    let _guard = state.writer.lock().await;
    let mut tx = state.db.begin().await?;
    let request_id = authorize(&state, &mut tx, &headers, Role::Node(&node_id)).await?;
    let now = Utc::now().timestamp_millis();
    sqlx::query("UPDATE nodes SET last_seen_at=?,goodbye_at=NULL,boot_id=?,boot_observed_at=?,agent_json=? WHERE node_id=?")
        .bind(now).bind(&heartbeat.boot_id).bind(now).bind(serde_json::to_string(&heartbeat.agent).unwrap()).bind(&node_id).execute(&mut *tx).await?;
    let cursor = store::change(
        &mut tx,
        "node.heartbeat",
        Some(&node_id),
        json!({"node_id":node_id}),
    )
    .await?;
    tx.commit().await?;
    Ok(success(
        StatusCode::OK,
        json!({"node_id":node_id,"last_seen_at":store::timestamp(now),"heartbeat_interval_seconds":HEARTBEAT_SECONDS}),
        request_id,
        cursor,
    ))
}

async fn goodbye(
    State(state): State<AppState>,
    Path(node_id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> ApiResult<Response> {
    let (goodbye, _): (Goodbye, _) = parse(body)?;
    bounded("boot_id", &goodbye.boot_id, 128)?;
    if let Some(reason) = &goodbye.reason {
        bounded("reason", reason, 256)?;
    }
    let _guard = state.writer.lock().await;
    let mut tx = state.db.begin().await?;
    let request_id = authorize(&state, &mut tx, &headers, Role::Node(&node_id)).await?;
    let node = store::node_row(&mut tx, &node_id).await?;
    if node.get::<Option<String>, _>("boot_id").as_deref() != Some(&goodbye.boot_id) {
        return Err(ApiError::conflict(
            "Goodbye boot_id must match the current heartbeat boot",
        ));
    }
    let now = Utc::now().timestamp_millis();
    sqlx::query("UPDATE nodes SET goodbye_at=? WHERE node_id=?")
        .bind(now)
        .bind(&node_id)
        .execute(&mut *tx)
        .await?;
    let cursor = store::change(
        &mut tx,
        "node.goodbye",
        Some(&node_id),
        json!({"node_id":node_id,"reason":goodbye.reason}),
    )
    .await?;
    tx.commit().await?;
    Ok(success(
        StatusCode::OK,
        json!({"node_id":node_id,"status":"offline"}),
        request_id,
        cursor,
    ))
}

async fn revoke(
    State(state): State<AppState>,
    Path(node_id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let _guard = state.writer.lock().await;
    let mut tx = state.db.begin().await?;
    let request_id = authorize(&state, &mut tx, &headers, Role::Admin).await?;
    let node = store::node_row(&mut tx, &node_id).await?;
    let revoked_at = node
        .get::<Option<i64>, _>("revoked_at")
        .unwrap_or_else(|| Utc::now().timestamp_millis());
    sqlx::query("UPDATE nodes SET revoked_at=? WHERE node_id=?")
        .bind(revoked_at)
        .bind(&node_id)
        .execute(&mut *tx)
        .await?;
    let cursor = store::change(
        &mut tx,
        "node.credential_revoked",
        Some(&node_id),
        json!({"node_id":node_id}),
    )
    .await?;
    tx.commit().await?;
    Ok(success(
        StatusCode::OK,
        json!({"node_id":node_id,"revoked_at":store::timestamp(revoked_at)}),
        request_id,
        cursor,
    ))
}
