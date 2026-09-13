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
    "/api/v1/nodes/{node_id}/inventory", "/api/v1/nodes/{node_id}/disks", "/api/v1/objects/{object_id}",
    "/api/v1/filesystems", "/api/v1/events", "/api/v1/capabilities",
    "/api/v1/findings", "/api/v1/findings/{finding_id}",
    "/api/v1/detectors/storage-activity/sources",
    "/api/v1/security/rule-sources", "/api/v1/security/rule-findings", "/api/v1/security/rule-findings/{finding_id}",
    "/api/v1/reliability/sources", "/api/v1/reliability/findings", "/api/v1/reliability/findings/{finding_id}",
    "/api/v1/attention", "/api/v1/attention/summary", "/api/v1/notifications/settings",
    "/api/v1/quotas", "/api/v1/diagnostics",
    "/api/v1/objects/{object_id}/history", "/api/v1/objects/{object_id}/capacity-forecast",
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
    expires: i64, time: i64, change: String, values: Arc<Vec<Value>>, bytes: usize, metadata: Value,
}
struct Snapshot {
    time: i64, change: String, nodes: Vec<Value>, objects: Vec<Value>,
    filesystems: Vec<Value>, events: Vec<Value>, cluster: Value, disks: BTreeMap<String, crate::disk_view::DiskIndex>,
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
        .route("/cidar-logo.png", get(brand_logo))
        .route("/css/console.css", get(console_css))
        .route("/css/live.css", get(live_css))
        .route("/css/fonts.css", get(fonts_css))
        .route("/js/{asset}", get(javascript))
}

