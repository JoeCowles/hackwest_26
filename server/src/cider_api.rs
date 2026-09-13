//! Adapter for ciderd's immutable schema-2 observations. No collectors run here.
use crate::{
    cider_wire::{self as wire, Collection, Heartbeat, Metric, Resource},
    error::{ApiError, ApiResult},
    store::{self, AppState},
};
use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode},
    routing::post,
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{Row, Sqlite, Transaction};
use std::collections::BTreeMap;
use subtle::ConstantTimeEq;
use uuid::Uuid;

#[derive(Default, Serialize, Deserialize)]
struct Graph {
    resources: BTreeMap<String, Resource>,
    relationships: BTreeMap<String, wire::Relationship>,
    versions: BTreeMap<String, Version>,
}

#[derive(Serialize, Deserialize)]
struct Version {
    generation: wire::Decimal,
    revision: wire::Decimal,
    fingerprint: String,
}

#[derive(Serialize, Deserialize)]
struct Receiver {
    generation: wire::Decimal,
    session: String,
    boot: String,
    sequence: wire::Decimal,
    inventory: Option<wire::Decimal>,
    projection_generation: i64,
    graph: Graph,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/v2/ciderd/heartbeat", post(heartbeat))
        .layer(DefaultBodyLimit::max(crate::MAX_BODY_BYTES))
        .with_state(state)
}

fn invalid(message: impl Into<String>) -> ApiError {
    ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, "invalid_ciderd_payload", message)
}

fn decode<T: serde::de::DeserializeOwned>(input: &str) -> ApiResult<T> {
    serde_json::from_str(input).map_err(|_| {
        ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", "Stored collector state is invalid")
    })
}

fn millis(input: &str) -> ApiResult<i64> {
    DateTime::parse_from_rfc3339(input)
        .map(|t| t.timestamp_millis())
        .map_err(|_| invalid("Expected an RFC3339 timestamp"))
}

fn private_metadata(value: &Value) -> ApiResult<()> {
    match value {
        Value::Object(fields) => {
            for (key, child) in fields {
                let key = key.to_ascii_lowercase().replace('-', "_");
                if ["file_contents", "file_content", "file_path", "filename", "filenames",
                    "command_line", "commandline", "argv", "username", "user_name",
                    "password", "authorization", "access_token"].contains(&key.as_str()) {
                    return Err(invalid("File-level or secret metadata is not accepted"));
                }
                private_metadata(child)?;
            }
        }
        Value::Array(values) => for child in values { private_metadata(child)?; },
        _ => {}
    }
    Ok(())
}

