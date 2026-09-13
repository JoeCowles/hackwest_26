use axum::{body::{to_bytes, Body}, http::Request, Router};
use chrono::{Duration, Utc};
use orchard_server::{api, cider_api, cider_wire, read_api, store::AppState};
use serde_json::{json, Value};
use sqlx::Row;
use tempfile::TempDir;
use tower::ServiceExt;
use uuid::Uuid;

const ADMIN: &str = "compatibility-test-admin";
const LARGE: u128 = u64::MAX as u128 + 1000;

struct Harness {
    directory: TempDir,
    state: AppState,
    app: Router,
    node: String,
    credential: String,
}

fn routes(state: &AppState) -> Router {
    api::router(state.clone())
        .merge(cider_api::router(state.clone()))
        .merge(read_api::router(state.clone()))
}

async fn request(app: &Router, method: &str, path: &str, credential: &str, body: Value) -> (u16, Value) {
    let request = Request::builder().method(method).uri(path)
        .header("authorization", format!("Bearer {credential}"))
        .header("content-type", "application/json")
        .header("x-request-id", Uuid::new_v4().to_string())
        .header("x-request-timestamp", Utc::now().to_rfc3339())
        .body(if method == "GET" { Body::empty() } else { Body::from(body.to_string()) }).unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status().as_u16();
    let bytes = to_bytes(response.into_body(), 2 * 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

impl Harness {
    async fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let state = AppState::open(&directory.path().join("db.sqlite3"), ADMIN).await.unwrap();
        let app = routes(&state);
        let token = state.local_enrollment_token().await.unwrap();
        let (status, body) = request(&app, "POST", "/api/v1/nodes/enroll", ADMIN,
            json!({"enrollment_token":token["enrollment_token"],"name":"ciderd integration test","agent":{"version":"0.1.0"}})).await;
        assert_eq!(status, 201, "{body}");
        Self { directory, state, app,
            node: body["data"]["node_id"].as_str().unwrap().into(),
            credential: body["data"]["credential"].as_str().unwrap().into() }
    }

    fn heartbeat(&self) -> Value {
        let at = (Utc::now() - Duration::seconds(2)).to_rfc3339();
        let hb = json!({
            "schema_version":"2.0","message_type":"heartbeat","node_id":self.node,
            "boot_id":"test-boot","agent_session_id":"test-session","agent_generation":"1",
            "sequence":"1","created_at":Utc::now().to_rfc3339(),"clock_id":"test-clock","monotonic_ns":"1200000000",
            "agent":{"version":"0.1.0","target":"aarch64-apple-darwin","os_version":"test","os_build":"test",
                "delivery_mode":"latest","heartbeat_interval_seconds":5,"quarantined_workers":0,
                "discarded_samples_total":"0","dropped_events_total":"0","payload_limited":false},
            "inventory":{"revision":"1","included":true},
            "resources":[{"resource_id":"driver-1","resource_type":"controller","revision":"1",
                "observed_at":at,"identity_confidence":"host_local",
                "attributes":{"source":"IOBlockStorageDriver","scope":"driver","registry_entry_id":"123"}}],
            "relationships":[],
            "collections":[{"collection_id":"collection-1","resource_id":"driver-1","collector":"iokit.block",
                "adapter_version":"1","source_version":"test","started_at":at,"finished_at":at,"clock_id":"test-clock",
                "started_monotonic_ns":"1000000000","finished_monotonic_ns":"1000000000","status":"ok",
                "metrics":[{"name":"storage.device.read_bytes_total","kind":"counter","unit":"bytes",
                    "availability":"available","attributes":{},"freshness":"live","value_type":"integer",
                    "value":LARGE.to_string(),"counter_epoch":"disk-epoch-1"}]}],
            "collector_states":[{"collector":"iokit.block","resource_id":"driver-1","phase":"idle",
                "poll_interval_seconds":5,"stale_after_seconds":15,"last_attempt_id":"collection-1"}],
            "events":[],"tombstones":[]
        });
        serde_json::from_value::<cider_wire::Heartbeat>(hb.clone()).unwrap().validate().unwrap();
        hb
    }

    async fn send(&self, hb: &Value) -> (u16, Value) {
        // The real schema-2 sender does not supply v1 request-ID/timestamp headers.
        let response = self.app.clone().oneshot(Request::builder()
            .method("POST").uri("/api/v2/ciderd/heartbeat")
            .header("authorization", format!("Bearer {}", self.credential))
            .header("content-type", "application/json")
            .body(Body::from(hb.to_string())).unwrap()).await.unwrap();
        let status = response.status().as_u16();
        assert_eq!(response.headers().get("cache-control").unwrap(), "no-store");
        let bytes = to_bytes(response.into_body(), 2 * 1024 * 1024).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    async fn samples(&self) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM metric_samples WHERE node_id=?")
            .bind(&self.node).fetch_one(&self.state.db).await.unwrap()
    }

    async fn device(&self) -> Value {
        let id = Uuid::new_v5(&Uuid::parse_str(&self.node).unwrap(), b"ciderd:driver-1");
        let (status, body) = request(&self.app, "GET", &format!("/api/v1/objects/{id}"), ADMIN, Value::Null).await;
        assert_eq!(status, 200, "{body}");
        body["data"].clone()
    }
}

#[tokio::test]
async fn duplicate_receipts_and_repeated_collections_do_not_refresh_measurements() {
    let h = Harness::new().await;
    let mut hb = h.heartbeat();
    let (status, original) = h.send(&hb).await;
    assert_eq!(status, 200, "{original}");
    serde_json::from_value::<cider_wire::Acknowledgement>(original.clone()).unwrap()
        .validate_for(&serde_json::from_value(hb.clone()).unwrap()).unwrap();
    assert_eq!(h.samples().await, 1);
    sqlx::query("UPDATE nodes SET last_seen_at=0 WHERE node_id=?").bind(&h.node).execute(&h.state.db).await.unwrap();
    assert_eq!(h.send(&hb).await, (200, original));
    let seen: i64 = sqlx::query_scalar("SELECT last_seen_at FROM nodes WHERE node_id=?").bind(&h.node).fetch_one(&h.state.db).await.unwrap();
    assert_eq!(seen, 0, "duplicate receipt must not renew liveness");
    hb["sequence"] = json!("2");
    hb["monotonic_ns"] = json!("6200000000");
    let (status, body) = h.send(&hb).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(h.samples().await, 1);
    let device = h.device().await;
    let metric = &device["latest_metrics"]["device_read_bytes_total"];
    assert_eq!(metric["value"], LARGE.to_string(), "{device}");
    assert!(metric["derived_rate_per_second"].is_null());
    assert_eq!(metric["ciderd"]["collection_id"], "collection-1");
}

#[tokio::test]
async fn exact_counter_rates_reset_at_epoch_boundaries() {
    let h = Harness::new().await;
    let mut hb = h.heartbeat();
    assert_eq!(h.send(&hb).await.0, 200);
    hb["sequence"] = json!("2");
    hb["monotonic_ns"] = json!("2200000000");
    hb["collections"][0]["collection_id"] = json!("collection-2");
    hb["collector_states"][0]["last_attempt_id"] = json!("collection-2");
    hb["collections"][0]["started_monotonic_ns"] = json!("2000000000");
    hb["collections"][0]["finished_monotonic_ns"] = json!("2000000000");
    hb["collections"][0]["started_at"] = json!((Utc::now() - Duration::seconds(1)).to_rfc3339());
    hb["collections"][0]["finished_at"] = hb["collections"][0]["started_at"].clone();
    hb["collections"][0]["metrics"][0]["value"] = json!((LARGE + 500).to_string());
    let (status, body) = h.send(&hb).await;
    assert_eq!(status, 200, "{body}");
    let device = h.device().await;
    assert_eq!(device["latest_metrics"]["device_read_bytes_total"]["derived_rate_per_second"], 500.0, "{device}");
    hb["sequence"] = json!("3");
    hb["monotonic_ns"] = json!("3200000000");
    hb["collections"][0]["collection_id"] = json!("collection-3");
    hb["collector_states"][0]["last_attempt_id"] = json!("collection-3");
    hb["collections"][0]["started_monotonic_ns"] = json!("3000000000");
    hb["collections"][0]["finished_monotonic_ns"] = json!("3000000000");
    hb["collections"][0]["started_at"] = json!(Utc::now().to_rfc3339());
    hb["collections"][0]["finished_at"] = hb["collections"][0]["started_at"].clone();
    hb["collections"][0]["metrics"][0]["value"] = json!("7");
    hb["collections"][0]["metrics"][0]["counter_epoch"] = json!("disk-epoch-2");
    assert_eq!(h.send(&hb).await.0, 200);
    let device = h.device().await;
    assert!(device["latest_metrics"]["device_read_bytes_total"]["derived_rate_per_second"].is_null());
    assert_eq!(device["latest_metrics"]["device_read_bytes_total"]["derivation_state"], "reset");
}

#[tokio::test]
async fn inventory_upserts_preserve_omitted_resources_until_explicit_tombstones() {
    let h = Harness::new().await;
    let mut hb = h.heartbeat();
    assert_eq!(h.send(&hb).await.0, 200);
    hb["sequence"] = json!("2");
    hb["inventory"]["revision"] = json!("2");
    hb["resources"] = json!([{"resource_id":"disk-2","resource_type":"physical_device","revision":"1",
        "observed_at":Utc::now().to_rfc3339(),"identity_confidence":"host_local","attributes":{"bsd_name":"disk2"}}]);
    let (status, body) = h.send(&hb).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(h.device().await["active"], true);
    hb["sequence"] = json!("3");
    hb["inventory"]["revision"] = json!("3");
    hb["resources"] = json!([]);
    hb["collections"] = json!([]);
    hb["collector_states"] = json!([]);
    hb["tombstones"] = json!([{"entity_type":"resource","entity_id":"driver-1","revision":"2",
        "observed_at":Utc::now().to_rfc3339(),"reason":"explicit removal"}]);
    let (status, body) = h.send(&hb).await;
    assert_eq!(status, 200, "{body}");
    let active: i64 = sqlx::query_scalar("SELECT active FROM objects WHERE node_id=? AND local_id='ciderd:driver-1'")
        .bind(&h.node).fetch_one(&h.state.db).await.unwrap();
    assert_eq!(active, 0);
}

#[tokio::test]
async fn missing_inventory_is_requested_and_committed_receipts_survive_reopen() {
    let h = Harness::new().await;
    let full = h.heartbeat();
    let mut missing = full.clone();
    missing["inventory"]["included"] = json!(false);
    missing["resources"] = json!([]);
    let (status, ack) = h.send(&missing).await;
    assert_eq!(status, 200, "{ack}");
    assert_eq!(ack["request_inventory"], true);
    assert!(ack["inventory_revision"].is_null());
    assert_eq!(h.samples().await, 0);
    let mut full = full;
    full["sequence"] = json!("2");
    let (status, ack) = h.send(&full).await;
    assert_eq!(status, 200, "{ack}");
    assert_eq!(ack["request_inventory"], false);
    assert_eq!(h.samples().await, 1);
    let reopened = AppState::open(&h.directory.path().join("db.sqlite3"), ADMIN).await.unwrap();
    let result = request(&routes(&reopened), "POST", "/api/v2/ciderd/heartbeat", &h.credential, full).await;
    assert_eq!(result, (200, ack));
    let version: i64 = sqlx::query_scalar("PRAGMA user_version").fetch_one(&reopened.db).await.unwrap();
    assert_eq!(version, 2);
}

#[tokio::test]
async fn credentials_conflicts_and_privacy_fail_atomically() {
    let h = Harness::new().await;
    let hb = h.heartbeat();
    assert_eq!(request(&h.app, "POST", "/api/v2/ciderd/heartbeat", ADMIN, hb.clone()).await.0, 401);
    let mut wrong_node = hb.clone();
    wrong_node["node_id"] = json!(Uuid::new_v4().to_string());
    assert_eq!(h.send(&wrong_node).await.0, 403);
    assert_eq!(h.send(&hb).await.0, 200);
    let mut conflict = hb.clone();
    conflict["collections"][0]["metrics"][0]["value"] = json!("9");
    assert_eq!(h.send(&conflict).await.0, 409);
    conflict["sequence"] = json!("2");
    assert_eq!(h.send(&conflict).await.0, 409, "immutable ID content cannot change in a later heartbeat");
    let mut private = hb;
    private["sequence"] = json!("2");
    private["resources"][0]["attributes"]["password"] = json!("not-a-real-secret");
    assert_eq!(h.send(&private).await.0, 422);
    assert_eq!(h.samples().await, 1);
    let receiver = sqlx::query("SELECT state_json FROM cider_nodes WHERE node_id=?").bind(&h.node).fetch_one(&h.state.db).await.unwrap();
    let receiver: Value = serde_json::from_str(&receiver.get::<String, _>("state_json")).unwrap();
    assert_eq!(receiver["sequence"], "1");
}
