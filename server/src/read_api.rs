//! Server Spec section 16: same-origin console and current-state read API.
use crate::{error::{ApiError, ApiResult}, store::{self, AppState}};
use axum::{Json, Router, extract::{OriginalUri, Path, Query, State}, http::{HeaderMap, HeaderValue, StatusCode, Uri}, response::{IntoResponse, Response}, routing::get};
use chrono::Utc;
use serde_json::{json, Value};
use sqlx::{Row, sqlite::SqliteRow};
use std::{collections::{BTreeMap, BTreeSet, HashMap}, path::Path as FilePath, sync::{Arc, Mutex}};
use subtle::ConstantTimeEq;
use uuid::Uuid;

const READ_ROUTES: &[&str] = &[
    "/api/v1/cluster", "/api/v1/nodes", "/api/v1/nodes/{node_id}",
    "/api/v1/nodes/{node_id}/inventory", "/api/v1/objects/{object_id}",
    "/api/v1/filesystems", "/api/v1/events", "/api/v1/capabilities",
];
const DIMENSIONS: &[&str] = &["availability", "capacity", "performance", "media_health", "filesystem_integrity", "security_activity", "recovery_readiness", "telemetry_freshness"];
type Parameters = BTreeMap<String, String>;

#[derive(Clone)]
struct ReadState {
    app: AppState,
    pages: Arc<Mutex<HashMap<String, Page>>>,
    budgets: Arc<Mutex<HashMap<String, (f64, i64)>>>,
}
#[derive(Clone)]
struct Page {
    owner: String, path: String, query: Parameters,
    expires: i64, time: i64, change: String, values: Arc<Vec<Value>>, bytes: usize,
}
struct Snapshot {
    time: i64, change: String, nodes: Vec<Value>, objects: Vec<Value>,
    filesystems: Vec<Value>, events: Vec<Value>, cluster: Value,
}

pub fn write_viewer_file(directory: &FilePath, token: &str) -> anyhow::Result<()> {
    use std::{fs, io::Write};
    let path = directory.join("viewer-token");
    if let Ok(meta) = fs::symlink_metadata(&path) {
        anyhow::ensure!(meta.file_type().is_file(), "Viewer credential must be a regular file");
    }
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)] {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&path)?;
    #[cfg(unix)] {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(token.as_bytes())?;
    file.sync_all()?;
    Ok(())
}

pub fn router(app: AppState) -> Router {
    let state = ReadState { app, pages: Default::default(), budgets: Default::default() };
    let mut api = Router::new();
    for route in READ_ROUTES { api = api.route(route, get(read)); }
    api.with_state(state)
        .route("/", get(index))
        .route("/index.html", get(index))
        .route("/css/console.css", get(console_css))
        .route("/css/live.css", get(live_css))
        .route("/js/{asset}", get(javascript))
}

async fn index() -> Response { asset("text/html; charset=utf-8", include_str!("../../web/index.html")) }
async fn console_css() -> Response { asset("text/css; charset=utf-8", include_str!("../../web/css/console.css")) }
async fn live_css() -> Response { asset("text/css; charset=utf-8", include_str!("../../web/css/live.css")) }
async fn javascript(Path(name): Path<String>) -> Response {
    let body = match name.as_str() {
        "app.js" => include_str!("../../web/js/app.js"),
        "api.js" => include_str!("../../web/js/api.js"),
        "data.js" => include_str!("../../web/js/data.js"),
        "model.js" => include_str!("../../web/js/model.js"),
        "views.js" => include_str!("../../web/js/views.js"),
        "stage.js" => include_str!("../../web/js/stage.js"),
        "lib.js" => include_str!("../../web/js/lib.js"),
        _ => return ApiError::new(StatusCode::NOT_FOUND, "not_found", "Asset not found").into_response(),
    };
    asset("text/javascript; charset=utf-8", body)
}
fn asset(content_type: &'static str, body: &'static str) -> Response {
    let mut response = ([("content-type", content_type), ("cache-control", "no-store"), ("x-content-type-options", "nosniff"), ("referrer-policy", "no-referrer")], body).into_response();
    response.headers_mut().insert("content-security-policy", HeaderValue::from_static("default-src 'self'; script-src 'self' https://unpkg.com https://cdn.jsdelivr.net; style-src 'self' 'unsafe-inline' https://fonts.googleapis.com; font-src https://fonts.gstatic.com; img-src 'self' data:; connect-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'"));
    response
}