async fn heartbeat(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<(HeaderMap, Json<Value>)> {
    let _permit = state.permits.try_acquire().map_err(|_| ApiError::unavailable())?;
    if !headers.get("content-type").and_then(|h| h.to_str().ok())
        .is_some_and(|h| h.split(';').next().unwrap_or("").trim().eq_ignore_ascii_case("application/json")) {
        return Err(ApiError::new(StatusCode::UNSUPPORTED_MEDIA_TYPE, "content_type", "Use application/json"));
    }
    let token = headers.get("authorization").and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .filter(|token| !token.is_empty() && token.len() <= 4096)
        .ok_or_else(|| ApiError::new(StatusCode::UNAUTHORIZED, "unauthorized", "A node credential is required"))?;
    let hb: Heartbeat = wire::parse_json(&body).map_err(|_| invalid("Invalid schema-2 heartbeat JSON"))?;
    let raw = serde_json::to_value(&hb).map_err(|_| invalid("Cannot encode heartbeat"))?;
    private_metadata(&raw)?;
    let fingerprint = store::fingerprint(raw.to_string().as_bytes());
    let now = Utc::now().timestamp_millis();
    let _writer = state.writer.lock().await;
    let mut tx = state.db.begin().await?;
    let token_hash = store::fingerprint(token.as_bytes());
    let node = sqlx::query("SELECT * FROM nodes WHERE credential_hash=? AND revoked_at IS NULL")
        .bind(&token_hash).fetch_optional(&mut *tx).await?
        .ok_or_else(|| ApiError::new(StatusCode::UNAUTHORIZED, "unauthorized", "Invalid node credential"))?;
    let expected: String = node.get("credential_hash");
    if !bool::from(expected.as_bytes().ct_eq(token_hash.as_bytes())) {
        return Err(ApiError::new(StatusCode::UNAUTHORIZED, "unauthorized", "Invalid node credential"));
    }
    let node_id: String = node.get("node_id");
    if node_id != hb.node_id {
        return Err(ApiError::new(StatusCode::FORBIDDEN, "forbidden", "Credential belongs to another node"));
    }
    let mut response_headers = HeaderMap::new();
    response_headers.insert("cache-control", "no-store".parse().expect("static header"));
    if let Some(receipt) = sqlx::query("SELECT fingerprint,ack_json FROM cider_receipts WHERE node_id=? AND session_id=? AND sequence=?")
        .bind(&node_id).bind(&hb.agent_session_id).bind(hb.sequence.to_string()).fetch_optional(&mut *tx).await? {
        if receipt.get::<String, _>("fingerprint") != fingerprint {
            return Err(ApiError::conflict("Sequence was already accepted with different content"));
        }
        let ack: Value = decode(&receipt.get::<String, _>("ack_json"))?;
        tx.commit().await?;
        return Ok((response_headers, Json(ack)));
    }
    if millis(&hb.created_at)?.abs_diff(now) > 300_000 {
        return Err(invalid("created_at must be within 300 seconds of server time"));
    }
    let stored: Option<String> = sqlx::query_scalar("SELECT state_json FROM cider_nodes WHERE node_id=?")
        .bind(&node_id).fetch_optional(&mut *tx).await?;
    let is_new = stored.is_none();
    let mut receiver = match stored {
        Some(value) => decode::<Receiver>(&value)?,
        None => {
            if node.get::<i64, _>("inventory_generation") != 0 {
                return Err(ApiError::conflict("Enroll a separate node identity for ciderd; legacy inventory already exists"));
            }
            Receiver {
                generation: hb.agent_generation, session: hb.agent_session_id.clone(),
                boot: hb.boot_id.clone(), sequence: wire::Decimal::default(), inventory: None,
                projection_generation: 0, graph: Graph::default(),
            }
        }
    };
    if receiver.projection_generation != node.get::<i64, _>("inventory_generation") {
        return Err(ApiError::conflict("Legacy inventory changed this ciderd node; do not mix ingestion protocols"));
    }
    let new_generation = !is_new && hb.agent_generation > receiver.generation;
    if !is_new && (hb.agent_generation < receiver.generation ||
        (hb.agent_generation == receiver.generation &&
            (hb.agent_session_id != receiver.session || hb.boot_id != receiver.boot || hb.sequence <= receiver.sequence))) {
        return Err(ApiError::conflict("Stale generation, session, boot, or sequence"));
    }
    if new_generation {
        if receiver.session == hb.agent_session_id {
            return Err(ApiError::conflict("A new generation requires a new agent session"));
        }
        receiver.inventory = None;
    }
    // Validate references against retained upserts, not only this message's inventory subset.
    if hb.inventory.included || receiver.inventory == Some(hb.inventory.revision) {
        hb.validate_with_resources(&receiver.graph.resources).map_err(|e| invalid(e.to_string()))?;
    } else {
        hb.validate().map_err(|e| invalid(e.to_string()))?;
    }
    if receiver.inventory.is_some_and(|revision| hb.inventory.revision < revision) {
        return Err(ApiError::conflict("Inventory revision moved backwards within an agent generation"));
    }
    let graph_changed = if hb.inventory.included {
        let changed = update_graph(&mut receiver.graph, &hb)?;
        let changed = changed || receiver.inventory != Some(hb.inventory.revision) || new_generation || is_new;
        receiver.inventory = Some(hb.inventory.revision);
        changed
    } else { false };
    receiver.generation = hb.agent_generation;
    receiver.session = hb.agent_session_id.clone();
    receiver.boot = hb.boot_id.clone();
    receiver.sequence = hb.sequence;
    if graph_changed {
        receiver.projection_generation = receiver.projection_generation.checked_add(1)
            .ok_or_else(|| ApiError::conflict("Inventory generation exhausted"))?;
        project_inventory(&mut tx, &hb, &receiver).await?;
    }
    let graph_known = receiver.inventory == Some(hb.inventory.revision);
    let mut stored_samples = 0usize;
    let mut collections: Vec<&Collection> = hb.collections.iter().collect();
    collections.sort_by_key(|c| c.finished_monotonic_ns);
    for collection in collections {
        let digest = store::fingerprint(serde_json::to_string(collection).expect("wire collection").as_bytes());
        let projected = record(&mut tx, &node_id, "collection", &collection.collection_id, &digest, now).await?;
        if !projected && graph_known {
            if let Some(resource) = receiver.graph.resources.get(&collection.resource_id) {
                if collection.clock_id == hb.clock_id && millis(&collection.finished_at)? >= now - 86_400_000 {
                    if millis(&collection.finished_at)? > now + 300_000 {
                        return Err(invalid("A collection timestamp is more than 300 seconds in the future"));
                    }
                    for metric in &collection.metrics {
                        stored_samples += project_metric(&mut tx, &hb, &receiver, resource, collection, metric, now).await?;
                    }
                    if let Some(used) = apfs_used(collection) {
                        stored_samples += project_metric(&mut tx, &hb, &receiver, resource, collection, &used, now).await?;
                    }
                }
                mark_projected(&mut tx, &node_id, "collection", &collection.collection_id).await?;
            }
        }
    }
    let mut stored_events = 0usize;
    for event in &hb.events {
        let digest = store::fingerprint(serde_json::to_string(event).expect("wire event").as_bytes());
        if !record(&mut tx, &node_id, "event", &event.event_id, &digest, now).await? && graph_known {
            if let Some(resource) = receiver.graph.resources.get(&event.resource_id) {
                let occurred = millis(&event.observed_at)?;
                if occurred > now + 300_000 { return Err(invalid("Event timestamp is in the future")); }
                if occurred >= now - 30 * 86_400_000 {
                    let object_id = object_id(&node_id, resource)?;
                    let category = match event.category.as_str() {
                        "collector" | "diagnostic" => "collector",
                        "availability" | "lifecycle" => "availability",
                        _ => "storage",
                    };
                    let severity = if event.severity == "error" { "critical" } else { &event.severity };
                    let value = json!({"object_id":object_id,"category":category,"severity":severity,
                        "occurred_at":event.observed_at,"source":event.source,"summary":event.message,
                        "details":{"ciderd":event}});
                    sqlx::query("INSERT INTO events(node_id,object_id,event_json,occurred_at,received_at) VALUES (?,?,?,?,?)")
                        .bind(&node_id).bind(&object_id).bind(value.to_string()).bind(occurred).bind(now).execute(&mut *tx).await?;
                    stored_events += 1;
                }
                mark_projected(&mut tx, &node_id, "event", &event.event_id).await?;
            }
        }
    }
    update_collector_states(&mut tx, &hb, &receiver, now).await?;
    sqlx::query("UPDATE nodes SET agent_json=?,last_seen_at=?,goodbye_at=NULL,boot_id=?,boot_observed_at=?,inventory_generation=? WHERE node_id=?")
        .bind(serde_json::to_string(&hb.agent).expect("wire agent")).bind(now).bind(&hb.boot_id).bind(now)
        .bind(receiver.projection_generation).bind(&node_id).execute(&mut *tx).await?;
    let change_id = store::change(&mut tx, "telemetry", Some(&node_id),
        json!({"protocol":"ciderd/2.0","sequence":hb.sequence,"stored_samples":stored_samples,"stored_events":stored_events})).await?;
    sqlx::query("INSERT INTO batches(node_id,boot_id,sequence,fingerprint,raw_json,received_at,sample_count,event_count,change_id) VALUES (?,?,?,?,?,?,?,?,?)")
        .bind(&node_id).bind(format!("ciderd:{}:{}", hb.agent_generation, hb.agent_session_id))
        .bind(hb.sequence.to_string()).bind(&fingerprint).bind(raw.to_string()).bind(now)
        .bind(stored_samples as i64).bind(stored_events as i64).bind(change_id).execute(&mut *tx).await?;
    let ack = json!({"schema_version":"2.0","agent_session_id":hb.agent_session_id,
        "accepted_sequence":hb.sequence,"inventory_revision":receiver.inventory,
        "request_inventory":!graph_known,"server_received_at":store::timestamp(now)});
    sqlx::query("INSERT INTO cider_receipts(node_id,session_id,sequence,fingerprint,ack_json,received_at) VALUES (?,?,?,?,?,?)")
        .bind(&node_id).bind(&hb.agent_session_id).bind(hb.sequence.to_string()).bind(&fingerprint)
        .bind(ack.to_string()).bind(now).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO cider_nodes(node_id,state_json,updated_at) VALUES (?,?,?) ON CONFLICT(node_id) DO UPDATE SET state_json=excluded.state_json,updated_at=excluded.updated_at")
        .bind(&node_id).bind(serde_json::to_string(&receiver).expect("receiver state")).bind(now).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok((response_headers, Json(ack)))
}

fn accept_version(graph: &mut Graph, key: String, generation: wire::Decimal, revision: wire::Decimal, value: &impl Serialize) -> ApiResult<bool> {
    let fingerprint = store::fingerprint(serde_json::to_string(value).expect("wire record").as_bytes());
    if let Some(old) = graph.versions.get(&key) {
        if old.generation == generation {
            if revision < old.revision { return Err(ApiError::conflict("Entity revision moved backwards")); }
            if revision == old.revision {
                if old.fingerprint != fingerprint { return Err(ApiError::conflict("Entity revision was reused with different content")); }
                return Ok(false);
            }
        }
    }
    graph.versions.insert(key, Version { generation, revision, fingerprint });
    Ok(true)
}

fn update_graph(graph: &mut Graph, hb: &Heartbeat) -> ApiResult<bool> {
    let mut changed = false;
    for resource in &hb.resources {
        if accept_version(graph, format!("resource:{}", resource.resource_id), hb.agent_generation, resource.revision, resource)? {
            graph.resources.insert(resource.resource_id.clone(), resource.clone());
            changed = true;
        }
    }
    for relationship in &hb.relationships {
        if accept_version(graph, format!("relationship:{}", relationship.relationship_id), hb.agent_generation, relationship.revision, relationship)? {
            graph.relationships.insert(relationship.relationship_id.clone(), relationship.clone());
            changed = true;
        }
    }
    for tombstone in &hb.tombstones {
        if accept_version(graph, format!("{}:{}", tombstone.entity_type, tombstone.entity_id), hb.agent_generation, tombstone.revision, tombstone)? {
            if tombstone.entity_type == "resource" { graph.resources.remove(&tombstone.entity_id); }
            else { graph.relationships.remove(&tombstone.entity_id); }
            changed = true;
        }
    }
    if graph.resources.len() > 4096 || graph.relationships.len() > 8192 || graph.versions.len() > 32768 ||
        serde_json::to_vec(graph).expect("graph").len() > 8 * 1024 * 1024 {
        return Err(invalid("Retained inventory exceeds receiver limits; enroll a new node identity"));
    }
    if graph.resources.values().filter(|r| r.resource_type == "host").count() > 1 {
        return Err(invalid("A node can have only one host resource"));
    }
    Ok(changed)
}

fn object_id(node: &str, resource: &Resource) -> ApiResult<String> {
    if resource.resource_type == "host" { return Ok(node.to_owned()); }
    let namespace = Uuid::parse_str(node).map_err(|_| invalid("Node identity must be the enrolled UUID"))?;
    Ok(Uuid::new_v5(&namespace, format!("ciderd:{}", resource.resource_id).as_bytes()).to_string())
}

fn object_kind(resource: &Resource) -> &'static str {
    match resource.resource_type.as_str() {
        "host" => "node",
        "physical_device" => "device",
        "controller" if resource.attributes.get("scope").and_then(Value::as_str) == Some("driver") => "device",
        "media" => "partition",
        "apfs_container" => "apfs_container",
        "filesystem" if resource.attributes.get("apfs").and_then(Value::as_bool) == Some(true) => "apfs_volume",
        "mount" if resource.attributes.get("filesystem_type").and_then(Value::as_str) == Some("nfs") => "nfs_mount",
        "mount" | "filesystem" => "mount",
        "snapshot" => "snapshot",
        _ => "provider",
    }
}