async fn index() -> Response { asset("text/html; charset=utf-8", include_str!("../../web/index.html")) }
async fn brand_logo() -> Response {
    ([(axum::http::header::CONTENT_TYPE, "image/png")], include_bytes!("../../web/cidar-logo.png").as_slice()).into_response()
}
async fn console_css() -> Response { asset("text/css; charset=utf-8", include_str!("../../web/css/console.css")) }
async fn live_css() -> Response { asset("text/css; charset=utf-8", include_str!("../../web/css/live.css")) }
async fn fonts_css() -> Response { asset("text/css; charset=utf-8", include_str!("../../web/css/fonts.css")) }
async fn javascript(Path(name): Path<String>) -> Response {
    let body = match name.as_str() {
        "app.js" => include_str!("../../web/js/app.js"),
        "operator.js" => include_str!("../../web/js/operator.js"),
        "operator-views.js" => include_str!("../../web/js/operator-views.js"),
        "storage.js" => include_str!("../../web/js/storage.js"),
        "storage-views.js" => include_str!("../../web/js/storage-views.js"),
        "security.js" => include_str!("../../web/js/security.js"),
        "security-views.js" => include_str!("../../web/js/security-views.js"),
        "reliability.js" => include_str!("../../web/js/reliability.js"),
        "reliability-views.js" => include_str!("../../web/js/reliability-views.js"),
        "rack.js" => include_str!("../../web/js/rack.js"),
        "api.js" => include_str!("../../web/js/api.js"),
        "session.js" => include_str!("../../web/js/session.js"),
        "vendor-preact-htm.js" => include_str!("../../web/js/vendor-preact-htm.js"),
        "vendor-three.js" => include_str!("../../web/js/vendor-three.js"),
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
    response.headers_mut().insert("content-security-policy", HeaderValue::from_static("default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; font-src 'self' data:; img-src 'self' data:; connect-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'"));
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

fn response(data: Value, time: i64, change: &str, next: Option<String>, request_id: &str, metadata: &Value) -> Response {
    let mut meta=json!({"api_version":"1","server_time":store::timestamp(time),"request_id":request_id,"snapshot_cursor":change,"next_cursor":next});
    if let Some(fields)=metadata.as_object() {meta.as_object_mut().expect("meta object").extend(fields.clone());}
    Json(json!({"data":data,"meta":meta})).into_response()
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
    if resource == "reliability" {
        return reliability_read(state, path, &parts, query, owner, request_id).await;
    }
    if matches!(resource, "findings" | "detectors") {
        return detection_read(state, path, &parts, query, owner, request_id).await;
    }
    if resource == "security" {
        return security_rule_read(state, path, &parts, query, owner, request_id).await;
    }
    if matches!(resource, "attention" | "notifications" | "quotas" | "diagnostics") {
        return operator_read(state, path, query, owner, request_id).await;
    }
    let id = parts.get(3).copied();
    if let Some(id) = id { Uuid::parse_str(id).map_err(|_| query_error("id", "Expected a resource UUID"))?; }
    let subresource = parts.get(4).copied();
    if resource == "objects" && matches!(subresource, Some("history" | "capacity-forecast")) {
        let now = Utc::now().timestamp_millis();
        let (data, meta) = if subresource == Some("history") {
            crate::history::read(&state.app, id.ok_or_else(not_found)?, &query, now).await?
        } else {
            if let Some(key) = query.keys().next() {return Err(query_error(key, "Unknown query parameter"));}
            (crate::history::forecast(&state.app, id.ok_or_else(not_found)?, now).await?, json!({}))
        };
        return Ok(response(data, now, &change_cursor(&state.app).await?, None, request_id, &meta));
    }
    let inventory = resource == "nodes" && subresource == Some("inventory");
    let disks = resource == "nodes" && subresource == Some("disks");
    let list = inventory || disks || (id.is_none() && matches!(resource, "nodes" | "filesystems" | "events"));
    let mut allowed = if list { vec!["limit", "cursor"] } else { vec![] };
    if inventory { allowed.extend(["generation", "kind", "disk_id"]); }
    else if disks { allowed.push("generation"); }
    else if resource == "nodes" && list { allowed.extend(["availability", "health", "q"]); }
    else if resource == "filesystems" { allowed.extend(["node_id", "classification", "filesystem_type", "health"]); }
    else if resource == "events" { allowed.extend(["node_id", "object_id", "category", "severity", "from", "to"]); }
    for key in query.keys() { if !allowed.contains(&key.as_str()) { return Err(query_error(key, "Unknown query parameter")); } }
    validate_enum(&query, "availability", &["online", "degraded", "offline", "unknown"])?;
    validate_enum(&query, "health", &["healthy", "warning", "critical", "unknown"])?;
    validate_enum(&query, "kind", &["node", "device", "partition", "apfs_store", "apfs_container", "apfs_volume", "snapshot", "mount", "nfs_mount", "quota", "provider"])?;
    validate_enum(&query, "classification", &["local", "network", "removable"])?;
    validate_enum(&query, "category", &["inventory", "availability", "storage", "security", "collector", "alert"])?;
    validate_enum(&query, "severity", &["info", "warning", "critical"])?;
    for key in ["q", "filesystem_type"] {
        if query.get(key).is_some_and(|v| v.chars().count() > 128) { return Err(query_error(key, "Maximum length is 128 characters")); }
    }
    if query.contains_key("disk_id") && query.contains_key("kind") { return Err(query_error("disk_id/kind", "Disk traversal cannot be combined with kind filtering")); }
    for key in ["node_id", "object_id", "disk_id"] {
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
    let metadata = if inventory || disks {
        let node=snapshot.nodes.iter().find(|n|n["node_id"]==id.unwrap_or("")).ok_or_else(not_found)?;
        let index=&snapshot.disks[id.unwrap_or("")];
        json!({"node_id":node["node_id"],"boot_id":node["boot_id"],"agent_session_id":node["agent_session_id"],"inventory_generation":node["inventory_generation"],"topology_revision":index.topology_revision,"disk_inventory":index.disk_inventory})
    } else {json!({})};
    let data = match (resource, id, subresource) {
        ("cluster", None, _) => snapshot.cluster.clone(),
        ("capabilities", None, _) => json!({
            "api_version":"1","service_version":env!("CARGO_PKG_VERSION"),"heartbeat_interval_seconds":5,
            "availability_thresholds_seconds":{"degraded":30,"offline":90},"poll_interval_seconds":5,"hidden_poll_interval_seconds":30,
            "implemented_read_routes":READ_ROUTES,"features":{"current_inventory":true,"physical_disks":true,"typed_storage_topology":true,"hardware_models":true,"historical_inventory":false,"events":true,"storage_activity_detection":true,"filesystem_nfs_rule_detection":true,"drive_reliability":true,"reliability_findings":true,"replacement_forecasts":false,"findings":true,"alerts":true,"twilio_notifications":true,"notification_configuration":"administrator_dashboard","nfs_user_quotas":true,"diagnostics_readiness":true,"capacity_forecasts":true,"sse":false,"metric_history":true,"prometheus":false,"openapi":false},
            "viewer_credential_expires_at":store::timestamp(state.app.viewer_expires_at),"viewer_scope":"telemetry:read","browser_transport":"same_origin",
            "limits":{"default_page_size":100,"maximum_page_size":500,"cursor_ttl_seconds":300,"read_requests_per_minute":120,"read_burst":20}
        }),
        ("nodes", Some(id), Some("inventory" | "disks")) => {
            let node = snapshot.nodes.iter().find(|n| n["node_id"] == id).ok_or_else(not_found)?;
            if let Some(generation) = query.get("generation") {
                let parsed = generation.parse::<u64>().map_err(|_| query_error("generation", "Expected a decimal generation"))?;
                if parsed.to_string() != node["inventory_generation"].as_str().unwrap_or("") {
                    return Err(query_error("generation", "This release exposes current inventory only"));
                }
            }
            let index=&snapshot.disks[id];
            if disks {Value::Array(index.disks.clone())} else {
                if let Some(disk)=query.get("disk_id") {if !index.disks.iter().any(|d|d["object_id"]==*disk) {return Err(not_found());}}
                Value::Array(snapshot.objects.iter().filter(|o| o["node_id"] == id && o["active"] == true && matches_filter(o, "kind", &query)
                    && query.get("disk_id").is_none_or(|disk|o["object_id"]==*disk || o["physical_disk_ids"].as_array().is_some_and(|ids|ids.iter().any(|v|v==disk)))).cloned().collect())
            }
        },
        ("nodes", Some(id), None) => snapshot.nodes.iter().find(|n| n["node_id"] == id).cloned().ok_or_else(not_found)?,
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
    if !list { return Ok(response(data, snapshot.time, &snapshot.change, None, request_id, &metadata)); }
    list_response(state, path, query, owner, data, snapshot.time, snapshot.change, limit, request_id, metadata)
}

async fn security_rule_read(state: &ReadState, path: &str, parts: &[&str], mut query: Parameters,
    owner: String, request_id: &str) -> ApiResult<Response> {
    let sources = path == "/api/v1/security/rule-sources";
    let finding_id = if parts.get(3) == Some(&"rule-findings") { parts.get(4).copied() } else { None };
    let list = sources || (parts.len() == 4 && parts[3] == "rule-findings");
    if !sources && !list && finding_id.is_none() { return Err(not_found()); }
    if let Some(id)=finding_id { Uuid::parse_str(id).map_err(|_|query_error("finding_id","Expected a finding UUID"))?; }
    let allowed:&[&str]=if sources { &["node_id","object_id","limit","cursor"] } else if list { &["node_id","object_id","status","limit","cursor"] } else { &[] };
    for key in query.keys(){if !allowed.contains(&key.as_str()){return Err(query_error(key,"Unknown query parameter"));}}
    for key in ["node_id","object_id"]{if let Some(id)=query.get(key){Uuid::parse_str(id).map_err(|_|query_error(key,"Expected a resource UUID"))?;}}
    validate_enum(&query,"status",&["open","resolved","interrupted","all"])?;
    let limit=match query.get("limit"){Some(n)=>n.parse::<usize>().ok().filter(|n|(1..=500).contains(n)).ok_or_else(||query_error("limit","Expected 1-500"))?,None=>100};
    if let Some(cursor)=query.remove("cursor"){return page(state,path,&query,&owner,&cursor,limit,request_id);}
    let mut tx=state.app.db.begin().await?;
    let change:i64=sqlx::query_scalar("SELECT COALESCE(MAX(change_id),0) FROM change_log").fetch_one(&mut *tx).await?;
    let now=Utc::now().timestamp_millis();
    let values=if sources {
        crate::security_rules::source_summaries(&mut tx,query.get("node_id").map(String::as_str),query.get("object_id").map(String::as_str)).await?
    } else {
        let status=query.get("status").map(String::as_str).unwrap_or("open");
        let rows=sqlx::query("SELECT finding_json FROM security_rule_findings WHERE (? IS NULL OR finding_id=?) AND (? IS NOT NULL OR ?='all' OR status=?) AND (? IS NULL OR node_id=?) AND (? IS NULL OR object_id=?) ORDER BY updated_at DESC,finding_id DESC LIMIT 10001")
            .bind(finding_id).bind(finding_id).bind(finding_id).bind(status).bind(status).bind(query.get("node_id")).bind(query.get("node_id")).bind(query.get("object_id")).bind(query.get("object_id")).fetch_all(&mut *tx).await?;
        if rows.len()>10000{return Err(query_error("filters","Narrow the filters to at most 10000 findings"));}
        rows.iter().map(|r|value(r,"finding_json")).collect::<ApiResult<Vec<_>>>()?
    };
    tx.commit().await?;
    if list { list_response(state,path,query,owner,json!(values),now,change.to_string(),limit,request_id,json!({"policy_version":crate::security_rules::POLICY_VERSION})) }
    else { Ok(response(values.into_iter().next().ok_or_else(not_found)?,now,&change.to_string(),None,request_id,&json!({}))) }
}

async fn change_cursor(app: &AppState) -> ApiResult<String> {
    let id: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(change_id),0) FROM change_log").fetch_one(&app.db).await?;
    Ok(id.to_string())
}

async fn operator_read(state: &ReadState, path: &str, mut query: Parameters, owner: String, request_id: &str) -> ApiResult<Response> {
    let allowed: &[&str] = match path {
        "/api/v1/attention" => &["node_id","object_id","status","kind","severity","acknowledged","limit","cursor"],
        "/api/v1/quotas" => &["node_id","uid","limit","cursor"],
        "/api/v1/diagnostics" => &["node_id","limit","cursor"],
        "/api/v1/attention/summary" | "/api/v1/notifications/settings" => &[],
        _ => return Err(not_found()),
    };
    for key in query.keys() {if !allowed.contains(&key.as_str()) {return Err(query_error(key,"Unknown query parameter"));}}
    for key in ["node_id","object_id"] {if let Some(id)=query.get(key) {Uuid::parse_str(id).map_err(|_|query_error(key,"Expected a resource UUID"))?;}}
    validate_enum(&query,"status",&["open","resolved","all"])?;
    validate_enum(&query,"severity",&["info","warning","critical"])?;
    validate_enum(&query,"acknowledged",&["true","false"])?;
    validate_enum(&query,"kind",&["activity","reliability","capacity","node_loss","filesystem","drive_removal"])?;
    if let Some(uid)=query.get("uid") {
        if uid.parse::<u32>().ok().filter(|n|*n<=i32::MAX as u32 && n.to_string()==*uid).is_none() {return Err(query_error("uid","Expected a canonical UID in 0..2147483647"));}
    }
    let limit = query.get("limit").map(|v|v.parse::<usize>().ok().filter(|n|(1..=500).contains(n)))
        .unwrap_or(Some(100)).ok_or_else(||query_error("limit","Expected 1-500"))?;
    if let Some(cursor)=query.remove("cursor") {return page(state,path,&query,&owner,&cursor,limit,request_id);}
    let now=Utc::now().timestamp_millis();
    let (data,meta,list)=match path {
        "/api/v1/attention" => {let (d,m)=crate::attention::list(&state.app,&query,now).await?;(d,m,true)},
        "/api/v1/quotas" => {let (d,m)=crate::quota::list(&state.app,&query,now).await?;(d,m,true)},
        "/api/v1/diagnostics" => {let (d,m)=crate::diagnostics::list(&state.app,&query,now).await?;(d,m,true)},
        "/api/v1/attention/summary" => (crate::attention::summary(&state.app,now).await?,json!({}),false),
        "/api/v1/notifications/settings" => (crate::notifications::settings(&state.app).await?,json!({}),false),
        _ => return Err(not_found()),
    };
    let change=change_cursor(&state.app).await?;
    if list {list_response(state,path,query,owner,data,now,change,limit,request_id,meta)}
    else {Ok(response(data,now,&change,None,request_id,&meta))}
}

// All collections share the same bounded frozen-page cache and credential scope.
#[allow(clippy::too_many_arguments)]
fn list_response(state: &ReadState, path: &str, query: Parameters, owner: String, data: Value,
    time: i64, change: String, limit: usize, request_id: &str, metadata: Value) -> ApiResult<Response> {
    let values = data.as_array().expect("list route");
    let next = if values.len() > limit {
        let bytes = data.to_string().len() + metadata.to_string().len();
        let mut pages = state.pages.lock().map_err(|_| unavailable("Pagination unavailable"))?;
        pages.retain(|_, p| p.expires > Utc::now().timestamp_millis());
        if pages.len() >= 128 || bytes + pages.values().map(|p| p.bytes).sum::<usize>() > 32 * 1024 * 1024 {
            return Err(ApiError::new(StatusCode::TOO_MANY_REQUESTS, "rate_limited", "Pagination cache is full; retry later"));
        }
        let key = Uuid::new_v4().to_string();
        let expires = time + 300_000;
        pages.insert(key.clone(), Page { owner, path: path.into(), query, expires, time, change: change.clone(), values: Arc::new(values.clone()), bytes, metadata: metadata.clone() });
        Some(format!("{key}.{expires}.{limit}"))
    } else { None };
    Ok(response(json!(values.iter().take(limit).collect::<Vec<_>>()), time, &change, next, request_id, &metadata))
}

async fn detection_read(state: &ReadState, path: &str, parts: &[&str], mut query: Parameters,
    owner: String, request_id: &str) -> ApiResult<Response> {
    let sources = path == "/api/v1/detectors/storage-activity/sources";
    let finding_id = if parts.get(2) == Some(&"findings") { parts.get(3).copied() } else { None };
    let list = sources || (parts.len() == 3 && parts[2] == "findings");
    if let Some(id) = finding_id { Uuid::parse_str(id).map_err(|_| query_error("finding_id", "Expected a finding UUID"))?; }
    let allowed: &[&str] = if sources { &["node_id", "object_id", "limit", "cursor"] }
        else if list { &["node_id", "object_id", "status", "from", "to", "limit", "cursor"] } else { &[] };
    for key in query.keys() { if !allowed.contains(&key.as_str()) { return Err(query_error(key, "Unknown query parameter")); } }
    for key in ["node_id", "object_id"] { if let Some(id) = query.get(key) { Uuid::parse_str(id).map_err(|_| query_error(key, "Expected a resource UUID"))?; } }
    validate_enum(&query, "status", &["open", "resolved", "interrupted", "all"])?;
    let limit = match query.get("limit") {
        Some(n) => n.parse::<usize>().ok().filter(|n| (1..=500).contains(n)).ok_or_else(|| query_error("limit", "Expected 1-500"))?,
        None => 100,
    };
    // Validate time filters even on subsequent page requests; absent bounds keep
    // old open episodes visible. A half-open range applies to first_seen_at.
    let from = parse_time(&query, "from", i64::MIN)?;
    let to = parse_time(&query, "to", i64::MAX)?;
    if from >= to { return Err(query_error("from/to", "from must precede to")); }
    if let Some(cursor) = query.remove("cursor") { return page(state, path, &query, &owner, &cursor, limit, request_id); }
    let mut tx = state.app.db.begin().await?;
    let change: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(change_id),0) FROM change_log").fetch_one(&mut *tx).await?;
    let now = Utc::now().timestamp_millis();
    let (data, metadata) = if sources {
        let values = crate::detection_store::source_summaries_filtered(&mut tx, now,
            query.get("node_id").map(String::as_str), query.get("object_id").map(String::as_str)).await?;
        let count = |predicate: fn(&Value) -> bool| values.iter().filter(|v| predicate(v)).count();
        let coverage = json!({"known_sources":values.len(),"active_sources":count(|v|v["active"]==true),
            "supported_sources":count(|v|v["active"]==true && v["support_state"]=="supported"),
            "capacity_limited_sources":count(|v|v["active"]==true && v["support_state"]=="capacity_limited"),
            "learning_sources":count(|v|v["active"]==true && v["support_state"]=="supported" && v["baseline"]["state"]=="learning"),
            "ready_sources":count(|v|v["active"]==true && v["support_state"]=="supported" && v["baseline"]["state"]=="ready"),
            "current_sources":count(|v|v["active"]==true && v["observation"]["state"]=="current"),
            "security_assessment":"unknown","inventory_completeness":"unknown"});
        (json!(values), json!({"policy":crate::detection::Policy::default(),"coverage":coverage}))
    } else {
        let status = query.get("status").map(String::as_str).unwrap_or("open");
        let rows = sqlx::query("SELECT finding_json FROM detection_findings WHERE (? IS NULL OR finding_id=?) AND (? IS NOT NULL OR ?='all' OR status=?) AND (? IS NULL OR node_id=?) AND (? IS NULL OR object_id=?) AND first_seen_at>=? AND first_seen_at<? ORDER BY first_seen_at DESC,finding_id DESC LIMIT 10001")
            .bind(finding_id).bind(finding_id).bind(finding_id).bind(status).bind(status)
            .bind(query.get("node_id")).bind(query.get("node_id")).bind(query.get("object_id")).bind(query.get("object_id"))
            .bind(from).bind(to).fetch_all(&mut *tx).await?;
        if rows.len() > 10000 { return Err(query_error("filters", "Narrow the filters to at most 10000 findings")); }
        let values = rows.iter().map(|r| value(r, "finding_json")).collect::<ApiResult<Vec<_>>>()?;
        (if list {json!(values)} else {values.into_iter().next().ok_or_else(not_found)?}, json!({}))
    };
    tx.commit().await?;
    if list { list_response(state, path, query, owner, data, now, change.to_string(), limit, request_id, metadata) }
    else { Ok(response(data, now, &change.to_string(), None, request_id, &metadata)) }
}

async fn reliability_read(
    state: &ReadState,
    path: &str,
    parts: &[&str],
    mut query: Parameters,
    owner: String,
    request_id: &str,
) -> ApiResult<Response> {
    let sources = path == "/api/v1/reliability/sources";
    let finding_id = if parts.get(3) == Some(&"findings") {
        parts.get(4).copied()
    } else {
        None
    };
    let list = sources || (parts.len() == 4 && parts[3] == "findings");
    if let Some(id) = finding_id {
        Uuid::parse_str(id).map_err(|_| query_error("finding_id", "Expected a finding UUID"))?;
    }
    let allowed: &[&str] = if sources {
        &["node_id", "object_id", "limit", "cursor"]
    } else if list {
        &[
            "node_id",
            "object_id",
            "status",
            "from",
            "to",
            "limit",
            "cursor",
        ]
    } else {
        &[]
    };
    for key in query.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(query_error(key, "Unknown query parameter"));
        }
    }
    for key in ["node_id", "object_id"] {
        if let Some(id) = query.get(key) {
            Uuid::parse_str(id).map_err(|_| query_error(key, "Expected a resource UUID"))?;
        }
    }
    validate_enum(
        &query,
        "status",
        &["open", "resolved", "interrupted", "all"],
    )?;
    let limit = match query.get("limit") {
        Some(n) => n
            .parse::<usize>()
            .ok()
            .filter(|n| (1..=500).contains(n))
            .ok_or_else(|| query_error("limit", "Expected 1-500"))?,
        None => 100,
    };
    // Validate time filters even on subsequent page requests; absent bounds keep
    // old open episodes visible. A half-open range applies to first_seen_at.
    let from = parse_time(&query, "from", i64::MIN)?;
    let to = parse_time(&query, "to", i64::MAX)?;
    if from >= to {
        return Err(query_error("from/to", "from must precede to"));
    }
    if let Some(cursor) = query.remove("cursor") {
        return page(state, path, &query, &owner, &cursor, limit, request_id);
    }
    let mut tx = state.app.db.begin().await?;
    let change: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(change_id),0) FROM change_log")
        .fetch_one(&mut *tx)
        .await?;
    let now = Utc::now().timestamp_millis();
    let (data, metadata) = if sources {
        let values = crate::reliability_store::source_summaries_filtered(
            &mut tx,
            now,
            query.get("node_id").map(String::as_str),
            query.get("object_id").map(String::as_str),
        )
        .await?;
        let count = |predicate: fn(&Value) -> bool| values.iter().filter(|v| predicate(v)).count();
        let coverage = json!({"known_sources":values.len(),"active_sources":count(|v|v["active"]==true),
            "supported_sources":count(|v|v["active"]==true && v["support_state"]=="supported"),
            "capacity_limited_sources":count(|v|v["active"]==true && v["support_state"]=="capacity_limited"),
            "current_sources":count(|v|v["active"]==true && v["observation"]["state"]=="current"),
            "assessment":"unknown","inventory_completeness":"unknown"});
        (
            json!(values),
            json!({"policy":crate::reliability::Policy::default(),"coverage":coverage}),
        )
    } else {
        let status = query.get("status").map(String::as_str).unwrap_or("open");
        let (count, bytes):(i64,i64) = sqlx::query_as("SELECT COUNT(*),COALESCE(SUM(length(CAST(finding_json AS BLOB))),0) FROM (SELECT finding_json FROM reliability_findings WHERE (? IS NULL OR finding_id=?) AND (? IS NOT NULL OR ?='all' OR status=?) AND (? IS NULL OR node_id=?) AND (? IS NULL OR object_id=?) AND first_seen_at>=? AND first_seen_at<? ORDER BY first_seen_at DESC,finding_id DESC LIMIT 10001)")
            .bind(finding_id).bind(finding_id).bind(finding_id).bind(status).bind(status)
            .bind(query.get("node_id")).bind(query.get("node_id")).bind(query.get("object_id")).bind(query.get("object_id"))
            .bind(from).bind(to).fetch_one(&mut *tx).await?;
        if count > 10000 {
            return Err(query_error(
                "filters",
                "Narrow the filters to at most 10000 findings",
            ));
        }
        if bytes > 32 * 1024 * 1024 {
            return Err(unavailable(
                "Finding selection exceeds bounded read capacity; narrow the filters",
            ));
        }
        let rows = sqlx::query("SELECT finding_json FROM reliability_findings WHERE (? IS NULL OR finding_id=?) AND (? IS NOT NULL OR ?='all' OR status=?) AND (? IS NULL OR node_id=?) AND (? IS NULL OR object_id=?) AND first_seen_at>=? AND first_seen_at<? ORDER BY first_seen_at DESC,finding_id DESC LIMIT 10001")
            .bind(finding_id).bind(finding_id).bind(finding_id).bind(status).bind(status)
            .bind(query.get("node_id")).bind(query.get("node_id")).bind(query.get("object_id")).bind(query.get("object_id"))
            .bind(from).bind(to).fetch_all(&mut *tx).await?;
        if rows.len() > 10000 {
            return Err(query_error(
                "filters",
                "Narrow the filters to at most 10000 findings",
            ));
        }
        let values = rows
            .iter()
            .map(|r| value(r, "finding_json"))
            .collect::<ApiResult<Vec<_>>>()?;
        (
            if list {
                json!(values)
            } else {
                values.into_iter().next().ok_or_else(not_found)?
            },
            json!({}),
        )
    };
    tx.commit().await?;
    if list {
        list_response(
            state,
            path,
            query,
            owner,
            data,
            now,
            change.to_string(),
            limit,
            request_id,
            metadata,
        )
    } else {
        Ok(response(
            data,
            now,
            &change.to_string(),
            None,
            request_id,
            &metadata,
        ))
    }
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
    let page = {
        let pages = state.pages.lock().map_err(|_| unavailable("Pagination unavailable"))?;
        pages.get(parts[0]).cloned().ok_or_else(|| ApiError::new(StatusCode::GONE, "cursor_expired", "Snapshot is no longer available; start a new traversal"))?
    };
    if page.path != path || page.query != *query || page.owner != owner || page.expires != expires || offset == 0 || offset >= page.values.len() || offset % limit != 0 {
        return Err(query_error("cursor", "Cursor does not match this traversal"));
    }
    let end = offset.saturating_add(limit).min(page.values.len());
    let next = (end < page.values.len()).then(|| format!("{}.{expires}.{end}", parts[0]));
    Ok(response(json!(&page.values[offset..end]), page.time, &page.change, next, request_id, &page.metadata))
}