fn query_error(field: &str, message: &str) -> ApiError {
    let mut error = ApiError::field(field, message);
    error.code = "invalid_query";
    error.message = "Invalid query parameters".into();
    error
}
fn unavailable(message: &str) -> ApiError { ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "unavailable", message) }
fn not_found() -> ApiError { ApiError::new(StatusCode::NOT_FOUND, "not_found", "Resource not found") }
fn value(row: &SqliteRow, column: &str) -> ApiResult<Value> {
    serde_json::from_str(&row.get::<String, _>(column)).map_err(|_| unavailable("Stored metadata cannot be decoded"))
}

async fn read(State(state): State<ReadState>, headers: HeaderMap, OriginalUri(uri): OriginalUri) -> Response {
    let request_id = headers.get("x-request-id").and_then(|v| v.to_str().ok()).and_then(|v| Uuid::parse_str(v).ok()).unwrap_or_else(Uuid::new_v4).to_string();
    let result = dispatch(&state, &headers, &uri, &request_id).await;
    let mut response = match result {
        Ok(response) => response,
        Err(mut error) => { error.request_id = request_id.clone(); error.into_response() }
    };
    response.headers_mut().insert("x-request-id", HeaderValue::from_str(&request_id).expect("UUID header"));
    response.headers_mut().insert("cache-control", HeaderValue::from_static("no-store"));
    if response.status() == StatusCode::TOO_MANY_REQUESTS {
        response.headers_mut().insert("retry-after", HeaderValue::from_static("1"));
    }
    response
}

async fn authorize(state: &ReadState, headers: &HeaderMap) -> ApiResult<String> {
    let token = headers.get("authorization").and_then(|h| h.to_str().ok()).and_then(|h| h.strip_prefix("Bearer ")).unwrap_or("");
    let hash = store::fingerprint(token.as_bytes());
    let admin = bool::from(hash.as_bytes().ct_eq(state.app.admin_hash.as_bytes()));
    let viewer_hash = store::fingerprint(state.app.viewer_token.as_bytes());
    let viewer = bool::from(hash.as_bytes().ct_eq(viewer_hash.as_bytes())) && Utc::now().timestamp_millis() < state.app.viewer_expires_at;
    if !admin && !viewer {
        let node: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM nodes WHERE credential_hash=? AND revoked_at IS NULL").bind(&hash).fetch_one(&state.app.db).await?;
        return Err(if node > 0 { ApiError::new(StatusCode::FORBIDDEN, "forbidden", "Node credentials cannot read cluster data") }
            else { ApiError::new(StatusCode::UNAUTHORIZED, "unauthenticated", "A valid, unexpired viewer or administrator credential is required") });
    }
    let now = Utc::now().timestamp_millis();
    let mut budgets = state.budgets.lock().map_err(|_| unavailable("Read budget unavailable"))?;
    let budget = budgets.entry(hash.clone()).or_insert((20.0, now));
    budget.0 = (budget.0 + (now - budget.1).max(0) as f64 / 500.0).min(20.0);
    budget.1 = now;
    if budget.0 < 1.0 { return Err(ApiError::new(StatusCode::TOO_MANY_REQUESTS, "rate_limited", "Read budget exhausted")); }
    budget.0 -= 1.0;
    Ok(hash)
}

fn response(data: Value, time: i64, change: &str, next: Option<String>, request_id: &str) -> Response {
    Json(json!({"data":data,"meta":{"api_version":"1","server_time":store::timestamp(time),"request_id":request_id,"snapshot_cursor":change,"next_cursor":next}})).into_response()
}