async fn project_inventory(tx: &mut Transaction<'_, Sqlite>, hb: &Heartbeat, receiver: &Receiver) -> ApiResult<()> {
    for tombstone in &hb.tombstones {
        if tombstone.entity_type == "resource" && !receiver.graph.resources.contains_key(&tombstone.entity_id) {
            sqlx::query("UPDATE objects SET active=0 WHERE node_id=? AND local_id=? AND object_id<>?")
                .bind(&hb.node_id).bind(format!("ciderd:{}", tombstone.entity_id)).bind(&hb.node_id).execute(&mut **tx).await?;
        }
    }
    for resource in receiver.graph.resources.values() {
        let id = object_id(&hb.node_id, resource)?;
        let kind = object_kind(resource);
        let mut properties = json!(resource.attributes);
        properties["ciderd_resource_id"] = json!(resource.resource_id);
        properties["ciderd_resource_type"] = json!(resource.resource_type);
        properties["ciderd_revision"] = json!(resource.revision);
        properties["identity_confidence"] = json!(resource.identity_confidence);
        properties["ciderd_capabilities"] = json!(resource.capabilities);
        if resource.resource_type == "controller" && kind == "device" {
            properties["throughput_scope"] = json!("iokit_driver");
        }
        if let Some(path) = resource.attributes.get("mount_path") { properties["mount_point"] = path.clone(); }
        if let Some(fsid) = resource.attributes.get("fsid") { properties["filesystem_id"] = json!(fsid.to_string()); }
        if let Some(pool) = resource.attributes.get("capacity_pool_id").and_then(Value::as_str)
            .and_then(|id| receiver.graph.resources.get(id)) {
            properties["capacity_pool_id"] = json!(object_id(&hb.node_id, pool)?);
        }
        let relationships: Vec<_> = receiver.graph.relationships.values().filter(|r|
            r.from_resource_id == resource.resource_id || r.to_resource_id == resource.resource_id).collect();
        let mut parents = vec![];
        for relation in &relationships {
            let parent = if relation.relation == "contains" && relation.to_resource_id == resource.resource_id {
                Some(&relation.from_resource_id)
            } else if ["backed_by", "attached_to", "snapshot_of", "mounts"].contains(&relation.relation.as_str()) && relation.from_resource_id == resource.resource_id {
                Some(&relation.to_resource_id)
            } else { None };
            if let Some(parent) = parent.and_then(|id| receiver.graph.resources.get(id)) { parents.push(object_id(&hb.node_id, parent)?); }
        }
        parents.sort(); parents.dedup();
        if parents.is_empty() && kind != "node" { parents.push(hb.node_id.clone()); }
        properties["ciderd_relationships"] = json!(relationships);
        let local_id = if kind == "node" { "node".to_owned() } else { format!("ciderd:{}", resource.resource_id) };
        sqlx::query("INSERT INTO objects(object_id,node_id,local_id,kind,parents_json,properties_json,generation,active) VALUES (?,?,?,?,?,?,?,1) ON CONFLICT(object_id) DO UPDATE SET kind=excluded.kind,parents_json=excluded.parents_json,properties_json=excluded.properties_json,generation=excluded.generation,active=1")
            .bind(&id).bind(&hb.node_id).bind(local_id).bind(kind).bind(json!(parents).to_string())
            .bind(properties.to_string()).bind(receiver.projection_generation).execute(&mut **tx).await?;
    }
    // A graph upsert is not a new acquisition or a physical counter reset.
    sqlx::query("UPDATE latest_samples SET sample_json=json_set(sample_json,'$.inventory_generation',?) WHERE object_id IN (SELECT object_id FROM objects WHERE node_id=? AND active=1) AND json_extract(sample_json,'$.ciderd') IS NOT NULL")
        .bind(receiver.projection_generation.to_string()).bind(&hb.node_id).execute(&mut **tx).await?;
    Ok(())
}