pub(crate) fn unknown(kind: &str, unit: &str) -> Value {
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
    out["value"] = if exact {
        let Some(total) = valid.iter().filter_map(|m| integer(m)).try_fold(0u128, u128::checked_add) else {
            out["reason"] = json!("aggregate_overflow");
            return out;
        };
        json!(total.to_string())
    }
        else { json!(valid.iter().filter_map(|m| number(m)).sum::<f64>()) };
    out["state"] = json!("ok");
    out["observed_at"] = json!(valid.iter().filter_map(|m| m["observed_at"].as_str()).min());
    out["received_at"] = json!(valid.iter().filter_map(|m| m["received_at"].as_str()).min());
    out["age_seconds"] = json!(valid.iter().filter_map(|m| m["age_seconds"].as_f64()).reduce(f64::max));
    out
}
pub(crate) fn capacity(objects: &[&Value]) -> Value {
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
fn filesystem_capacity(object: &Value) -> Value {
    let mut out = capacity(&[object]);
    // A filesystem row describes one observer, so preserve its non-current
    // value and source state instead of losing them in a valid-only sum.
    for name in ["capacity_bytes", "used_bytes", "free_bytes", "available_bytes"] {
        let measurement = metric(object, name, "bytes");
        if measurement["state"] != "ok" {
            out[name] = measurement;
            out[name]["coverage"] = json!({"observed":0,"expected":1});
        }
    }
    out
}
fn node_availability(row: &SqliteRow, now: i64) -> &'static str {
    let age = row.get::<Option<i64>, _>("last_seen_at").map(|at| (now - at).max(0));
    if row.get::<Option<i64>, _>("goodbye_at").is_some() || row.get::<Option<i64>, _>("revoked_at").is_some() || age.is_some_and(|age| age >= 90_000) { "offline" }
    else if age.is_some_and(|age| age >= 30_000) { "degraded" }
    else if age.is_some() { "online" } else { "unknown" }
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
    let mut result=json!({"overall":overall,"dimensions":dimensions,"unknown_dimensions":missing});
    finish_assessment(&mut result);
    result
}
fn finish_assessment(health: &mut Value) {
    let incomplete=health["unknown_dimensions"].as_array().is_none_or(|v|!v.is_empty());
    health["assessment_state"]=json!(if incomplete {"incomplete"} else {"complete"});
    health["observed_severity"]=json!(match health["overall"].as_str() {Some("critical")=>"critical",Some("warning")=>"warning",_=>"none"});
    if incomplete && health["overall"]=="healthy" {health["overall"]=json!("unknown");}
}
fn apply_security_warning(health: &mut Value, sources: &[Value]) {
    let evidence: Vec<_> = sources.iter().filter(|source| source["active"] == true
        && source["support_state"] == "supported" && source["episode"]["state"] == "open"
        && source["observation"]["state"] == "current")
        .map(|source| json!({"finding_id":source["episode"]["finding_id"],"source_id":source["source_id"],
            "node_id":source["node_id"],"object_id":source["object_id"],"direction":source["direction"],
            "observed_at":source["observation"]["observed_at"]})).collect();
    if evidence.is_empty() { return; }
    health["dimensions"]["security_activity"] = json!({"status":"warning","reasons":["sustained_storage_activity_deviation"],"evidence":evidence});
    if health["overall"] != "critical" { health["overall"] = json!("warning"); }
    if let Some(unknown) = health["unknown_dimensions"].as_array_mut() { unknown.retain(|v| v != "security_activity"); }
}
fn freshness(name: &str) -> i64 {
    if name.starts_with("device_") && (name.contains("_total") || name.contains("_per_second")) { 15 }
    else if name.starts_with("smart_") || name.starts_with("nvme_") || name.contains("temperature") || name == "node_clock_offset_seconds" { 900 }
    else { 90 }
}
fn from_latest(raw: &Value, now: i64, active: bool, boot: Option<&str>, generation: &str) -> Value {
    let raw = crate::cider_api::read_sample(raw, now);
    let sample = &raw["sample"];
    let observed = sample["observed_at"].as_str().and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok()).map(|d| d.timestamp_millis());
    let age = if raw.get("ciderd").is_some() {
        let received = raw["received_at"].as_str().and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok()).map(|d| d.timestamp_millis());
        raw["ciderd"]["age_at_receipt_seconds"].as_f64().zip(received)
            .map(|(age, at)| age + (now - at).max(0) as f64 / 1000.0)
    } else { observed.map(|at| (now - at).max(0) as f64 / 1000.0) };
    let stale_after = raw["ciderd"]["stale_after_seconds"].as_u64()
        .map(|seconds| seconds as f64).unwrap_or_else(|| freshness(sample["name"].as_str().unwrap_or("")) as f64);
    let old_identity = !active || raw["boot_id"].as_str() != boot || raw["inventory_generation"].as_str() != Some(generation);
    let mut state = sample["state"].as_str().unwrap_or("unknown");
    if state == "ok" && (old_identity || age.is_none_or(|age| age > stale_after)) { state = "stale"; }
    let mut val = sample["value"].clone();
    if sample["kind"] == "counter" || sample["unit"] == "bytes" {
        val = if let Some(n) = val.as_u64() { json!(n.to_string()) }
            else if val.as_str().is_some_and(|s| s.parse::<u128>().is_ok()) { val }
            else { Value::Null };
        if val.is_null() && state == "ok" { state = "unknown"; }
    }
    json!({"value":val,"kind":sample["kind"],"unit":sample["unit"],"state":state,
        "source":sample["source"],"scope":raw["scope"],"observed_at":sample["observed_at"],"received_at":raw["received_at"],"age_seconds":age,
        "boot_id":raw["boot_id"],"inventory_generation":raw["inventory_generation"],"labels":sample["labels"],"ciderd":raw["ciderd"],
        "derived_rate_per_second":if state == "ok" {raw["derived_rate_per_second"].clone()} else {Value::Null},"derivation_state":raw["derivation_state"]})
}
pub(crate) fn rate(object: &Value, name: &str) -> Value {
    let raw = metric(object, name, "bytes"); let mut out = raw.clone();
    out["kind"] = json!("gauge"); out["unit"] = json!("bytes/second");
    out["value"] = if raw["state"] == "ok" && raw["derivation_state"] == "ok" { raw["derived_rate_per_second"].clone() } else { Value::Null };
    if out["value"].is_null() && out["state"] == "ok" { out["state"] = json!("unknown"); }
    out
}
pub(crate) fn select_capacity_observer<'a>(selected: &mut BTreeMap<String, &'a Value>, key: String, object: &'a Value) {
    fn quality(object: &Value) -> (bool, usize, f64) {
        let metrics: Vec<_> = ["capacity_bytes", "used_bytes", "free_bytes", "available_bytes"]
            .into_iter().map(|name| metric(object, name, "bytes")).collect();
        let complete = integer(&metrics[0]).zip(integer(&metrics[1])).is_some_and(|(total, used)| used <= total);
        let valid: Vec<_> = metrics.iter().filter(|m| integer(m).is_some()).collect();
        let age = valid.iter().map(|m| m["age_seconds"].as_f64().unwrap_or(f64::INFINITY)).fold(0.0, f64::max);
        (complete, valid.len(), age)
    }
    selected.entry(key).and_modify(|previous| {
        let current = quality(object);
        let old = quality(previous);
        // Keep one coherent observer; never combine its free/used fields with another's.
        if (current.0, current.1) > (old.0, old.1) ||
            ((current.0, current.1) == (old.0, old.1) && current.2 < old.2) {
            *previous = object;
        }
    }).or_insert(object);
}