async fn dispatch(state: &ReadState, headers: &HeaderMap, uri: &Uri, request_id: &str) -> ApiResult<Response> {
    let _permit = state.app.permits.clone().try_acquire_owned().map_err(|_| ApiError::unavailable())?;
    let owner = authorize(state, headers).await?;
    let pairs = Query::<Vec<(String, String)>>::try_from_uri(uri).map_err(|_| query_error("query", "Expected URL-encoded query parameters"))?.0;
    let mut query = Parameters::new();
    for (key, val) in pairs {
        if query.insert(key.clone(), val).is_some() { return Err(query_error(&key, "Duplicate query parameter")); }
    }
    let path = uri.path();
    let parts: Vec<_> = path.trim_matches('/').split('/').collect();
    let resource = parts.get(2).copied().unwrap_or("");
    let id = parts.get(3).copied();
    if let Some(id) = id { Uuid::parse_str(id).map_err(|_| query_error("id", "Expected a resource UUID"))?; }
    let inventory = resource == "nodes" && parts.get(4) == Some(&"inventory");
    let list = inventory || (id.is_none() && matches!(resource, "nodes" | "filesystems" | "events"));
    let mut allowed = if list { vec!["limit", "cursor"] } else { vec![] };
    if inventory { allowed.extend(["generation", "kind"]); }
    else if resource == "nodes" && list { allowed.extend(["availability", "health", "q"]); }
    else if resource == "filesystems" { allowed.extend(["node_id", "classification", "filesystem_type", "health"]); }
    else if resource == "events" { allowed.extend(["node_id", "object_id", "category", "severity", "from", "to"]); }
    for key in query.keys() { if !allowed.contains(&key.as_str()) { return Err(query_error(key, "Unknown query parameter")); } }
    validate_enum(&query, "availability", &["online", "degraded", "offline", "unknown"])?;
    validate_enum(&query, "health", &["healthy", "warning", "critical", "unknown"])?;
    validate_enum(&query, "kind", &["node", "device", "partition", "apfs_store", "apfs_container", "apfs_volume", "snapshot", "mount", "nfs_mount", "provider"])?;
    validate_enum(&query, "classification", &["local", "network", "removable"])?;
    validate_enum(&query, "category", &["inventory", "availability", "storage", "security", "collector", "alert"])?;
    validate_enum(&query, "severity", &["info", "warning", "critical"])?;
    for key in ["q", "filesystem_type"] {
        if query.get(key).is_some_and(|v| v.chars().count() > 128) { return Err(query_error(key, "Maximum length is 128 characters")); }
    }
    for key in ["node_id", "object_id"] {
        if let Some(id) = query.get(key) { Uuid::parse_str(id).map_err(|_| query_error(key, "Expected a resource UUID"))?; }
    }
    let limit = match query.get("limit") {
        Some(n) => n.parse::<usize>().ok().filter(|n| (1..=500).contains(n)).ok_or_else(|| query_error("limit", "Expected 1-500"))?,
        None => 100,
    };
    if let Some(cursor) = query.remove("cursor") {
        return page(state, path, &query, &owner, &cursor, limit, request_id);
    }
    let snapshot = snapshot(&state.app, resource == "events", &query).await?;
    let data = match (resource, id, inventory) {
        ("cluster", None, _) => snapshot.cluster.clone(),
        ("capabilities", None, _) => json!({
            "api_version":"1","service_version":env!("CARGO_PKG_VERSION"),"heartbeat_interval_seconds":5,
            "availability_thresholds_seconds":{"degraded":30,"offline":90},"poll_interval_seconds":5,"hidden_poll_interval_seconds":30,
            "implemented_read_routes":READ_ROUTES,"features":{"current_inventory":true,"historical_inventory":false,"events":true,"alerts":false,"sse":false,"metric_history":false,"prometheus":false,"openapi":false},
            "viewer_credential_expires_at":store::timestamp(state.app.viewer_expires_at),"viewer_scope":"telemetry:read","browser_transport":"same_origin",
            "limits":{"default_page_size":100,"maximum_page_size":500,"cursor_ttl_seconds":300,"read_requests_per_minute":120,"read_burst":20}
        }),
        ("nodes", Some(id), true) => {
            let node = snapshot.nodes.iter().find(|n| n["node_id"] == id).ok_or_else(not_found)?;
            if let Some(generation) = query.get("generation") {
                let parsed = generation.parse::<u64>().map_err(|_| query_error("generation", "Expected a decimal generation"))?;
                if parsed.to_string() != node["inventory_generation"].as_str().unwrap_or("") {
                    return Err(query_error("generation", "This release exposes current inventory only"));
                }
            }
            Value::Array(snapshot.objects.iter().filter(|o| o["node_id"] == id && o["active"] == true && matches_filter(o, "kind", &query)).cloned().collect())
        },
        ("nodes", Some(id), false) => snapshot.nodes.iter().find(|n| n["node_id"] == id).cloned().ok_or_else(not_found)?,
        ("nodes", None, _) => Value::Array(snapshot.nodes.iter().filter(|n| {
            matches_filter(n, "availability", &query) && health_filter(n, &query)
                && query.get("q").is_none_or(|q| n["name"].as_str().unwrap_or("").to_lowercase().contains(&q.to_lowercase()))
        }).cloned().collect()),
        ("objects", Some(id), _) => snapshot.objects.iter().find(|o| o["object_id"] == id).cloned().ok_or_else(not_found)?,
        ("filesystems", None, _) => Value::Array(snapshot.filesystems.iter().filter(|f| {
            ["node_id", "classification", "filesystem_type"].iter().all(|key| matches_filter(f, key, &query)) && health_filter(f, &query)
        }).cloned().collect()),
        ("events", None, _) => Value::Array(snapshot.events.clone()),
        _ => return Err(not_found()),
    };
    if !list { return Ok(response(data, snapshot.time, &snapshot.change, None, request_id)); }
    let values = data.as_array().expect("list route");
    let next = if values.len() > limit {
        let bytes = data.to_string().len();
        let mut pages = state.pages.lock().map_err(|_| unavailable("Pagination unavailable"))?;
        pages.retain(|_, p| p.expires > Utc::now().timestamp_millis());
        if pages.len() >= 128 || bytes + pages.values().map(|p| p.bytes).sum::<usize>() > 32 * 1024 * 1024 {
            return Err(ApiError::new(StatusCode::TOO_MANY_REQUESTS, "rate_limited", "Pagination cache is full; retry later"));
        }
        let key = Uuid::new_v4().to_string();
        let expires = snapshot.time + 300_000;
        pages.insert(key.clone(), Page { owner, path: path.into(), query, expires, time: snapshot.time, change: snapshot.change.clone(), values: Arc::new(values.clone()), bytes });
        Some(format!("{key}.{expires}.{limit}"))
    } else { None };
    Ok(response(json!(values.iter().take(limit).collect::<Vec<_>>()), snapshot.time, &snapshot.change, next, request_id))
}