async fn record(tx: &mut Transaction<'_, Sqlite>, node: &str, kind: &str, id: &str, fingerprint: &str, now: i64) -> ApiResult<bool> {
    if let Some(row) = sqlx::query("SELECT fingerprint,projected FROM cider_records WHERE node_id=? AND entity_type=? AND entity_id=?")
        .bind(node).bind(kind).bind(id).fetch_optional(&mut **tx).await? {
        if row.get::<String, _>("fingerprint") != fingerprint {
            return Err(ApiError::conflict("An immutable collection or event ID was reused with different content"));
        }
        return Ok(row.get::<i64, _>("projected") != 0);
    }
    sqlx::query("INSERT INTO cider_records(node_id,entity_type,entity_id,fingerprint,received_at) VALUES (?,?,?,?,?)")
        .bind(node).bind(kind).bind(id).bind(fingerprint).bind(now).execute(&mut **tx).await?;
    Ok(false)
}

async fn mark_projected(tx: &mut Transaction<'_, Sqlite>, node: &str, kind: &str, id: &str) -> ApiResult<()> {
    sqlx::query("UPDATE cider_records SET projected=1 WHERE node_id=? AND entity_type=? AND entity_id=?")
        .bind(node).bind(kind).bind(id).execute(&mut **tx).await?;
    Ok(())
}