fn parents_in(object: &Value, parents: &BTreeSet<String>) -> bool {
    object["parent_ids"].as_array().is_some_and(|ids| !ids.is_empty() &&
        ids.iter().all(|id| id.as_str().is_some_and(|id| parents.contains(id))))
}

fn local_capacity_objects<'a>(objects: &[&'a Value]) -> Vec<&'a Value> {
    let mut physical: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for object in objects {
        if let (Some(node), Some(resource_type)) = (object["node_id"].as_str(), object["properties"]["ciderd_resource_type"].as_str()) {
            // An empty set marks a native node whose physical inventory is unavailable.
            let devices = physical.entry(node.into()).or_default();
            if resource_type == "physical_device" && object["properties"]["source"] == "diskutil.list.physical" {
                if let Some(id) = object["object_id"].as_str() { devices.insert(id.into()); }
            }
        }
    }
    let mut media = physical.clone();
    for object in objects {
        if matches!(object["kind"].as_str(), Some("partition" | "apfs_store")) {
            if let Some(node) = object["node_id"].as_str() {
                if physical.get(node).is_some_and(|ids| parents_in(object, ids)) {
                    if let Some(id) = object["object_id"].as_str() { media.entry(node.into()).or_default().insert(id.into()); }
                }
            }
        }
    }
    let mut physical_sources: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for object in objects {
        if let (Some(node), Some(id), Some(bsd)) = (object["node_id"].as_str(), object["object_id"].as_str(), object["properties"]["bsd_name"].as_str()) {
            if media.get(node).is_some_and(|ids| ids.contains(id)) {
                physical_sources.entry(node.into()).or_default().insert(format!("/dev/{bsd}"));
            }
        }
    }
    let mut selected = BTreeMap::new();
    for object in objects {
        let properties = &object["properties"];
        let node = object["node_id"].as_str().unwrap_or("");
        if properties["local"] == false { continue; }
        // Preserve legacy scope only for legacy nodes. Native nodes require proven
        // physical backing even while physical inventory is pending or unavailable.
        let key = if object["kind"] == "apfs_container" {
            if media.get(node).is_some_and(|ids| !parents_in(object, ids)) { continue; }
            object["object_id"].as_str().map(str::to_owned)
        } else if object["kind"] == "mount" && properties["filesystem_type"].as_str().is_some_and(|t| !matches!(t.to_lowercase().as_str(),
            "apfs" | "nfs" | "nfs4" | "smbfs" | "cifs" | "webdav" | "autofs" | "devfs" | "procfs" | "kernfs" | "fdesc" | "fdescfs" | "tmpfs" | "ramfs" | "synthfs")) {
                if physical.contains_key(node) && !properties["source"].as_str()
                    .is_some_and(|source| physical_sources.get(node).is_some_and(|sources| sources.contains(source))) { continue; }
                properties["filesystem_id"].as_str().map(|id| format!("{node}:{id}"))
            } else { None };
        if let Some(key) = key { select_capacity_observer(&mut selected, key, object); }
    }
    selected.into_values().collect()
}
fn parse_time(query: &Parameters, key: &str, default: i64) -> ApiResult<i64> {
    match query.get(key) { None => Ok(default), Some(text) => chrono::DateTime::parse_from_rfc3339(text).map(|d| d.timestamp_millis()).map_err(|_| query_error(key, "Expected an RFC3339 timestamp")) }
}