fn validate_enum(query: &Parameters, key: &str, choices: &[&str]) -> ApiResult<()> {
    if query.get(key).is_some_and(|v| !choices.contains(&v.as_str())) { return Err(query_error(key, "Unsupported filter value")); }
    Ok(())
}
fn matches_filter(item: &Value, key: &str, query: &Parameters) -> bool { query.get(key).is_none_or(|v| item[key].as_str() == Some(v.as_str())) }
fn health_filter(item: &Value, query: &Parameters) -> bool { query.get("health").is_none_or(|v| item["health"]["overall"].as_str() == Some(v.as_str())) }
fn page(state: &ReadState, path: &str, query: &Parameters, owner: &str, cursor: &str, limit: usize, request_id: &str) -> ApiResult<Response> {
    let parts: Vec<_> = cursor.split('.').collect();
    if parts.len() != 3 || Uuid::parse_str(parts[0]).is_err() { return Err(query_error("cursor", "Malformed cursor")); }
    let expires: i64 = parts[1].parse().map_err(|_| query_error("cursor", "Malformed cursor"))?;
    let offset: usize = parts[2].parse().map_err(|_| query_error("cursor", "Malformed cursor"))?;
    if expires <= Utc::now().timestamp_millis() { return Err(ApiError::new(StatusCode::GONE, "cursor_expired", "Start a new traversal")); }
    let pages = state.pages.lock().map_err(|_| unavailable("Pagination unavailable"))?;
    let page = pages.get(parts[0]).ok_or_else(|| ApiError::new(StatusCode::GONE, "cursor_expired", "Snapshot is no longer available; start a new traversal"))?;
    if page.path != path || page.query != *query || page.owner != owner || page.expires != expires || offset == 0 || offset >= page.values.len() || offset % limit != 0 {
        return Err(query_error("cursor", "Cursor does not match this traversal"));
    }
    let end = offset.saturating_add(limit).min(page.values.len());
    let next = (end < page.values.len()).then(|| format!("{}.{expires}.{end}", parts[0]));
    Ok(response(json!(&page.values[offset..end]), page.time, &page.change, next, request_id))
}

