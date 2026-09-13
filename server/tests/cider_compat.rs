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

#[tokio::test]
async fn native_user_quota_is_separate_from_volume_capacity_and_keeps_exact_usage() {
    let h=Harness::new().await;
    let mut hb=h.heartbeat();
    hb["resources"][0]["resource_type"]=json!("nfs_user_quota");
    hb["resources"][0]["attributes"]=json!({"server":"127.0.0.1","export_path":"/export","uid":"1000","protocol":"rquota-v1-udp"});
    hb["collections"][0]["collector"]=json!("nfs.rquota");
    hb["collector_states"][0]["collector"]=json!("nfs.rquota");
    hb["collections"][0]["metrics"]=json!([{"name":"storage.nfs.quota.used_bytes","kind":"gauge","unit":"bytes",
        "availability":"available","attributes":{},"freshness":"live","value_type":"integer","value":LARGE.to_string()}]);
    assert_eq!(h.send(&hb).await.0,200);
    let object=h.device().await;
    assert_eq!(object["kind"],"quota");
    assert_eq!(object["latest_metrics"]["nfs_quota_used_bytes"]["value"],LARGE.to_string());
}

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
    assert_eq!(version, 6);
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

#[tokio::test]
async fn catalog_metric_names_populate_compatibility_aliases() {
    let cases = [
        ("storage.apfs.purgeable_bytes", "apfs_purgeable_bytes", "filesystem", json!({})),
        ("storage.nvme.critical_warning_bits", "nvme_critical_warning", "physical_device", json!({})),
        ("storage.nvme.endurance_used_percent", "nvme_percentage_used", "physical_device", json!({})),
        ("storage.nfs.client.operations_total", "nfs_operations_total", "nfs_client", json!({"family":"v3_procedure","operation":"read"})),
        ("storage.nfs.client.rpc_timeouts_total", "nfs_rpc_timeouts_total", "nfs_client", json!({})),
        ("storage.nfs.client.rpc_retries_total", "nfs_retransmissions_total", "nfs_client", json!({})),
    ];
    let mut missing = vec![];
    for (source, alias, resource_type, attributes) in cases {
        let h = Harness::new().await;
        let mut hb = h.heartbeat();
        let definition = &cider_wire::catalog()[source];
        let mut metric = json!({"name":source,"kind":definition.kind,"unit":definition.unit,
            "availability":"available","attributes":attributes,"freshness":"live","value_type":"integer",
            "value":"17"});
        if definition.kind == "counter" { metric["counter_epoch"] = json!("epoch"); }
        hb["resources"][0]["resource_type"] = json!(resource_type);
        hb["collections"][0]["collector"] = json!(definition.collector);
        hb["collections"][0]["metrics"] = json!([metric]);
        hb["collector_states"][0]["collector"] = json!(definition.collector);
        let (status, response) = h.send(&hb).await;
        assert_eq!(status, 200, "{source}: {response}");
        let object = h.device().await;
        if object["latest_metrics"][alias]["value"] != "17" {
            missing.push(format!("{source} -> {alias}"));
        }
    }
    assert!(missing.is_empty(), "Missing aliases: {missing:?}");
}

#[tokio::test]
async fn shared_capacity_uses_a_current_observer_instead_of_first_object_id() {
    let h = Harness::new().await;
    let mut hb = h.heartbeat();
    let mut ids = ["mount-a", "mount-b"];
    let namespace = Uuid::parse_str(&h.node).unwrap();
    ids.sort_by_key(|id| Uuid::new_v5(&namespace, format!("ciderd:{id}").as_bytes()));
    hb["resources"] = json!([]);
    hb["collections"] = json!([]);
    hb["collector_states"] = json!([]);
    hb["monotonic_ns"] = json!("100000000000");
    for (index, id) in ids.iter().enumerate() {
        let at = (Utc::now() - Duration::seconds(if index == 0 { 99 } else { 1 })).to_rfc3339();
        hb["resources"].as_array_mut().unwrap().push(json!({"resource_id":id,"resource_type":"mount","revision":"1",
            "observed_at":at,"identity_confidence":"host_local","attributes":{"filesystem_type":"nfs",
                "shared_filesystem_id":"export-1","shared_filesystem_authoritative":true}}));
        let metrics: Vec<_> = [
            ("storage.filesystem.total_bytes", "1000"),
            ("storage.filesystem.block_accounted_used_bytes", "200"),
            ("storage.filesystem.free_bytes", "800"),
            ("storage.filesystem.available_bytes", "700"),
        ].into_iter().map(|(name, value)| json!({"name":name,"kind":"gauge","unit":"bytes",
            "availability":"available","attributes":{},"freshness":"live","value_type":"integer","value":value})).collect();
        let mono = if index == 0 { "1000000000" } else { "99000000000" };
        hb["collections"].as_array_mut().unwrap().push(json!({"collection_id":id,"resource_id":id,"collector":"filesystem.capacity",
            "adapter_version":"1","source_version":"test","started_at":at,"finished_at":at,"clock_id":"test-clock",
            "started_monotonic_ns":mono,"finished_monotonic_ns":mono,"status":"ok","metrics":metrics}));
        hb["collector_states"].as_array_mut().unwrap().push(json!({"collector":"filesystem.capacity","resource_id":id,
            "phase":"idle","poll_interval_seconds":5,"stale_after_seconds":15,"last_attempt_id":id}));
    }
    let (status, body) = h.send(&hb).await;
    assert_eq!(status, 200, "{body}");
    let (status, body) = request(&h.app, "GET", "/api/v1/cluster", ADMIN, Value::Null).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["data"]["capacity"]["shared"]["capacity_bytes"]["value"], "1000", "{body}");
    assert_eq!(body["data"]["capacity"]["shared"]["used_ratio"]["value"], 0.2);
}