/// Reuse the same committed, aged object projection for operator features.
pub(crate) async fn current_objects(app: &AppState) -> ApiResult<(Vec<Value>, Vec<Value>)> {
    let view = snapshot(app, false, &Parameters::new()).await?;
    Ok((view.nodes, view.objects))
}

/// Capacity concerns operate on coherent pool/filesystem observers, never an
/// additive list of APFS volumes or all observers of the same NFS filesystem.
pub(crate) async fn attention_capacity_inputs(app: &AppState) -> ApiResult<Vec<Value>> {
    let (_, objects) = current_objects(app).await?;
    let active: Vec<_> = objects.iter().filter(|o| o["active"] == true).collect();
    let mut selected: BTreeMap<String, &Value> = local_capacity_objects(&active).into_iter()
        .map(|o| (local_attention_key(o), o)).collect();
    for object in active.into_iter().filter(|o| o["kind"] == "nfs_mount") {
        let p = &object["properties"];
        if p["shared_filesystem_authoritative"] == true {
            if let Some(id) = p["shared_filesystem_id"].as_str().filter(|id| !id.is_empty()) {
                select_capacity_observer(&mut selected, format!("shared:{id}"), object);
            }
        }
    }
    Ok(selected.into_iter().map(|(key, o)| json!({"key":key,"node_id":o["node_id"],
        "object_id":o["object_id"],"capacity":filesystem_capacity(o)})).collect())
}