fn metric_name(metric: &Metric) -> &str {
    // MNT_NOWAIT capacity is a cached observation, never an alias for live statfs.
    if metric.freshness.as_deref() != Some("live") { return &metric.name; }
    match metric.name.as_str() {
        "storage.device.read_bytes_total" => "device_read_bytes_total",
        "storage.device.write_bytes_total" => "device_write_bytes_total",
        "storage.device.read_operations_total" => "device_read_ops_total",
        "storage.device.write_operations_total" => "device_write_ops_total",
        "storage.device.read_errors_total" => "device_read_errors_total",
        "storage.device.write_errors_total" => "device_write_errors_total",
        "storage.device.read_retries_total" => "device_read_retries_total",
        "storage.device.write_retries_total" => "device_write_retries_total",
        "storage.device.read_accounted_time_nanoseconds_total" => "device_read_time_ns_total",
        "storage.device.write_accounted_time_nanoseconds_total" => "device_write_time_ns_total",
        "storage.filesystem.total_bytes" | "storage.apfs.container_capacity_bytes" => "capacity_bytes",
        "storage.filesystem.free_bytes" | "storage.apfs.container_free_bytes" => "free_bytes",
        "storage.filesystem.available_bytes" => "available_bytes",
        "storage.filesystem.block_accounted_used_bytes" | "storage.apfs.volume_used_bytes" | "orchard.apfs.container_used_bytes" => "used_bytes",
        "storage.filesystem.reported_files" => "files_total",
        "storage.filesystem.reported_free_files" => "files_free",
        "storage.apfs.volume_quota_bytes" => "apfs_quota_bytes",
        "storage.apfs.volume_reserve_bytes" => "apfs_reserve_bytes",
        "storage.apfs.volume_purgeable_bytes" => "apfs_purgeable_bytes",
        "storage.apfs.snapshot_count" => "snapshot_count",
        "storage.media.smart_passed" => "smart_healthy",
        "storage.media.temperature_celsius" => "device_temperature_celsius",
        "storage.nvme.media_errors_total" => "nvme_media_errors_total",
        "storage.nvme.critical_warning" => "nvme_critical_warning",
        "storage.nvme.available_spare_percent" => "nvme_available_spare_percent",
        "storage.nvme.percentage_used" => "nvme_percentage_used",
        "storage.nfs.client_operations_total" => "nfs_operations_total",
        "storage.nfs.rpc_timeouts_total" => "nfs_rpc_timeouts_total",
        "storage.nfs.rpc_retries_total" => "nfs_retransmissions_total",
        _ => &metric.name,
    }
}