fn unknown(kind: &str, unit: &str) -> Value {
    json!({"value":null,"kind":kind,"unit":unit,"state":"unknown","source":"server","scope":"object","observed_at":null,"received_at":null,"age_seconds":null,"boot_id":null,"inventory_generation":null})
}
fn metric(object: &Value, name: &str, unit: &str) -> Value {
    object["latest_metrics"].get(name).cloned().unwrap_or_else(|| unknown("gauge", unit))
}
fn number(measurement: &Value) -> Option<f64> {
    if measurement["state"] != "ok" { return None; }
    measurement["value"].as_f64().or_else(|| measurement["value"].as_str()?.parse::<f64>().ok()).filter(|n| n.is_finite())
}
fn integer(measurement: &Value) -> Option<u128> {
    if measurement["state"] != "ok" { return None; }
    measurement["value"].as_str()?.parse().ok()
}
fn aggregate(values: Vec<Value>, unit: &str, exact: bool) -> Value {
    let valid: Vec<_> = values.iter().filter(|m| if exact { integer(m).is_some() } else { number(m).is_some() }).collect();
    let mut out = unknown("gauge", unit);
    out["source"] = json!("server:deduplicated_sum");
    out["scope"] = json!("aggregate");
    out["coverage"] = json!({"observed":valid.len(),"expected":values.len()});
    if valid.is_empty() { return out; }
    out["value"] = if exact { json!(valid.iter().filter_map(|m| integer(m)).sum::<u128>().to_string()) }
        else { json!(valid.iter().filter_map(|m| number(m)).sum::<f64>()) };
    out["state"] = json!("ok");
    out["observed_at"] = json!(valid.iter().filter_map(|m| m["observed_at"].as_str()).min());
    out["received_at"] = json!(valid.iter().filter_map(|m| m["received_at"].as_str()).min());
    out["age_seconds"] = json!(valid.iter().filter_map(|m| m["age_seconds"].as_f64()).reduce(f64::max));
    out
}
fn capacity(objects: &[&Value]) -> Value {
    let mut out = json!({"forecast_full_at":null,"forecast_state":"insufficient_data"});
    for name in ["capacity_bytes", "used_bytes", "free_bytes", "available_bytes"] {
        out[name] = aggregate(objects.iter().map(|o| metric(o, name, "bytes")).collect(), "bytes", true);
    }
    out["used_ratio"] = unknown("gauge", "ratio");
    let complete = ["capacity_bytes", "used_bytes"].iter().all(|key| {
        out[*key]["coverage"]["observed"] == out[*key]["coverage"]["expected"]
    });
    if let (Some(total), Some(used)) = (integer(&out["capacity_bytes"]), integer(&out["used_bytes"])) {
        if complete && total > 0 && used <= total {
            let mut ratio = out["used_bytes"].clone();
            ratio["value"] = json!(used as f64 / total as f64); ratio["unit"] = json!("ratio");
            out["used_ratio"] = ratio;
        }
    }
    out
}
fn health(availability: &str, capacity: &Value) -> Value {
    let mut dimensions = serde_json::Map::new();
    for name in DIMENSIONS { dimensions.insert((*name).into(), json!({"status":"unknown","reasons":[],"evidence":[]})); }
    let status = match availability { "online" => "healthy", "degraded" => "warning", "offline" => "critical", _ => "unknown" };
    dimensions.insert("availability".into(), json!({"status":status,"reasons":[format!("collector_{availability}")],"evidence":[]}));
    if let Some(ratio) = number(&capacity["used_ratio"]) {
        dimensions.insert("capacity".into(), json!({"status":if ratio >= 0.95 {"critical"} else if ratio >= 0.90 {"warning"} else {"healthy"},"reasons":["capacity_used_ratio"],"evidence":[capacity["used_ratio"].clone()]}));
    }
    let overall = ["critical", "warning", "healthy"].into_iter().find(|status| dimensions.values().any(|d| d["status"] == *status)).unwrap_or("unknown");
    let missing: Vec<_> = DIMENSIONS.iter().filter(|d| dimensions[**d]["status"] == "unknown").copied().collect();
    json!({"overall":overall,"dimensions":dimensions,"unknown_dimensions":missing})
}
fn freshness(name: &str) -> i64 {
    if name.starts_with("device_") && (name.contains("_total") || name.contains("_per_second")) { 15 }
    else if name.starts_with("smart_") || name.starts_with("nvme_") || name.contains("temperature") || name == "node_clock_offset_seconds" { 900 }
    else { 90 }
}
fn from_latest(raw: &Value, now: i64, active: bool, boot: Option<&str>, generation: &str) -> Value {
    let sample = &raw["sample"];
    let observed = sample["observed_at"].as_str().and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok()).map(|d| d.timestamp_millis());
    let age = observed.map(|at| (now - at).max(0) as f64 / 1000.0);
    let old_identity = !active || raw["boot_id"].as_str() != boot || raw["inventory_generation"].as_str() != Some(generation);
    let mut state = sample["state"].as_str().unwrap_or("unknown");
    if old_identity || (state == "ok" && age.is_none_or(|age| age > freshness(sample["name"].as_str().unwrap_or("")) as f64)) { state = "stale"; }
    let mut val = sample["value"].clone();
    if sample["kind"] == "counter" || sample["unit"] == "bytes" {
        val = if let Some(n) = val.as_u64() { json!(n.to_string()) }
            else if val.as_str().is_some_and(|s| s.parse::<u64>().is_ok()) { val }
            else { Value::Null };
        if val.is_null() && state == "ok" { state = "unknown"; }
    }
    json!({"value":val,"kind":sample["kind"],"unit":sample["unit"],"state":state,
        "source":sample["source"],"scope":raw["scope"],"observed_at":sample["observed_at"],"received_at":raw["received_at"],"age_seconds":age,
        "boot_id":raw["boot_id"],"inventory_generation":raw["inventory_generation"],"labels":sample["labels"],
        "derived_rate_per_second":if state == "ok" {raw["derived_rate_per_second"].clone()} else {Value::Null},"derivation_state":raw["derivation_state"]})
}
fn rate(object: &Value, name: &str) -> Value {
    let raw = metric(object, name, "bytes"); let mut out = raw.clone();
    out["kind"] = json!("gauge"); out["unit"] = json!("bytes/second");
    out["value"] = if raw["state"] == "ok" && raw["derivation_state"] == "ok" { raw["derived_rate_per_second"].clone() } else { Value::Null };
    if out["value"].is_null() && out["state"] == "ok" { out["state"] = json!("unknown"); }
    out
}
fn local_capacity_objects<'a>(objects: &[&'a Value]) -> Vec<&'a Value> {
    let mut selected = BTreeMap::new();
    for object in objects {
        let properties = &object["properties"];
        let key = if object["kind"] == "apfs_container" { Some(object["object_id"].as_str().unwrap_or("").to_owned()) }
            else if object["kind"] == "mount" && properties["filesystem_type"].as_str().is_some_and(|t| !matches!(t.to_lowercase().as_str(), "apfs" | "nfs" | "nfs4")) {
                properties["filesystem_id"].as_str().map(|id| format!("{}:{id}", object["node_id"].as_str().unwrap_or("")))
            } else { None };
        if let Some(key) = key { selected.entry(key).or_insert(*object); }
    }
    selected.into_values().collect()
}
fn parse_time(query: &Parameters, key: &str, default: i64) -> ApiResult<i64> {
    match query.get(key) { None => Ok(default), Some(text) => chrono::DateTime::parse_from_rfc3339(text).map(|d| d.timestamp_millis()).map_err(|_| query_error(key, "Expected an RFC3339 timestamp")) }
}