fn local_attention_key(object: &Value) -> String {
    if object["kind"]=="mount" {
        if let (Some(node),Some(filesystem))=(object["node_id"].as_str(),object["properties"]["filesystem_id"].as_str()) {
            return format!("local:{node}:{filesystem}");
        }
    }
    format!("local:{}",object["object_id"].as_str().unwrap_or(""))
}

#[test]
fn local_filesystem_attention_identity_survives_observer_selection() {
    let mut a=json!({"object_id":"observer-a","node_id":"node-a","kind":"mount","properties":{"filesystem_id":"same-fs"}});
    let mut b=a.clone();b["object_id"]=json!("observer-b");
    assert_eq!(local_attention_key(&a),local_attention_key(&b));
    b["node_id"]=json!("node-b");assert_ne!(local_attention_key(&a),local_attention_key(&b));
    a["kind"]=json!("apfs_container");b=a.clone();b["object_id"]=json!("other-pool");assert_ne!(local_attention_key(&a),local_attention_key(&b));
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
    let inventory_times = sqlx::query("SELECT node_id,MAX(updated) AS updated FROM (SELECT node_id,received_at AS updated FROM inventory_generations UNION ALL SELECT node_id,json_extract(state_json,'$.inventory_updated_at') AS updated FROM cider_nodes) WHERE updated IS NOT NULL GROUP BY node_id").fetch_all(&mut *tx).await?;
    let native_rows=sqlx::query("SELECT node_id,json_extract(state_json,'$.generation') AS agent_generation,json_extract(state_json,'$.session') AS agent_session_id FROM cider_nodes").fetch_all(&mut *tx).await?;
    let native:BTreeMap<String,Value>=native_rows.iter().map(|r|(r.get("node_id"),json!({"agent_generation":r.get::<String,_>("agent_generation"),"agent_session_id":r.get::<String,_>("agent_session_id")}))).collect();
    let detection_sources = crate::detection_store::current_warning_summaries(&mut tx, now).await?;
    let reliability_sources = crate::reliability_store::active_summaries(&mut tx, now).await?;
    let device_watches = crate::device_watch::summaries(&mut tx,now).await?;
    let device_watch_nodes = crate::device_watch::node_summaries(&mut tx,now).await?;
    let reliability_findings:Vec<Value> = reliability_sources.iter().flat_map(|s|s["findings"].as_array().into_iter().flatten().cloned()).collect();
    let mut detection_by_node: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    let mut detection_by_object: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for source in &detection_sources {
        if source["active"] == true && source["episode"]["state"] == "open" && source["observation"]["state"] == "current" {
            if let Some(id) = source["node_id"].as_str() { detection_by_node.entry(id.into()).or_default().push(source.clone()); }
            if let Some(id) = source["object_id"].as_str() { detection_by_object.entry(id.into()).or_default().push(source.clone()); }
        }
    }
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
    let has_driver_io = devices.iter().any(|device| device["properties"]["throughput_scope"] == "iokit_driver");
    let io_devices: Vec<_> = devices.iter().copied().filter(|device| !has_driver_io || device["properties"]["throughput_scope"] == "iokit_driver").collect();
        let root = owned.iter().find(|o| o["kind"] == "node").copied();
        let agent = value(&row, "agent_json")?; let last_seen: Option<i64> = row.get("last_seen_at");
        let age = last_seen.map(|at| (now - at).max(0) as f64 / 1000.0);
        let availability = node_availability(&row, now);
        let cap = capacity(&local_capacity_objects(&owned));
        let temperatures: Vec<Value> = devices.iter().map(|o| metric(o,"device_temperature_celsius","celsius")).collect();
        let temperature = temperatures.into_iter().filter(|m| number(m).is_some()).max_by(|a,b| number(a).unwrap_or_default().total_cmp(&number(b).unwrap_or_default())).unwrap_or_else(|| unknown("gauge","celsius"));
        let mut node = json!({"node_id":id,"name":row.get::<String,_>("name"),"model":root.map(|o|o["properties"]["model"].clone()).unwrap_or(Value::Null),
            "os_version":agent.get("os_version").cloned().or_else(||root.map(|o|metric(o,"node_os_version","string")["value"].clone())).unwrap_or(Value::Null),"agent_version":agent["version"],
            "inventory_generation":row.get::<i64,_>("inventory_generation").to_string(),"last_seen_at":last_seen.map(store::timestamp),"last_seen_age_seconds":age,"heartbeat_interval_seconds":5,
            "availability":availability,"health":health(availability,&cap),"active_alert_count":null,"alerts_available":false,"capacity":cap,
            "read_bytes_per_second":aggregate(io_devices.iter().map(|o|rate(o,"device_read_bytes_total")).collect(),"bytes/second",false),
            "write_bytes_per_second":aggregate(io_devices.iter().map(|o|rate(o,"device_write_bytes_total")).collect(),"bytes/second",false),"temperature_celsius":temperature,
            "capabilities":["inventory","telemetry"],"enrolled_at":store::timestamp(row.get("enrolled_at")),
            "inventory_updated_at":inventory_times.iter().find(|r|r.get::<String,_>("node_id")==id).map(|r|store::timestamp(r.get("updated"))),
            "inventory_url":format!("/api/v1/nodes/{id}/inventory")});
        node["boot_id"]=json!(row.get::<Option<String>,_>("boot_id"));
        node["device_watch"]=device_watch_nodes.get(&id).cloned().unwrap_or_else(crate::device_watch::unknown_node);
        node["agent_generation"]=native.get(&id).map(|v|v["agent_generation"].clone()).unwrap_or(Value::Null);
        node["agent_session_id"]=native.get(&id).map(|v|v["agent_session_id"].clone()).unwrap_or(Value::Null);
        node["hardware"]=crate::hardware::hardware_view(&root.map(|o|o["properties"].clone()).unwrap_or(Value::Null), &root.map(|o|o["properties"]["ciderd_acquisition"].clone()).unwrap_or(Value::Null), node["boot_id"].as_str());
        if availability != "online" {
            if node["hardware"]["state"]=="ok" {node["hardware"]["state"]=json!("stale");node["hardware"]["reason"]=json!("owner_unavailable");}
            for key in ["read_bytes_per_second", "write_bytes_per_second", "temperature_celsius"] { if node[key]["state"] == "ok" { node[key]["state"] = json!("stale"); } }
            for key in ["capacity_bytes", "used_bytes", "free_bytes", "available_bytes", "used_ratio"] { if node["capacity"][key]["state"] == "ok" { node["capacity"][key]["state"] = json!("stale"); } }
        }
        if let Some(sources) = detection_by_node.get(&id) { apply_security_warning(&mut node["health"], sources); }
        nodes.push(node);
    }
    let online_ids: BTreeSet<_> = nodes.iter().filter(|n| n["availability"] == "online").filter_map(|n| n["node_id"].as_str()).collect();
    // Node summaries retain otherwise-valid source values with a stale summary
    // state. Exposed object/filesystem observations then reflect owner liveness;
    // unavailable owners never contribute to current cluster aggregates.
    for object in &mut objects {
        if !online_ids.contains(object["node_id"].as_str().unwrap_or("")) {
            for measurement in object["latest_metrics"].as_object_mut().expect("metric map").values_mut() {
                if measurement["state"] == "ok" {
                    measurement["state"] = json!("stale");
                    measurement["derived_rate_per_second"] = Value::Null;
                }
            }
            for series in object["latest_metric_series"].as_array_mut().expect("series list") {
                let measurement = &mut series["measurement"];
                if measurement["state"] == "ok" {
                    measurement["state"] = json!("stale");
                    measurement["derived_rate_per_second"] = Value::Null;
                }
            }
        }
    }
    let mut disks=BTreeMap::new();let mut normalized=Vec::new();
    let mut current_reliability=Vec::new();
    for node in &nodes {
        let mut input=node.clone();input["_snapshot_time"]=json!(now);
        let mut index=crate::disk_view::build_disk_index(&input,&objects.iter().filter(|o|o["node_id"]==node["node_id"]).cloned().collect::<Vec<_>>());
        for disk in &mut index.disks {
            disk["reliability"]=crate::reliability_view::disk_assessment(node,disk,&index.objects,&reliability_sources,&reliability_findings);
            disk["device_watch"]=crate::device_watch::disk_projection(node,disk,&index.objects,&device_watches);
            disk["diagnostics"]=crate::diagnostics::disk(node,disk["object_id"].as_str().unwrap_or(""),&index.objects,now);
            current_reliability.extend(disk["reliability"]["findings"].as_array().into_iter().flatten().filter(|f|f["current"]==true).cloned());
        }
        normalized.append(&mut index.objects);disks.insert(node["node_id"].as_str().unwrap_or("").to_owned(),index);
    }
    objects=normalized;objects.sort_by(|a,b|a["object_id"].as_str().cmp(&b["object_id"].as_str()));
    for object in &mut objects {
        if let Some(sources) = object["object_id"].as_str().and_then(|id|detection_by_object.get(id)) { apply_security_warning(&mut object["health"], sources); }
    }
    let active: Vec<_> = objects.iter().filter(|o| o["active"] == true && online_ids.contains(o["node_id"].as_str().unwrap_or(""))).collect();
    let local = local_capacity_objects(&active); let mut shared = BTreeMap::new(); let mut unresolved = 0;
    for object in objects.iter().filter(|o|o["active"] == true && o["kind"] == "nfs_mount") {
        let properties = &object["properties"];
        if properties["shared_filesystem_authoritative"] == true {
            if let Some(id) = properties["shared_filesystem_id"].as_str().filter(|id|!id.is_empty()) {
                if online_ids.contains(object["node_id"].as_str().unwrap_or("")) { select_capacity_observer(&mut shared, id.to_owned(), object); }
                continue;
            }
        }
        unresolved += 1;
    }
    let mut counts = json!({"total":nodes.len(),"online":0,"degraded":0,"offline":0,"unknown":0});
    for node in &nodes { let key = node["availability"].as_str().unwrap_or("unknown"); counts[key] = json!(counts[key].as_u64().unwrap_or(0)+1); }
    let local_cap = capacity(&local);
    let contributors: BTreeSet<_> = local.iter().filter(|o| integer(&metric(o,"capacity_bytes","bytes")).is_some()).filter_map(|o|o["node_id"].as_str()).collect();
    let excluded: Vec<_> = nodes.iter().filter_map(|n|n["node_id"].as_str()).filter(|id|!contributors.contains(id)).map(str::to_owned).collect();
    let cluster_id = Uuid::new_v5(&Uuid::NAMESPACE_OID, app.admin_hash.as_bytes()).to_string();
    let observed_node_count=online_ids.len();
    for node in &mut nodes {
        let owned:Vec<_>=current_reliability.iter().filter(|f|f["node_id"]==node["node_id"]).cloned().collect();
        crate::reliability_view::apply_health(&mut node["health"],&owned);
        finish_assessment(&mut node["health"]);
    }
    let cluster_health = ["critical","warning","healthy"].into_iter().find(|status|nodes.iter().any(|n|n["health"]["overall"]==*status)).unwrap_or("unknown");
    let mut overall_health = health("unknown",&local_cap); overall_health["overall"] = json!(cluster_health);
    apply_security_warning(&mut overall_health, &detection_sources);
    crate::reliability_view::apply_health(&mut overall_health,&current_reliability);
    finish_assessment(&mut overall_health);
    let cluster = json!({"cluster_id":cluster_id,"node_counts":counts,"health":overall_health,"capacity":{"local":local_cap,"shared":capacity(&shared.into_values().collect::<Vec<_>>()),
        "unresolved_shared_mounts":unresolved,"contributing_node_ids":contributors,"excluded_node_ids":excluded},
        "throughput":{"read_bytes_per_second":aggregate(nodes.iter().map(|n|n["read_bytes_per_second"].clone()).collect(),"bytes/second",false),"write_bytes_per_second":aggregate(nodes.iter().map(|n|n["write_bytes_per_second"].clone()).collect(),"bytes/second",false)},
        "active_alert_counts":{"warning":null,"critical":null},"alerts_available":false,"observed_node_count":observed_node_count,"expected_node_count":nodes.len()});
    let filesystems = objects.iter().filter(|o| o["active"] == true && matches!(o["kind"].as_str(), Some("apfs_volume"|"mount"|"nfs_mount"))).map(|object| {
        let p = &object["properties"];
        let kind = p["filesystem_type"].as_str().unwrap_or(if object["kind"] == "nfs_mount" {"nfs"} else if object["kind"] == "apfs_volume" {"apfs"} else {"unknown"});
        let classification = if object["kind"] == "nfs_mount" || matches!(kind.to_ascii_lowercase().as_str(),"nfs"|"nfs4"|"smbfs"|"cifs"|"webdav") {"network"} else if p["removable"] == true {"removable"} else {"local"};
        json!({"object_id":object["object_id"],"node_id":object["node_id"],"filesystem_type":kind,"classification":classification,
            "mount_point":p["mount_point"],"shared_filesystem_id":p["shared_filesystem_id"],"capacity":filesystem_capacity(object),
            "quota":{"limit_bytes":metric(object,"quota_limit_bytes","bytes"),"used_bytes":metric(object,"quota_used_bytes","bytes"),"available_bytes":metric(object,"quota_available_bytes","bytes")},
            "mount_status":metric(object,"nfs_mount_status","enum"),"health":object["health"],"inventory_generation":object["inventory_generation"]})
    }).collect();
    Ok(Snapshot {time:now,change:change.to_string(),nodes,objects,filesystems,events,cluster,disks})
}