fn apfs_used(collection: &Collection) -> Option<Metric> {
    let total = collection.metrics.iter().find(|m| m.name == "storage.apfs.container_capacity_bytes" && m.availability == "available" && m.freshness.as_deref() == Some("live"))?;
    let free = collection.metrics.iter().find(|m| m.name == "storage.apfs.container_free_bytes" && m.availability == "available" && m.freshness.as_deref() == Some("live"))?;
    let used = unsigned(total.value.as_ref()?)?.checked_sub(unsigned(free.value.as_ref()?)?)?;
    let mut metric = total.clone();
    metric.name = "orchard.apfs.container_used_bytes".into();
    metric.value = Some(json!(used.to_string()));
    metric.source_field = Some("server: container_capacity_bytes - container_free_bytes".into());
    Some(metric)
}

fn unsigned(value: &Value) -> Option<u128> {
    value.as_str().and_then(|n| n.parse().ok()).or_else(|| value.as_u64().map(u128::from))
}

async fn project_metric(
    tx: &mut Transaction<'_, Sqlite>, hb: &Heartbeat, receiver: &Receiver,
    resource: &Resource, collection: &Collection, metric: &Metric, now: i64,
) -> ApiResult<usize> {
    let object = object_id(&hb.node_id, resource)?;
    let scope = store::default_scope(object_kind(resource));
    let name = metric_name(metric);
    let source = format!("ciderd:{}", collection.collector);
    let labels: BTreeMap<String, String> = metric.attributes.iter().map(|(k,v)|
        (k.clone(), v.as_str().map(str::to_owned).unwrap_or_else(|| v.to_string()))).collect();
    let labels_json = serde_json::to_string(&labels).expect("labels");
    let key = store::fingerprint(json!([object,name,source,scope,labels]).to_string().as_bytes());
    let prior: Option<String> = sqlx::query_scalar("SELECT observation_json FROM cider_series WHERE node_id=? AND series_key=?")
        .bind(&hb.node_id).bind(&key).fetch_optional(&mut **tx).await?;
    let prior: Option<Value> = prior.as_deref().map(decode).transpose()?;
    let stale_after = hb.collector_states.iter().find(|s| s.resource_id == collection.resource_id && s.collector == collection.collector)
        .map(|s| s.stale_after_seconds).unwrap_or(90);
    let mono = collection.finished_monotonic_ns.get();
    let age_at_receipt = (hb.monotonic_ns.get() - mono) as f64 / 1_000_000_000.0;
    let observed = millis(&collection.finished_at)?;
    let source_ok = metric.availability == "available" && metric.freshness.as_deref() == Some("live");
    let state = if metric.availability == "available" {
        if source_ok && age_at_receipt <= stale_after as f64 { "ok" } else { "stale" }
    } else { match metric.availability.as_str() {
        "not_collected" => "failed", value => value,
    }};
    let value = metric.value.clone().unwrap_or(Value::Null);
    let latest = prior.as_ref().is_none_or(|p| {
        p["clock_id"] != json!(collection.clock_id) || unsigned(&p["monotonic_ns"]).is_some_and(|old| mono > old)
    });
    let (rate, derivation) = if metric.kind != "counter" || !source_ok {
        (None, "unavailable")
    } else if let Some(previous) = prior.as_ref() {
        if previous["clock_id"] != json!(collection.clock_id) || previous["counter_epoch"] != json!(metric.counter_epoch) {
            (None, "reset")
        } else if previous["availability"] != "available" || previous["freshness"] != "live" {
            (None, "unavailable")
        } else if let Some(elapsed) = unsigned(&previous["monotonic_ns"]).and_then(|old| mono.checked_sub(old)).filter(|n| *n > 0 && *n <= stale_after as u128 * 1_000_000_000) {
            match unsigned(&value).and_then(|current| current.checked_sub(unsigned(&previous["value"])?)) {
                Some(delta) => (Some(delta as f64 / (elapsed as f64 / 1_000_000_000.0)), "ok"),
                None => (None, "reset"),
            }
        } else { (None, "unavailable") }
    } else { (None, "insufficient_data") };
    let mut history_labels = labels.clone();
    history_labels.insert("ciderd_clock_id".into(), collection.clock_id.clone());
    if let Some(epoch) = &metric.counter_epoch { history_labels.insert("ciderd_counter_epoch".into(), epoch.clone()); }
    let numeric = if state == "ok" {
        value.as_f64().or_else(|| value.as_str().and_then(|s| s.parse::<f64>().ok())).filter(|v| v.is_finite())
    } else { None };
    let result = sqlx::query("INSERT OR IGNORE INTO metric_samples(node_id,object_id,boot_id,generation,name,kind,unit,state,source,scope,labels_json,value_json,numeric_value,observed_at,received_at,rate_per_second,derivation_state) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)")
        .bind(&hb.node_id).bind(&object).bind(&hb.boot_id).bind(receiver.projection_generation)
        .bind(name).bind(&metric.kind).bind(&metric.unit).bind(state).bind(&source).bind(scope)
        .bind(serde_json::to_string(&history_labels).expect("labels")).bind(value.to_string()).bind(numeric)
        .bind(observed).bind(now).bind(rate).bind(derivation).execute(&mut **tx).await?;
    if result.rows_affected() == 0 {
        return Err(ApiError::conflict("Different collection IDs produced the same series timestamp"));
    }
    if latest {
        let observation = json!({"clock_id":collection.clock_id,"monotonic_ns":collection.finished_monotonic_ns,
            "counter_epoch":metric.counter_epoch,"value":value,"availability":metric.availability,"freshness":metric.freshness});
        sqlx::query("INSERT INTO cider_series(node_id,series_key,observation_json,updated_at) VALUES (?,?,?,?) ON CONFLICT(node_id,series_key) DO UPDATE SET observation_json=excluded.observation_json,updated_at=excluded.updated_at")
            .bind(&hb.node_id).bind(&key).bind(observation.to_string()).bind(now).execute(&mut **tx).await?;
        // A failed attempt does not erase a previous valid observation. Its status remains visible separately.
        if source_ok || prior.as_ref().is_none_or(|p| p["availability"] != "available" || p["freshness"] != "live") {
            let raw = json!({"sample":{"object_id":object,"name":name,"kind":metric.kind,"value":value,
                "unit":metric.unit,"state":state,"source":source,"scope":scope,"labels":labels,"observed_at":collection.finished_at},
                "scope":scope,"boot_id":hb.boot_id,"inventory_generation":receiver.projection_generation.to_string(),
                "received_at":store::timestamp(now),"derived_rate_per_second":rate,"derivation_state":derivation,
                "ciderd":{"collection_id":collection.collection_id,"resource_id":collection.resource_id,
                    "source_metric":metric,"clock_id":collection.clock_id,"counter_epoch":metric.counter_epoch,
                    "age_at_receipt_seconds":age_at_receipt,"stale_after_seconds":stale_after,
                    "original_inventory_generation":receiver.projection_generation.to_string()}});
            sqlx::query("INSERT INTO latest_samples(object_id,name,source,scope,labels_json,sample_json,observed_at,received_at) VALUES (?,?,?,?,?,?,?,?) ON CONFLICT(object_id,name,source,scope,labels_json) DO UPDATE SET sample_json=excluded.sample_json,observed_at=excluded.observed_at,received_at=excluded.received_at")
                .bind(&object).bind(name).bind(&source).bind(scope).bind(&labels_json).bind(raw.to_string()).bind(observed).bind(now).execute(&mut **tx).await?;
        }
    }
    Ok(1)
}