#[tokio::test]
async fn native_node_metadata_exposes_os_and_inventory_acquisition_time() {
    let h = Harness::new().await;
    let mut hb = h.heartbeat();
    hb["agent"]["os_version"] = json!("15.7-test");
    assert_eq!(h.send(&hb).await.0, 200);
    let (_, first) = request(&h.app, "GET", &format!("/api/v1/nodes/{}", h.node), ADMIN, Value::Null).await;
    assert_eq!(first["data"]["os_version"], "15.7-test");
    let inventory_at = first["data"]["inventory_updated_at"].as_str().expect("native inventory has an update time");
    chrono::DateTime::parse_from_rfc3339(inventory_at).unwrap();
    hb["sequence"] = json!("2");
    hb["inventory"]["included"] = json!(false);
    hb["resources"] = json!([]);
    assert_eq!(h.send(&hb).await.0, 200);
    let (_, repeated) = request(&h.app, "GET", &format!("/api/v1/nodes/{}", h.node), ADMIN, Value::Null).await;
    assert_eq!(repeated["data"]["inventory_updated_at"], inventory_at, "heartbeat delivery does not refresh inventory");
}

#[tokio::test]
async fn native_network_filesystems_are_classified_and_filtered_as_network() {
    let h = Harness::new().await;
    let mut hb = h.heartbeat();
    hb["resources"] = json!([]);
    hb["collections"] = json!([]);
    hb["collector_states"] = json!([]);
    for kind in ["nfs", "nfs4", "smbfs", "cifs", "webdav"] {
        hb["resources"].as_array_mut().unwrap().push(json!({"resource_id":kind,"resource_type":"mount","revision":"1",
            "observed_at":Utc::now().to_rfc3339(),"identity_confidence":"host_local",
            "attributes":{"filesystem_type":kind,"local":false}}));
    }
    let (status, body) = h.send(&hb).await;
    assert_eq!(status, 200, "{body}");
    let (status, body) = request(&h.app, "GET", "/api/v1/filesystems?classification=network", ADMIN, Value::Null).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["data"].as_array().unwrap().len(), 5, "{body}");
    let (_, local) = request(&h.app, "GET", "/api/v1/filesystems?classification=local", ADMIN, Value::Null).await;
    assert!(local["data"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn repeated_native_failures_preserve_the_last_usable_observation() {
    let h = Harness::new().await;
    let mut hb = h.heartbeat();
    hb["resources"][0]["resource_type"] = json!("nfs_client");
    hb["collections"][0]["collector"] = json!("nfsstat.client");
    hb["collector_states"][0]["collector"] = json!("nfsstat.client");
    let metric = &mut hb["collections"][0]["metrics"][0];
    metric["name"] = json!("storage.nfs.client.rpc_invalid_replies_total");
    metric["unit"] = json!(cider_wire::catalog()["storage.nfs.client.rpc_invalid_replies_total"].unit);
    let (status, body) = h.send(&hb).await;
    assert_eq!(status, 200, "{body}");
    for sequence in 2..=3 {
        let collection_id = format!("failure-{sequence}");
        let at = (Utc::now() + Duration::milliseconds(sequence)).to_rfc3339();
        hb["sequence"] = json!(sequence.to_string());
        hb["monotonic_ns"] = json!((sequence * 1_000_000_000 + 200_000_000).to_string());
        hb["collections"][0]["collection_id"] = json!(collection_id);
        hb["collector_states"][0]["last_attempt_id"] = json!(collection_id);
        hb["collections"][0]["started_monotonic_ns"] = json!((sequence * 1_000_000_000).to_string());
        hb["collections"][0]["finished_monotonic_ns"] = json!((sequence * 1_000_000_000).to_string());
        hb["collections"][0]["started_at"] = json!(at);
        hb["collections"][0]["finished_at"] = json!(at);
        hb["collections"][0]["status"] = json!("failed");
        let metric = hb["collections"][0]["metrics"][0].as_object_mut().unwrap();
        metric.insert("availability".into(), json!("failed"));
        for field in ["value", "freshness", "value_type", "counter_epoch"] { metric.remove(field); }
        let (status, body) = h.send(&hb).await;
        assert_eq!(status, 200, "{body}");
        let device = h.device().await;
        let reading = &device["latest_metrics"]["storage.nfs.client.rpc_invalid_replies_total"];
        assert_eq!(reading["value"], LARGE.to_string(), "failure {sequence}: {device}");
        assert_eq!(reading["ciderd"]["collection_id"], "collection-1");
        assert_eq!(device["properties"]["ciderd_collector_states"][0]["last_attempt"]["status"], "failed");
    }
}

#[tokio::test]
async fn legacy_mutations_cannot_replace_native_node_state() {
    let h = Harness::new().await;
    assert_eq!(h.send(&h.heartbeat()).await.0, 200);
    let paths = [
        ("PUT", "inventory", json!({"generation":2,"objects":[]})),
        ("POST", "heartbeat", json!({"boot_id":"legacy-boot","agent":{"version":"legacy"}})),
        ("POST", "goodbye", json!({"boot_id":"test-boot"})),
        ("POST", "telemetry", json!({"schema_version":"1.0","node_id":h.node,"boot_id":"test-boot","sequence":1,
            "observed_at":Utc::now().to_rfc3339(),"sent_at":Utc::now().to_rfc3339(),"agent":{"version":"legacy"},
            "inventory_generation":1,"samples":[],"events":[]})),
    ];
    for (method, suffix, body) in paths {
        let (status, body) = request(&h.app, method, &format!("/api/v1/nodes/{}/{suffix}", h.node), &h.credential, body).await;
        assert_eq!(status, 409, "legacy {suffix} must not mutate native state: {body}");
    }
    let mut next = h.heartbeat();
    next["sequence"] = json!("2");
    // The immutable inventory and collection content are exactly the accepted values.
    next["resources"] = json!([]);
    next["collections"] = json!([]);
    next["collector_states"] = json!([]);
    next["inventory"]["included"] = json!(false);
    let (status, body) = h.send(&next).await;
    assert_eq!(status, 200, "native ingestion continues: {body}");
    assert_eq!(h.device().await["active"], true);
}

#[tokio::test]
async fn empty_failed_acquisition_marks_retained_reading_stale_and_recovers() {
    let h = Harness::new().await;
    let mut hb = h.heartbeat();
    assert_eq!(h.send(&hb).await.0, 200);
    let original = h.device().await["latest_metrics"]["device_read_bytes_total"].clone();
    let mut failed = hb["collections"][0].clone();
    failed["collection_id"] = json!("empty-failure");
    failed["status"] = json!("timeout");
    failed["metrics"] = json!([]);
    failed["started_monotonic_ns"] = json!("2000000000");
    failed["finished_monotonic_ns"] = json!("2000000000");
    failed["started_at"] = json!((Utc::now() - Duration::seconds(1)).to_rfc3339());
    failed["finished_at"] = failed["started_at"].clone();
    hb["sequence"] = json!("2");
    hb["monotonic_ns"] = json!("2200000000");
    hb["collections"].as_array_mut().unwrap().push(failed);
    hb["collector_states"][0]["last_attempt_id"] = json!("empty-failure");
    let (status, body) = h.send(&hb).await;
    assert_eq!(status, 200, "{body}");
    let device = h.device().await;
    let retained = &device["latest_metrics"]["device_read_bytes_total"];
    assert_eq!(retained["state"], "stale", "{device}");
    assert_eq!(retained["value"], original["value"]);
    assert_eq!(retained["observed_at"], original["observed_at"]);
    assert_eq!(retained["received_at"], original["received_at"]);
    assert_eq!(retained["ciderd"]["collection_id"], "collection-1");
    assert_eq!(retained["ciderd"]["last_attempt"]["status"], "timeout");
    assert!(retained["derived_rate_per_second"].is_null());
    hb["sequence"] = json!("3");
    hb["monotonic_ns"] = json!("3200000000");
    hb["collections"] = json!([hb["collections"][0].clone()]);
    hb["collections"][0]["collection_id"] = json!("recovery");
    hb["collections"][0]["started_monotonic_ns"] = json!("3000000000");
    hb["collections"][0]["finished_monotonic_ns"] = json!("3000000000");
    hb["collections"][0]["started_at"] = json!(Utc::now().to_rfc3339());
    hb["collections"][0]["finished_at"] = hb["collections"][0]["started_at"].clone();
    hb["collections"][0]["metrics"][0]["value"] = json!((LARGE + 2000).to_string());
    hb["collector_states"][0]["last_attempt_id"] = json!("recovery");
    let (status, body) = h.send(&hb).await;
    assert_eq!(status, 200, "{body}");
    let recovered = h.device().await;
    assert_eq!(recovered["latest_metrics"]["device_read_bytes_total"]["state"], "ok");
    assert_eq!(recovered["latest_metrics"]["device_read_bytes_total"]["value"], (LARGE + 2000).to_string());
}

#[tokio::test]
async fn unavailable_owner_stales_retained_values_without_erasing_source_failures() {
    let h = Harness::new().await;
    let (_, empty) = request(&h.app, "GET", &format!("/api/v1/nodes/{}", h.node), ADMIN, Value::Null).await;
    assert_eq!(empty["data"]["capacity"]["capacity_bytes"]["state"], "unknown", "absent data stays unknown");
    let mut hb = h.heartbeat();
    hb["resources"][0]["resource_type"] = json!("mount");
    hb["resources"][0]["attributes"] = json!({"filesystem_type":"hfs","filesystem_id":"fs-test","local":true,"source":"/dev/disk1"});
    hb["resources"].as_array_mut().unwrap().push(json!({"resource_id":"physical-disk","resource_type":"physical_device","revision":"1",
        "observed_at":Utc::now().to_rfc3339(),"identity_confidence":"host_local",
        "attributes":{"source":"diskutil.list.physical","bsd_name":"disk1"}}));
    hb["collections"][0]["collector"] = json!("filesystem.capacity");
    hb["collections"][0]["metrics"] = json!([
        {"name":"storage.filesystem.total_bytes","kind":"gauge","unit":"bytes","availability":"available",
            "attributes":{},"freshness":"live","value_type":"integer","value":"1000"},
        {"name":"storage.filesystem.block_accounted_used_bytes","kind":"gauge","unit":"bytes","availability":"available",
            "attributes":{},"freshness":"live","value_type":"integer","value":"200"},
        {"name":"storage.filesystem.available_bytes","kind":"gauge","unit":"bytes","availability":"unsupported","attributes":{}}
    ]);
    hb["collector_states"][0]["collector"] = json!("filesystem.capacity");
    hb["collector_states"][0]["stale_after_seconds"] = json!(5400);
    let (status, body) = h.send(&hb).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(h.device().await["latest_metrics"]["capacity_bytes"]["state"], "ok");
    sqlx::query("UPDATE nodes SET last_seen_at=? WHERE node_id=?")
        .bind((Utc::now() - Duration::seconds(91)).timestamp_millis()).bind(&h.node).execute(&h.state.db).await.unwrap();
    let device = h.device().await;
    assert_eq!(device["latest_metrics"]["capacity_bytes"]["state"], "stale", "{device}");
    assert_eq!(device["latest_metrics"]["capacity_bytes"]["value"], "1000");
    assert_eq!(device["latest_metrics"]["storage.filesystem.available_bytes"]["state"], "unsupported");
    let (_, node) = request(&h.app, "GET", &format!("/api/v1/nodes/{}", h.node), ADMIN, Value::Null).await;
    assert_eq!(node["data"]["capacity"]["capacity_bytes"]["state"], "stale");
    assert_eq!(node["data"]["capacity"]["capacity_bytes"]["value"], "1000");
    let (_, filesystems) = request(&h.app, "GET", "/api/v1/filesystems", ADMIN, Value::Null).await;
    assert_eq!(filesystems["data"][0]["capacity"]["capacity_bytes"]["state"], "stale");
    assert_eq!(filesystems["data"][0]["capacity"]["capacity_bytes"]["value"], "1000");
    hb["sequence"] = json!("2");
    assert_eq!(h.send(&hb).await.0, 200);
    assert_eq!(h.device().await["latest_metrics"]["capacity_bytes"]["state"], "ok", "current owner and unexpired source recover");
}


#[tokio::test]
async fn unavailable_node_summaries_retain_rates_and_temperature_as_stale() {
    let h = Harness::new().await;
    let mut hb = h.heartbeat();
    let mut temperature = hb["collections"][0].clone();
    temperature["collection_id"] = json!("temperature");
    temperature["collector"] = json!("smartctl");
    temperature["metrics"] = json!([{"name":"storage.media.temperature_celsius","kind":"gauge","unit":"celsius",
        "availability":"available","freshness":"live","value_type":"number","value":37.5,"attributes":{}}]);
    hb["collections"].as_array_mut().unwrap().push(temperature);
    hb["collector_states"].as_array_mut().unwrap().push(json!({"collector":"smartctl","resource_id":"driver-1","phase":"idle",
        "poll_interval_seconds":300,"stale_after_seconds":900,"last_attempt_id":"temperature"}));
    let (status, body) = h.send(&hb).await;
    assert_eq!(status, 200, "{body}");
    hb["sequence"] = json!("2");
    hb["monotonic_ns"] = json!("2200000000");
    hb["collections"][0]["collection_id"] = json!("read-second");
    hb["collections"][0]["started_monotonic_ns"] = json!("2000000000");
    hb["collections"][0]["finished_monotonic_ns"] = json!("2000000000");
    hb["collections"][0]["started_at"] = json!(Utc::now().to_rfc3339());
    hb["collections"][0]["finished_at"] = hb["collections"][0]["started_at"].clone();
    hb["collections"][0]["metrics"][0]["value"] = json!((LARGE + 500).to_string());
    hb["collector_states"][0]["last_attempt_id"] = json!("read-second");
    let (status, body) = h.send(&hb).await;
    assert_eq!(status, 200, "{body}");
    for (seconds, availability) in [(0, "online"), (31, "degraded"), (91, "offline")] {
        sqlx::query("UPDATE nodes SET last_seen_at=? WHERE node_id=?")
            .bind((Utc::now() - Duration::seconds(seconds)).timestamp_millis()).bind(&h.node).execute(&h.state.db).await.unwrap();
        let (_, node) = request(&h.app, "GET", &format!("/api/v1/nodes/{}", h.node), ADMIN, Value::Null).await;
        assert_eq!(node["data"]["availability"], availability);
        assert_eq!(node["data"]["read_bytes_per_second"]["value"], 500.0, "{availability}: {node}");
        assert_eq!(node["data"]["temperature_celsius"]["value"], 37.5);
        let state = if seconds == 0 { "ok" } else { "stale" };
        assert_eq!(node["data"]["read_bytes_per_second"]["state"], state);
        assert_eq!(node["data"]["temperature_celsius"]["state"], state);
        let (_, cluster) = request(&h.app, "GET", "/api/v1/cluster", ADMIN, Value::Null).await;
        assert_eq!(cluster["data"]["throughput"]["read_bytes_per_second"]["value"].is_null(), seconds != 0);
    }
}

#[tokio::test]
async fn acquisition_context_survives_retained_graph_replay_and_legacy_state() {
    let h = Harness::new().await;
    let mut hb = h.heartbeat();
    let observed = hb["resources"][0]["observed_at"].clone();
    assert_eq!(h.send(&hb).await.0, 200);
    let original = h.device().await["properties"]["ciderd_acquisition"].clone();
    assert_eq!(original["boot_id"], "test-boot");
    assert_eq!(original["observed_at"], observed);
    assert_eq!(original["agent_session_id"], "test-session");
    hb["sequence"] = json!("1");
    hb["boot_id"] = json!("new-boot");
    hb["agent_generation"] = json!("2");
    hb["agent_session_id"] = json!("new-session");
    hb["resources"] = json!([]);
    hb["collections"] = json!([]);
    hb["collector_states"] = json!([]);
    assert_eq!(h.send(&hb).await.0, 200);
    assert_eq!(h.device().await["properties"]["ciderd_acquisition"], original);
    assert_eq!(h.send(&hb).await.0, 200);
    assert_eq!(h.device().await["properties"]["ciderd_acquisition"], original);
    sqlx::query("UPDATE cider_nodes SET state_json=json_remove(state_json,'$.graph.acquisition') WHERE node_id=?")
        .bind(&h.node).execute(&h.state.db).await.unwrap();
    hb["sequence"] = json!("2");
    hb["inventory"]["revision"] = json!("2");
    assert_eq!(h.send(&hb).await.0, 200);
    assert!(h.device().await["properties"]["ciderd_acquisition"].is_null());
    hb["sequence"] = json!("3");
    hb["inventory"]["revision"] = json!("3");
    hb["resources"] = h.heartbeat()["resources"].clone();
    hb["resources"][0]["revision"] = json!("2");
    assert_eq!(h.send(&hb).await.0, 200);
    assert_eq!(h.device().await["properties"]["ciderd_acquisition"]["boot_id"], "new-boot");
    hb["sequence"] = json!("4");
    hb["inventory"]["revision"] = json!("4");
    hb["resources"] = json!([]);
    hb["tombstones"] = json!([{"entity_type":"resource","entity_id":"driver-1","revision":"3",
        "observed_at":Utc::now().to_rfc3339(),"reason":"removed"}]);
    assert_eq!(h.send(&hb).await.0, 200);
    let persisted: String = sqlx::query_scalar("SELECT state_json FROM cider_nodes WHERE node_id=?")
        .bind(&h.node).fetch_one(&h.state.db).await.unwrap();
    let persisted: Value = serde_json::from_str(&persisted).unwrap();
    assert!(persisted["graph"]["acquisition"].get("resource:driver-1").is_none());
}

fn physical_heartbeat(h: &Harness) -> Value {
    let mut hb=h.heartbeat();let at=hb["resources"][0]["observed_at"].clone();
    for name in ["disk0","disk1"] {hb["resources"].as_array_mut().unwrap().push(json!({"resource_id":name,"resource_type":"physical_device","revision":"1","observed_at":at,"identity_confidence":"host_local","attributes":{"source":"diskutil.list.physical","bsd_name":name,"size_bytes":LARGE.to_string()}}));}
    hb["resources"].as_array_mut().unwrap().push(json!({"resource_id":"host","resource_type":"host","revision":"1","observed_at":at,"identity_confidence":"host_local","attributes":{"model_identifier":"Mac14,3","hardware_state":"ok","hardware_source":"sysctl.hw.model","hardware_observed_at":at}}));
    hb["relationships"]=json!([{"relationship_id":"driver-disk","revision":"1","observed_at":at,"from_resource_id":"driver-1","to_resource_id":"disk0","relation":"attached_to","attributes":{"source":"derived.storage","state":"resolved","mapping_method":"iokit.direct_whole_media","boot_id":"test-boot","agent_session_id":"test-session"}}]);
    hb
}

#[tokio::test]
async fn disk_routes_filter_objects_validate_roles_and_freeze_cursor_metadata() {
    let h=Harness::new().await;let mut hb=physical_heartbeat(&h);assert_eq!(h.send(&hb).await.0,200);
    let disk=Uuid::new_v5(&Uuid::parse_str(&h.node).unwrap(),b"ciderd:disk0");let path=format!("/api/v1/nodes/{}/disks?limit=1",h.node);
    let (status,first)=request(&h.app,"GET",&path,ADMIN,Value::Null).await;assert_eq!(status,200,"{first}");
    assert_eq!(first["data"].as_array().unwrap().len(),1);assert_eq!(first["meta"]["node_id"],h.node);assert_eq!(first["meta"]["boot_id"],"test-boot");assert_eq!(first["meta"]["disk_inventory"]["state"],"ok");
    let inventory_path=format!("/api/v1/nodes/{}/inventory?disk_id={disk}&limit=1",h.node);
    let (status,inventory)=request(&h.app,"GET",&inventory_path,ADMIN,Value::Null).await;assert_eq!(status,200,"{inventory}");
    let cursor=first["meta"]["next_cursor"].as_str().unwrap();let inventory_cursor=inventory["meta"]["next_cursor"].as_str().unwrap();
    hb["sequence"]=json!("2");hb["inventory"]["revision"]=json!("2");hb["resources"]=json!([]);hb["relationships"]=json!([]);assert_eq!(h.send(&hb).await.0,200);
    let (status,next)=request(&h.app,"GET",&format!("{path}&cursor={cursor}"),ADMIN,Value::Null).await;assert_eq!(status,200,"{next}");
    for key in ["node_id","boot_id","inventory_generation","topology_revision","disk_inventory"] {assert_eq!(first["meta"][key],next["meta"][key],"{key}");}
    let (status,next)=request(&h.app,"GET",&format!("{inventory_path}&cursor={inventory_cursor}"),ADMIN,Value::Null).await;assert_eq!(status,200,"{next}");assert_eq!(inventory["meta"]["inventory_generation"],next["meta"]["inventory_generation"]);
    assert_eq!(request(&h.app,"GET",&format!("/api/v1/nodes/{}/disks",h.node),&h.credential,Value::Null).await.0,403);
    assert_eq!(request(&h.app,"GET",&format!("/api/v1/nodes/{}/disks",h.node),"",Value::Null).await.0,401);
    assert_eq!(request(&h.app,"GET",&format!("{path}&generation=0"),ADMIN,Value::Null).await.0,400);
    assert_eq!(request(&h.app,"GET",&format!("/api/v1/nodes/{}/inventory?disk_id=bad",h.node),ADMIN,Value::Null).await.0,400);
    assert_eq!(request(&h.app,"GET",&format!("/api/v1/nodes/{}/inventory?disk_id={}",h.node,Uuid::new_v4()),ADMIN,Value::Null).await.0,404);
    assert_eq!(request(&h.app,"GET",&format!("/api/v1/nodes/{}/inventory?disk_id={disk}&kind=device",h.node),ADMIN,Value::Null).await.0,400);
    assert_eq!(request(&h.app,"GET",&format!("/api/v1/nodes/{}/inventory?limit=1&cursor={cursor}",h.node),ADMIN,Value::Null).await.0,400);
    let (_,node)=request(&h.app,"GET",&format!("/api/v1/nodes/{}",h.node),ADMIN,Value::Null).await;assert_eq!(node["data"]["hardware"]["machine_family"],"mac_mini");
    let (_,caps)=request(&h.app,"GET","/api/v1/capabilities",ADMIN,Value::Null).await;assert_eq!(caps["data"]["features"]["physical_disks"],true);
}

#[tokio::test]
async fn successful_empty_physical_inventory_retains_original_attempt_context() {
    let h=Harness::new().await;let mut hb=physical_heartbeat(&h);
    hb["resources"]=json!([hb["resources"][3].clone()]);hb["relationships"]=json!([]);
    hb["collections"][0]["resource_id"]=json!("host");hb["collections"][0]["collector"]=json!("diskutil.inventory");hb["collections"][0]["metrics"]=json!([]);
    hb["collector_states"][0]["resource_id"]=json!("host");hb["collector_states"][0]["collector"]=json!("diskutil.inventory");hb["collector_states"][0]["stale_after_seconds"]=json!(900);
    assert_eq!(h.send(&hb).await.0,200);
    let path=format!("/api/v1/nodes/{}/disks",h.node);let (_,first)=request(&h.app,"GET",&path,ADMIN,Value::Null).await;
    assert_eq!(first["meta"]["disk_inventory"]["state"],"ok","{first}");assert_eq!(first["data"],json!([]));
    let stored=sqlx::query("SELECT properties_json FROM objects WHERE object_id=?").bind(&h.node).fetch_one(&h.state.db).await.unwrap();
    let original:Value=serde_json::from_str(&stored.get::<String,_>("properties_json")).unwrap();
    hb["sequence"]=json!("2");hb["inventory"]["revision"]=json!("2");hb["resources"]=json!([]);hb["collections"]=json!([]);hb["collector_states"]=json!([]);
    assert_eq!(h.send(&hb).await.0,200);let (_,next)=request(&h.app,"GET",&path,ADMIN,Value::Null).await;
    assert_eq!(next["meta"]["disk_inventory"]["state"],"ok");assert_eq!(first["meta"]["disk_inventory"]["observed_at"],next["meta"]["disk_inventory"]["observed_at"]);
    let stored=sqlx::query("SELECT properties_json FROM objects WHERE object_id=?").bind(&h.node).fetch_one(&h.state.db).await.unwrap();
    let next:Value=serde_json::from_str(&stored.get::<String,_>("properties_json")).unwrap();
    assert_eq!(original["ciderd_collector_states"][0]["acquisition"],next["ciderd_collector_states"][0]["acquisition"]);
}

#[tokio::test]
async fn disk_cursor_keeps_source_snapshot_immutable_and_checks_owner_and_expiry() {
    let h=Harness::new().await;assert_eq!(h.send(&physical_heartbeat(&h)).await.0,200);
    let path=format!("/api/v1/nodes/{}/disks?limit=1",h.node);
    let (_,first)=request(&h.app,"GET",&path,&h.state.viewer_token,Value::Null).await;
    let cursor=first["meta"]["next_cursor"].as_str().unwrap();
    assert_eq!(request(&h.app,"GET",&format!("{path}&cursor={cursor}"),ADMIN,Value::Null).await.0,400,"cursor bound to credential owner");
    sqlx::query("UPDATE nodes SET last_seen_at=0 WHERE node_id=?").bind(&h.node).execute(&h.state.db).await.unwrap();
    let (status,next)=request(&h.app,"GET",&format!("{path}&cursor={cursor}"),&h.state.viewer_token,Value::Null).await;assert_eq!(status,200,"{next}");
    assert_eq!(next["data"][0]["availability"],"online");assert_eq!(next["data"][0]["hardware_size_bytes"]["state"],"ok");assert_eq!(first["meta"]["server_time"],next["meta"]["server_time"]);assert_eq!(first["meta"]["topology_revision"],next["meta"]["topology_revision"]);
    let expired=format!("{}.0.1",cursor.split('.').next().unwrap());assert_eq!(request(&h.app,"GET",&format!("{path}&cursor={expired}"),&h.state.viewer_token,Value::Null).await.0,410);
}

#[tokio::test]
async fn retained_relationship_context_cannot_authorize_new_boot_disk_io() {
    let h=Harness::new().await;let mut hb=physical_heartbeat(&h);assert_eq!(h.send(&hb).await.0,200);
    let original=h.device().await;let edge=original["properties"]["ciderd_relationships"][0]["acquisition"].clone();assert_eq!(edge["boot_id"],"test-boot");
    hb["agent_generation"]=json!("2");hb["agent_session_id"]=json!("new-session");hb["boot_id"]=json!("new-boot");hb["resources"]=json!([]);hb["relationships"]=json!([]);hb["collections"]=json!([]);hb["collector_states"]=json!([]);
    assert_eq!(h.send(&hb).await.0,200);assert_eq!(h.send(&hb).await.0,200);
    let retained=h.device().await;assert_eq!(retained["object_id"],original["object_id"]);assert_eq!(retained["properties"]["ciderd_relationships"][0]["acquisition"],edge);
    let (_,disks)=request(&h.app,"GET",&format!("/api/v1/nodes/{}/disks",h.node),ADMIN,Value::Null).await;
    for disk in disks["data"].as_array().unwrap(){assert!(disk["io"]["read_bytes_per_second"]["value"].is_null());assert_eq!(disk["io"]["source_object_ids"],json!([]));}
    hb["sequence"]=json!("2");hb["inventory"]["revision"]=json!("2");hb["resources"]=physical_heartbeat(&h)["resources"].clone();hb["relationships"]=physical_heartbeat(&h)["relationships"].clone();hb["relationships"][0]["attributes"]["boot_id"]=json!("new-boot");hb["relationships"][0]["attributes"]["agent_session_id"]=json!("new-session");
    assert_eq!(h.send(&hb).await.0,200);assert_eq!(h.device().await["properties"]["ciderd_relationships"][0]["acquisition"]["boot_id"],"new-boot");
}

#[tokio::test]
async fn physical_removal_alerts_once_and_reappearance_allows_a_new_episode() {
    let h = Harness::new().await;
    let mut tx = h.state.db.begin().await.unwrap();
    orchard_server::notifications::configure(&mut tx, &json!({"expected_revision":"initial",
        "enabled":true,"sender":"+15555550124","recipient":"+15555550123"}), "test", Utc::now().timestamp_millis()).await.unwrap();
    tx.commit().await.unwrap();
    let mut hb = h.heartbeat();
    hb["collections"] = json!([]);
    hb["collector_states"] = json!([]);
    hb["resources"][0]["resource_type"] = json!("physical_device");
    hb["resources"][0]["attributes"] = json!({"bsd_name":"disk2"});
    let mut disk = hb["resources"][0].clone();
    assert_eq!(h.send(&hb).await.0, 200);
    // Omission from an upsert is not a removal.
    hb["sequence"] = json!("2");
    hb["resources"] = json!([]);
    assert_eq!(h.send(&hb).await.0, 200);
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM attention_episodes WHERE kind='drive_removal'")
        .fetch_one(&h.state.db).await.unwrap();
    assert_eq!(count, 0);
    hb["sequence"] = json!("3");
    hb["inventory"]["revision"] = json!("2");
    hb["tombstones"] = json!([{"entity_type":"resource","entity_id":"driver-1","revision":"2",
        "observed_at":Utc::now().to_rfc3339(),"reason":"explicit removal"}]);
    let result = h.send(&hb).await;
    assert_eq!(result.0, 200, "{}", result.1);
    assert_eq!(h.send(&hb).await.0, 200, "receipt retry is idempotent");
    hb["sequence"] = json!("4");
    assert_eq!(h.send(&hb).await.0, 200, "repeated tombstone is idempotent");
    orchard_server::attention::reconcile(&h.state, Utc::now().timestamp_millis()).await.unwrap();
    let (status, body) = request(&h.app, "GET", "/api/v1/attention?kind=drive_removal", ADMIN, Value::Null).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["data"].as_array().unwrap().len(), 1);
    assert_eq!(body["data"][0]["status"], "open");
    assert_eq!(body["data"][0]["observation_state"], "ok");
    assert_eq!(body["data"][0]["notification"]["state"], "queued");
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM notification_outbox")
        .fetch_one(&h.state.db).await.unwrap();
    assert_eq!(count, 1);
    disk["revision"] = json!("3");
    hb["sequence"] = json!("5");
    hb["inventory"]["revision"] = json!("3");
    hb["tombstones"] = json!([]);
    hb["resources"] = json!([disk]);
    assert_eq!(h.send(&hb).await.0, 200);
    let resolved: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM attention_episodes WHERE kind='drive_removal' AND status='resolved'")
        .fetch_one(&h.state.db).await.unwrap();
    assert_eq!(resolved, 1);
    hb["sequence"] = json!("6");
    hb["inventory"]["revision"] = json!("4");
    hb["resources"] = json!([]);
    hb["tombstones"] = json!([{"entity_type":"resource","entity_id":"driver-1","revision":"4",
        "observed_at":Utc::now().to_rfc3339(),"reason":"explicit removal"}]);
    assert_eq!(h.send(&hb).await.0, 200);
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM attention_episodes WHERE kind='drive_removal'")
        .fetch_one(&h.state.db).await.unwrap();
    assert_eq!(count, 2);
}

#[tokio::test]
async fn reboot_cleanup_and_driver_removal_do_not_raise_physical_drive_alerts() {
    for reboot in [true, false] {
        let h = Harness::new().await;
        let mut hb = h.heartbeat();
        hb["collections"] = json!([]);
        hb["collector_states"] = json!([]);
        if reboot { hb["resources"][0]["resource_type"] = json!("physical_device"); }
        assert_eq!(h.send(&hb).await.0, 200);
        hb["sequence"] = json!("2");
        hb["inventory"]["revision"] = json!("2");
        hb["resources"] = json!([]);
        if reboot {
            hb["agent_generation"] = json!("2");
            hb["agent_session_id"] = json!("new-session");
            hb["boot_id"] = json!("new-boot");
            // Cleanup can arrive after the first heartbeat from a new boot.
            assert_eq!(h.send(&hb).await.0, 200);
            hb["sequence"] = json!("3");
            hb["inventory"]["revision"] = json!("3");
        }
        hb["tombstones"] = json!([{"entity_type":"resource","entity_id":"driver-1","revision":"2",
            "observed_at":Utc::now().to_rfc3339(),"reason":"explicit removal"}]);
        let result = h.send(&hb).await;
        assert_eq!(result.0, 200, "{}", result.1);
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM attention_episodes WHERE kind='drive_removal'")
            .fetch_one(&h.state.db).await.unwrap();
        assert_eq!(count, 0);
    }
}