#[cfg(test)]
mod capacity_tests {
    use super::*;

    fn observed(id: &str, kind: &str, total: &str, used: &str, age: f64) -> Value {
        let mut object = json!({"object_id":id,"node_id":"node","kind":kind,"parent_ids":[],
            "properties":{"filesystem_type":"hfs","filesystem_id":id,"local":true},"latest_metrics":{}});
        for (name, value) in [("capacity_bytes", total), ("used_bytes", used), ("free_bytes", "10"), ("available_bytes", "10")] {
            object["latest_metrics"][name] = json!({"value":value,"state":"ok","age_seconds":age});
        }
        object
    }

    #[test]
    fn local_capacity_excludes_nonlocal_and_pseudo_mounts() {
        let pool = observed("pool", "apfs_container", "100", "20", 1.0);
        let mut autofs = observed("automount", "mount", "0", "0", 1.0);
        autofs["properties"]["filesystem_type"] = json!("autofs");
        autofs["properties"]["local"] = json!(false);
        autofs["latest_metrics"] = json!({}); // Only cached MNT_NOWAIT readings exist.
        let mut devfs = observed("devices", "mount", "40", "40", 1.0);
        devfs["properties"]["filesystem_type"] = json!("devfs");
        let mut remote = observed("remote", "mount", "800", "300", 1.0);
        remote["properties"]["filesystem_type"] = json!("smbfs");
        remote["properties"]["local"] = json!(false);
        let selected = local_capacity_objects(&[&pool, &autofs, &devfs, &remote]);
        let result = capacity(&selected);
        assert_eq!(result["capacity_bytes"]["value"], "100");
        assert_eq!(result["used_ratio"]["value"], 0.2);
        assert_eq!(result["capacity_bytes"]["coverage"], json!({"observed":1,"expected":1}));
    }