async fn snapshot(app: &AppState, include_events: bool, query: &Parameters) -> ApiResult<Snapshot> {
    let mut tx = app.db.begin().await?;
    let change: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(change_id),0) FROM change_log").fetch_one(&mut *tx).await?;
    let now = Utc::now().timestamp_millis();
    let node_rows = sqlx::query("SELECT * FROM nodes ORDER BY node_id LIMIT 2049").fetch_all(&mut *tx).await?;
    let object_rows = sqlx::query("SELECT * FROM objects ORDER BY object_id LIMIT 50001").fetch_all(&mut *tx).await?;
    let latest = sqlx::query("SELECT object_id,sample_json FROM latest_samples ORDER BY object_id,name,observed_at DESC,source,scope,labels_json LIMIT 200001").fetch_all(&mut *tx).await?;
    if node_rows.len() > 2048 || object_rows.len() > 50000 || latest.len() > 200000 { return Err(unavailable("Current snapshot exceeds this release's bounded read capacity")); }
    let mut events = Vec::new();
    if include_events {
        let to = parse_time(query, "to", now)?; let from = parse_time(query, "from", now - 3_600_000)?;
        if from >= to || to > now + 300_000 { return Err(query_error("from/to", "from must precede to; to cannot exceed server time by 300 seconds")); }
        let rows = sqlx::query("SELECT * FROM events WHERE occurred_at>=? AND occurred_at<? AND (? IS NULL OR node_id=?) AND (? IS NULL OR object_id=?) AND (? IS NULL OR json_extract(event_json,'$.category')=?) AND (? IS NULL OR json_extract(event_json,'$.severity')=?) ORDER BY occurred_at DESC,event_id DESC LIMIT 10001")
            .bind(from).bind(to).bind(query.get("node_id")).bind(query.get("node_id")).bind(query.get("object_id")).bind(query.get("object_id"))
            .bind(query.get("category")).bind(query.get("category")).bind(query.get("severity")).bind(query.get("severity")).fetch_all(&mut *tx).await?;
        if rows.len() > 10000 { return Err(query_error("from/to", "Narrow the event window or filters to at most 10000 events")); }
        for row in rows {
            let mut event = value(&row, "event_json")?;
            event["event_id"] = json!(row.get::<i64, _>("event_id").to_string());
            event["node_id"] = json!(row.get::<String, _>("node_id"));
            event["received_at"] = json!(store::timestamp(row.get("received_at")));
            event["details"] = json!({});
            events.push(event);
        }
    }
    let inventory_times = sqlx::query("SELECT node_id,MAX(received_at) AS updated FROM inventory_generations GROUP BY node_id").fetch_all(&mut *tx).await?;
    tx.commit().await?;
    let metadata: HashMap<String, (Option<String>, String)> = node_rows.iter().map(|r| (r.get("node_id"), (r.get("boot_id"), r.get::<i64, _>("inventory_generation").to_string()))).collect();
    let mut by_object: HashMap<String, Vec<Value>> = HashMap::new();
    for row in latest { by_object.entry(row.get("object_id")).or_default().push(value(&row, "sample_json")?); }
    let mut objects = Vec::new();
    for row in object_rows {
        let object_id: String = row.get("object_id"); let node_id: String = row.get("node_id"); let active = row.get::<i64, _>("active") != 0;
        let Some((boot, generation)) = metadata.get(&node_id) else { continue; };
        let mut metrics: serde_json::Map<String, Value> = serde_json::Map::new(); let mut series = Vec::new();
        for raw in by_object.remove(&object_id).unwrap_or_default() {
            let name = raw["sample"]["name"].as_str().unwrap_or("").to_string();
            let measurement = from_latest(&raw, now, active, boot.as_deref(), generation);
            if let Some(previous) = metrics.get_mut(&name) {
                *previous = unknown(measurement["kind"].as_str().unwrap_or("gauge"), measurement["unit"].as_str().unwrap_or("unknown"));
                previous["ambiguity"] = json!("multiple_series");
            } else { metrics.insert(name.clone(), measurement.clone()); }
            series.push(json!({"name":name,"measurement":measurement}));
        }
        objects.push(json!({"object_id":object_id,"node_id":node_id,"local_id":row.get::<String,_>("local_id"),"kind":row.get::<String,_>("kind"),
            "parent_ids":value(&row,"parents_json")?,"properties":value(&row,"properties_json")?,"inventory_generation":row.get::<i64,_>("generation").to_string(),"active":active,
            "latest_metrics":metrics,"latest_metric_series":series,"health":health("unknown",&Value::Null)}));
    }
    let mut nodes = Vec::new();
    for row in node_rows {
        let id: String = row.get("node_id");
        let owned: Vec<_> = objects.iter().filter(|o| o["node_id"] == id && o["active"] == true).collect();
        let devices: Vec<_> = owned.iter().copied().filter(|o| o["kind"] == "device").collect();
        let root = owned.iter().find(|o| o["kind"] == "node").copied();
        let agent = value(&row, "agent_json")?; let last_seen: Option<i64> = row.get("last_seen_at");
        let age = last_seen.map(|at| (now - at).max(0) as f64 / 1000.0);
        let availability = if row.get::<Option<i64>, _>("goodbye_at").is_some() || row.get::<Option<i64>, _>("revoked_at").is_some() || age.is_some_and(|a| a >= 90.0) { "offline" }
            else if age.is_some_and(|a| a >= 30.0) { "degraded" } else if age.is_some() { "online" } else { "unknown" };
        let cap = capacity(&local_capacity_objects(&owned));
        let temperatures: Vec<Value> = devices.iter().map(|o| metric(o,"device_temperature_celsius","celsius")).collect();
        let temperature = temperatures.into_iter().filter(|m| number(m).is_some()).max_by(|a,b| number(a).unwrap_or_default().total_cmp(&number(b).unwrap_or_default())).unwrap_or_else(|| unknown("gauge","celsius"));
        let mut node = json!({"node_id":id,"name":row.get::<String,_>("name"),"model":root.map(|o|o["properties"]["model"].clone()).unwrap_or(Value::Null),
            "os_version":root.map(|o|metric(o,"node_os_version","string")["value"].clone()).unwrap_or(Value::Null),"agent_version":agent["version"],
            "inventory_generation":row.get::<i64,_>("inventory_generation").to_string(),"last_seen_at":last_seen.map(store::timestamp),"last_seen_age_seconds":age,"heartbeat_interval_seconds":5,
            "availability":availability,"health":health(availability,&cap),"active_alert_count":null,"alerts_available":false,"capacity":cap,
            "read_bytes_per_second":aggregate(devices.iter().map(|o|rate(o,"device_read_bytes_total")).collect(),"bytes/second",false),
            "write_bytes_per_second":aggregate(devices.iter().map(|o|rate(o,"device_write_bytes_total")).collect(),"bytes/second",false),"temperature_celsius":temperature,
            "capabilities":["inventory","telemetry"],"enrolled_at":store::timestamp(row.get("enrolled_at")),
            "inventory_updated_at":inventory_times.iter().find(|r|r.get::<String,_>("node_id")==id).map(|r|store::timestamp(r.get("updated"))),
            "inventory_url":format!("/api/v1/nodes/{id}/inventory")});
        if availability != "online" {
            for key in ["read_bytes_per_second", "write_bytes_per_second", "temperature_celsius"] { node[key]["state"] = json!("stale"); }
            for key in ["capacity_bytes", "used_bytes", "free_bytes", "available_bytes", "used_ratio"] { node["capacity"][key]["state"] = json!("stale"); }
        }
        nodes.push(node);
    }
    let online_ids: BTreeSet<_> = nodes.iter().filter(|n| n["availability"] == "online").filter_map(|n| n["node_id"].as_str()).collect();
    let active: Vec<_> = objects.iter().filter(|o| o["active"] == true && online_ids.contains(o["node_id"].as_str().unwrap_or(""))).collect();
    let local = local_capacity_objects(&active); let mut shared = BTreeMap::new(); let mut unresolved = 0;
    for object in objects.iter().filter(|o|o["active"] == true && o["kind"] == "nfs_mount") {
        let properties = &object["properties"];
        if properties["shared_filesystem_authoritative"] == true {
            if let Some(id) = properties["shared_filesystem_id"].as_str().filter(|id|!id.is_empty()) {
                if online_ids.contains(object["node_id"].as_str().unwrap_or("")) { shared.entry(id.to_owned()).or_insert(object); }
                continue;
            }
        }
        unresolved += 1;
    }
    let mut counts = json!({"total":nodes.len(),"online":0,"degraded":0,"offline":0,"unknown":0});
    for node in &nodes { let key = node["availability"].as_str().unwrap_or("unknown"); counts[key] = json!(counts[key].as_u64().unwrap_or(0)+1); }
    let local_cap = capacity(&local);
    let contributors: BTreeSet<_> = local.iter().filter(|o| integer(&metric(o,"capacity_bytes","bytes")).is_some()).filter_map(|o|o["node_id"].as_str()).collect();
    let excluded: Vec<_> = nodes.iter().filter_map(|n|n["node_id"].as_str()).filter(|id|!contributors.contains(id)).collect();
    let cluster_id = Uuid::new_v5(&Uuid::NAMESPACE_OID, app.admin_hash.as_bytes()).to_string();
    let cluster_health = ["critical","warning","healthy"].into_iter().find(|status|nodes.iter().any(|n|n["health"]["overall"]==*status)).unwrap_or("unknown");
    let mut overall_health = health("unknown",&local_cap); overall_health["overall"] = json!(cluster_health);
    let cluster = json!({"cluster_id":cluster_id,"node_counts":counts,"health":overall_health,"capacity":{"local":local_cap,"shared":capacity(&shared.into_values().collect::<Vec<_>>()),
        "unresolved_shared_mounts":unresolved,"contributing_node_ids":contributors,"excluded_node_ids":excluded},
        "throughput":{"read_bytes_per_second":aggregate(nodes.iter().map(|n|n["read_bytes_per_second"].clone()).collect(),"bytes/second",false),"write_bytes_per_second":aggregate(nodes.iter().map(|n|n["write_bytes_per_second"].clone()).collect(),"bytes/second",false)},
        "active_alert_counts":{"warning":null,"critical":null},"alerts_available":false,"observed_node_count":online_ids.len(),"expected_node_count":nodes.len()});
    let filesystems = objects.iter().filter(|o| o["active"] == true && matches!(o["kind"].as_str(), Some("apfs_volume"|"mount"|"nfs_mount"))).map(|object| {
        let p = &object["properties"];
        let kind = p["filesystem_type"].as_str().unwrap_or(if object["kind"] == "nfs_mount" {"nfs"} else if object["kind"] == "apfs_volume" {"apfs"} else {"unknown"});
        let classification = if object["kind"] == "nfs_mount" || matches!(kind,"nfs"|"nfs4") {"network"} else if p["removable"] == true {"removable"} else {"local"};
        json!({"object_id":object["object_id"],"node_id":object["node_id"],"filesystem_type":kind,"classification":classification,
            "mount_point":p["mount_point"],"shared_filesystem_id":p["shared_filesystem_id"],"capacity":capacity(&[object]),
            "quota":{"limit_bytes":metric(object,"quota_limit_bytes","bytes"),"used_bytes":metric(object,"quota_used_bytes","bytes"),"available_bytes":metric(object,"quota_available_bytes","bytes")},
            "mount_status":metric(object,"nfs_mount_status","enum"),"health":object["health"],"inventory_generation":object["inventory_generation"]})
    }).collect();
    Ok(Snapshot {time:now,change:change.to_string(),nodes,objects,filesystems,events,cluster})
}