async fn update_collector_states(tx: &mut Transaction<'_, Sqlite>, hb: &Heartbeat, receiver: &Receiver, now: i64) -> ApiResult<()> {
    for resource in receiver.graph.resources.values() {
        let states: Vec<_> = hb.collector_states.iter().filter(|s| s.resource_id == resource.resource_id).map(|s| {
            let attempt = s.last_attempt_id.as_ref().and_then(|id| hb.collections.iter().find(|c| c.collection_id == *id));
            json!({"state":s,"last_attempt":attempt.map(|c| json!({"collection_id":c.collection_id,
                "status":c.status,"finished_at":c.finished_at,"error":c.error})),"reported_at":hb.created_at})
        }).collect();
        if !states.is_empty() {
            sqlx::query("UPDATE objects SET properties_json=json_set(properties_json,'$.ciderd_collector_states',json(?)) WHERE object_id=? AND node_id=?")
                .bind(json!(states).to_string()).bind(object_id(&hb.node_id, resource)?).bind(&hb.node_id).execute(&mut **tx).await?;
        }
    }
    let rows = sqlx::query("SELECT l.object_id,l.name,l.source,l.scope,l.labels_json,l.sample_json FROM latest_samples l JOIN objects o ON o.object_id=l.object_id WHERE o.node_id=? AND json_extract(l.sample_json,'$.ciderd') IS NOT NULL")
        .bind(&hb.node_id).fetch_all(&mut **tx).await?;
    for row in rows {
        let mut raw: Value = decode(&row.get::<String, _>("sample_json"))?;
        let received = raw["received_at"].as_str().and_then(|s| millis(s).ok()).unwrap_or(now);
        let age = raw["ciderd"]["age_at_receipt_seconds"].as_f64().unwrap_or(f64::INFINITY) + (now - received).max(0) as f64 / 1000.0;
        let policy = raw["ciderd"]["stale_after_seconds"].as_u64().unwrap_or(90);
        if raw["sample"]["state"] == "ok" && (age > policy as f64 || raw["ciderd"]["clock_id"] != json!(hb.clock_id)) {
            raw["sample"]["state"] = json!("stale");
            sqlx::query("UPDATE latest_samples SET sample_json=? WHERE object_id=? AND name=? AND source=? AND scope=? AND labels_json=?")
                .bind(raw.to_string()).bind(row.get::<String, _>("object_id")).bind(row.get::<String, _>("name"))
                .bind(row.get::<String, _>("source")).bind(row.get::<String, _>("scope")).bind(row.get::<String, _>("labels_json"))
                .execute(&mut **tx).await?;
        }
    }
    Ok(())
}

/// Source age uses monotonic acquisition age plus server elapsed time, not delivery time.
pub fn read_sample(raw: &Value, now: i64) -> Value {
    let mut value = raw.clone();
    if raw.get("ciderd").is_none() { return value; }
    let received = raw["received_at"].as_str().and_then(|s| millis(s).ok()).unwrap_or(now);
    let age = raw["ciderd"]["age_at_receipt_seconds"].as_f64().unwrap_or(f64::INFINITY) + (now - received).max(0) as f64 / 1000.0;
    let policy = raw["ciderd"]["stale_after_seconds"].as_u64().unwrap_or(90);
    if age > policy as f64 && value["sample"]["state"] == "ok" { value["sample"]["state"] = json!("stale"); }
    value
}

pub async fn retain(tx: &mut Transaction<'_, Sqlite>, now: i64) -> Result<(), sqlx::Error> {
    let cutoff = now - 30 * 86_400_000;
    for table in ["cider_receipts", "cider_records"] {
        sqlx::query(&format!("DELETE FROM {table} WHERE received_at<?")).bind(cutoff).execute(&mut **tx).await?;
    }
    sqlx::query("DELETE FROM cider_series WHERE updated_at<?").bind(cutoff).execute(&mut **tx).await?;
    Ok(())
}