    #[test]
    fn local_capacity_stays_unknown_until_native_physical_inventory_is_available() {
        // Mount and APFS collection can finish while list physical is pending or failed.
        let store = json!({"object_id":"store","node_id":"node","kind":"partition","parent_ids":["node"],
            "properties":{"ciderd_resource_type":"media","source":"diskutil.apfs.list","bsd_name":"disk0s2"}});
        let mut pool = observed("pool", "apfs_container", "100", "20", 1.0);
        pool["properties"]["ciderd_resource_type"] = json!("apfs_container");
        pool["parent_ids"] = json!(["store"]);
        let mut image = observed("image-mount", "mount", "400", "80", 1.0);
        image["properties"]["ciderd_resource_type"] = json!("mount");
        image["properties"]["source"] = json!("/dev/disk4s1");
        let selected = local_capacity_objects(&[&store, &pool, &image]);
        assert!(selected.is_empty(), "Unproven native backing cannot enter physical capacity");
        let result = capacity(&selected);
        for name in ["capacity_bytes", "used_bytes", "free_bytes", "available_bytes", "used_ratio"] {
            assert_eq!(result[name]["state"], "unknown");
            assert!(result[name]["value"].is_null());
        }
    }

    #[test]
    fn local_capacity_preserves_legacy_fallback_alongside_native_nodes() {
        let native = json!({"object_id":"native","node_id":"native","kind":"node","parent_ids":[],
            "properties":{"ciderd_resource_type":"host"}});
        let pool = observed("pool", "apfs_container", "100", "20", 1.0);
        let mount = observed("legacy-mount", "mount", "200", "40", 1.0);
        let selected = local_capacity_objects(&[&native, &pool, &mount]);
        assert_eq!(selected.len(), 2);
        assert_eq!(capacity(&selected)["capacity_bytes"]["value"], "300");
        assert_eq!(capacity(&selected)["used_ratio"]["value"], 0.2);
    }

    #[test]
    fn local_capacity_requires_exact_physical_backing_when_inventory_is_available() {
        let disk = json!({"object_id":"disk","node_id":"node","kind":"device","parent_ids":[],
            "properties":{"ciderd_resource_type":"physical_device","source":"diskutil.list.physical","bsd_name":"disk0"}});
        let store = json!({"object_id":"store","node_id":"node","kind":"partition","parent_ids":["disk"],
            "properties":{"bsd_name":"disk0s2"}});
        let partition = json!({"object_id":"partition","node_id":"node","kind":"partition","parent_ids":["disk"],
            "properties":{"bsd_name":"disk0s3"}});
        let mut pool = observed("pool", "apfs_container", "100", "20", 1.0);
        pool["parent_ids"] = json!(["store"]);
        let mut physical = observed("physical-mount", "mount", "200", "40", 1.0);
        physical["properties"]["source"] = json!("/dev/disk0s3");
        let mut image = observed("image-mount", "mount", "400", "80", 1.0);
        image["properties"]["source"] = json!("/dev/disk4s1");
        let mut prefix = observed("prefix-mount", "mount", "800", "160", 1.0);
        prefix["properties"]["source"] = json!("/dev/disk0s3suffix");
        let image_store = json!({"object_id":"image-store","node_id":"node","kind":"partition","parent_ids":[],
            "properties":{"bsd_name":"disk4s2"}});
        let mut image_pool = observed("image-pool", "apfs_container", "1600", "320", 1.0);
        image_pool["parent_ids"] = json!(["image-store"]);
        let selected = local_capacity_objects(&[&disk, &store, &partition, &pool, &physical, &image, &prefix, &image_store, &image_pool]);
        assert_eq!(capacity(&selected)["capacity_bytes"]["value"], "300");
        assert_eq!(selected.len(), 2);
    }

    #[test]
    fn duplicate_capacity_observers_prefer_complete_then_fresh_measurements() {
        let mut stale = observed("stale", "mount", "100", "20", 1.0);
        let mut partial = observed("partial", "mount", "200", "40", 1.0);
        let mut older = observed("older", "mount", "500", "100", 20.0);
        let mut current = observed("current", "mount", "1000", "200", 2.0);
        for object in [&mut stale, &mut partial, &mut older, &mut current] {
            object["properties"]["filesystem_id"] = json!("same-filesystem");
        }
        for measurement in stale["latest_metrics"].as_object_mut().unwrap().values_mut() {
            measurement["state"] = json!("stale");
        }
        partial["latest_metrics"].as_object_mut().unwrap().remove("used_bytes");
        let selected = local_capacity_objects(&[&stale, &partial, &older, &current]);
        assert_eq!(selected.len(), 1);
        assert_eq!(capacity(&selected)["capacity_bytes"]["value"], "1000");
        assert_eq!(capacity(&selected)["used_ratio"]["value"], 0.2);
    }
}

#[cfg(test)]
mod pagination_tests {
    use super::*;
    const ADMIN:&str="disk-read-cache-bound-tests";
    async fn fixture(count:usize)->(tempfile::TempDir,ReadState,HeaderMap) {
        let directory=tempfile::tempdir().unwrap();let app=AppState::open(&directory.path().join("read.sqlite3"),ADMIN).await.unwrap();
        let mut tx=app.db.begin().await.unwrap();
        for i in 0..count {sqlx::query("INSERT INTO nodes(node_id,name,agent_json,credential_hash,enrolled_at) VALUES (?,?,?,?,0)").bind(Uuid::new_v4().to_string()).bind(format!("node-{i}")).bind("{}").bind(format!("credential-{i}")).execute(&mut *tx).await.unwrap();}
        tx.commit().await.unwrap();
        let mut headers=HeaderMap::new();headers.insert("authorization",format!("Bearer {ADMIN}").parse().unwrap());
        (directory,ReadState{app,pages:Default::default(),budgets:Default::default()},headers)
    }
    fn cached(bytes:usize)->Page {Page{owner:String::new(),path:String::new(),query:Parameters::new(),expires:Utc::now().timestamp_millis()+300_000,time:0,change:"0".into(),values:Arc::new(vec![]),bytes,metadata:json!({})}}
    #[tokio::test]
    async fn pagination_retains_traversal_and_serialized_byte_bounds() {
        let (_directory,state,headers)=fixture(2).await;
        for i in 0..128 {state.pages.lock().unwrap().insert(i.to_string(),cached(1));}
        let uri:Uri="/api/v1/nodes?limit=1".parse().unwrap();
        assert_eq!(dispatch(&state,&headers,&uri,"test").await.unwrap_err().status,StatusCode::TOO_MANY_REQUESTS);
        state.pages.lock().unwrap().clear();state.pages.lock().unwrap().insert("large".into(),cached(32*1024*1024));
        assert_eq!(dispatch(&state,&headers,&uri,"test").await.unwrap_err().status,StatusCode::TOO_MANY_REQUESTS);
        state.pages.lock().unwrap().get_mut("large").unwrap().expires=0;
        assert!(dispatch(&state,&headers,&uri,"test").await.is_ok());
    }
    #[tokio::test]
    async fn snapshot_capacity_and_read_budget_errors_remain_distinct() {
        let (_directory,state,headers)=fixture(2049).await;let uri:Uri="/api/v1/nodes".parse().unwrap();
        assert_eq!(dispatch(&state,&headers,&uri,"test").await.unwrap_err().status,StatusCode::SERVICE_UNAVAILABLE);
        state.budgets.lock().unwrap().insert(store::fingerprint(ADMIN.as_bytes()),(0.0,Utc::now().timestamp_millis()));
        assert_eq!(dispatch(&state,&headers,&uri,"test").await.unwrap_err().status,StatusCode::TOO_MANY_REQUESTS);
    }
}
