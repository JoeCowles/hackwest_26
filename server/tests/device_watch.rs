//! Synthetic authenticated heartbeat acceptance; never launches a provider worker.
use axum::{
    Router,
    body::{Body, to_bytes},
    http::Request,
};
use chrono::Utc;
use orchard_server::{api, cider_api, cider_wire, notifications, read_api, store::AppState};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const ADMIN: &str = "synthetic-device-watch-admin";
const ID: &str = "11111111-1111-4111-8111-111111111111";
struct Harness {
    _directory: tempfile::TempDir,
    state: AppState,
    app: Router,
    node: String,
    credential: String,
    sequence: u64,
    generation: u64,
    session: String,
}
async fn request(
    app: &Router,
    method: &str,
    path: &str,
    credential: &str,
    body: Value,
) -> (u16, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("authorization", format!("Bearer {credential}"))
                .header("content-type", "application/json")
                .header("x-request-id", Uuid::new_v4().to_string())
                .header("x-request-timestamp", Utc::now().to_rfc3339())
                .body(if method == "GET" {
                    Body::empty()
                } else {
                    Body::from(body.to_string())
                })
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status().as_u16();
    (
        status,
        serde_json::from_slice(
            &to_bytes(response.into_body(), 2 * 1024 * 1024)
                .await
                .unwrap(),
        )
        .unwrap(),
    )
}
fn routes(state: &AppState) -> Router {
    api::router(state.clone())
        .merge(cider_api::router(state.clone()))
        .merge(read_api::router(state.clone()))
}
impl Harness {
    async fn watch_views(&self, now: i64) -> Vec<Value> {
        let mut tx = self.state.db.begin().await.unwrap();
        orchard_server::device_watch::summaries(&mut tx, now)
            .await
            .unwrap()
    }
    async fn node_view(&self) -> Value {
        let (code, body) = request(
            &self.app,
            "GET",
            &format!("/api/v1/nodes/{}", self.node),
            ADMIN,
            Value::Null,
        )
        .await;
        assert_eq!(code, 200, "{body}");
        body["data"].clone()
    }
    async fn restart(&mut self) {
        self.app = Router::new();
        self.state.db.close().await;
        self.state = AppState::open(&self._directory.path().join("db.sqlite3"), ADMIN)
            .await
            .unwrap();
        self.app = routes(&self.state);
    }
    async fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let state = AppState::open(&directory.path().join("db.sqlite3"), ADMIN)
            .await
            .unwrap();
        let app = routes(&state);
        let token = state.local_enrollment_token().await.unwrap();
        let (status,body)=request(&app,"POST","/api/v1/nodes/enroll",ADMIN,json!({"enrollment_token":token["enrollment_token"],"name":"Synthetic USB test","agent":{"version":"test"}})).await;
        assert_eq!(status, 201, "{body}");
        let mut tx = state.db.begin().await.unwrap();
        notifications::configure(&mut tx,&json!({"expected_revision":"initial","enabled":true,"sender":"+15555550100","recipient":"+15555550101"}),ADMIN,Utc::now().timestamp_millis()).await.unwrap();
        tx.commit().await.unwrap();
        Self {
            _directory: directory,
            state,
            app,
            node: body["data"]["node_id"].as_str().unwrap().into(),
            credential: body["data"]["credential"].as_str().unwrap().into(),
            sequence: 0,
            generation: 1,
            session: "session-1".into(),
        }
    }
    fn heartbeat(
        &mut self,
        second: u64,
        devices: Vec<Value>,
        status: &str,
        complete: bool,
    ) -> Value {
        self.sequence += 1;
        let at = Utc::now().to_rfc3339();
        let host = format!("{}/host", self.node);
        let mut resources = vec![
            json!({"resource_id":host,"resource_type":"host","revision":"1","observed_at":at,"identity_confidence":"host_local","attributes":{}}),
        ];
        // Retained inventory intentionally includes both incarnation resources.
        for (driver, registry) in [("driver-1", "123"), ("driver-2", "456")] {
            resources.push(json!({"resource_id":driver,"resource_type":"controller","revision":"1","observed_at":at,"identity_confidence":"host_local","attributes":{"source":"IOBlockStorageDriver","scope":"driver","registry_entry_id":registry}}));
        }
        json!({"schema_version":"2.0","message_type":"heartbeat","node_id":self.node,"boot_id":"boot-1","agent_session_id":self.session,"agent_generation":self.generation.to_string(),"sequence":self.sequence.to_string(),"created_at":at,"clock_id":"clock-1","monotonic_ns":((second+1)*1_000_000_000).to_string(),
            "agent":{"version":"test","target":"aarch64-apple-darwin","os_version":"test","os_build":"test","delivery_mode":"latest","heartbeat_interval_seconds":5,"quarantined_workers":0,"discarded_samples_total":"0","dropped_events_total":"0","payload_limited":false},
            "inventory":{"revision":"1","included":self.sequence==1},"resources":if self.sequence==1 {resources}else{vec![]},"relationships":[],
            "collections":[{"collection_id":format!("{}-{}",self.session,second),"resource_id":host,"collector":"iokit.block","adapter_version":"1","source_version":"test","started_at":at,"finished_at":at,"clock_id":"clock-1","started_monotonic_ns":(second*1_000_000_000).to_string(),"finished_monotonic_ns":(second*1_000_000_000).to_string(),"status":status,"metrics":[],"extensions":{"usb_device_snapshot":{"version":1,"complete":complete,"devices":devices}}}],
            "collector_states":[{"collector":"iokit.block","resource_id":host,"phase":"idle","poll_interval_seconds":5,"stale_after_seconds":15,"last_attempt_id":format!("{}-{}",self.session,second)}],"events":[],"tombstones":[]})
    }
    async fn send(&self, hb: &Value) {
        serde_json::from_value::<cider_wire::Heartbeat>(hb.clone())
            .unwrap()
            .validate()
            .unwrap();
        let (status, body) = request(
            &self.app,
            "POST",
            "/api/v2/ciderd/heartbeat",
            &self.credential,
            hb.clone(),
        )
        .await;
        assert_eq!(status, 200, "{body}");
    }
    async fn observe(
        &mut self,
        second: u64,
        devices: Vec<Value>,
        status: &str,
        complete: bool,
    ) -> Value {
        let hb = self.heartbeat(second, devices, status, complete);
        self.send(&hb).await;
        hb
    }
    async fn episodes(&self) -> Vec<Value> {
        let (code, body) = request(&self.app, "GET", "/api/v1/attention", ADMIN, Value::Null).await;
        assert_eq!(code, 200, "{body}");
        body["data"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|v| {
                v["source_key"].as_str().is_some_and(|s| {
                    s.starts_with("device_presence:") || s.starts_with("usb_link:")
                })
            })
            .cloned()
            .collect()
    }
    async fn queued(&self) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM notification_outbox WHERE state='queued'")
            .fetch_one(&self.state.db)
            .await
            .unwrap()
    }
}
fn device(driver: &str, bps: Option<&str>) -> Value {
    json!({"identity":ID,"identity_basis":"reported_usb_serial","identity_scope":"usb_enclosure","driver_resource_id":driver,"bsd_name":"disk4","negotiated_bps":bps,"speed_state":if bps.is_some(){"available"}else{"unknown"},"reason":null})
}

#[tokio::test]
async fn complete_absence_queues_once_and_positive_reappearance_resolves() {
    let mut h = Harness::new().await;
    h.observe(1, vec![device("driver-1", Some("5000000000"))], "ok", true)
        .await;
    h.observe(6, vec![device("driver-1", Some("5000000000"))], "ok", true)
        .await;
    h.observe(11, vec![], "ok", true).await;
    assert!(h.episodes().await.is_empty());
    let absent = h.observe(16, vec![], "ok", true).await;
    let episodes = h.episodes().await;
    assert_eq!(
        episodes.len(),
        1,
        "Two observed absences must create the missing-device concern"
    );
    assert_eq!(episodes[0]["status"], "open");
    assert_eq!(h.queued().await, 1);
    h.send(&absent).await;
    h.observe(21, vec![], "ok", true).await;
    assert_eq!(h.episodes().await[0]["id"], episodes[0]["id"]);
    assert_eq!(h.queued().await, 1);
    h.observe(26, vec![device("driver-2", Some("5000000000"))], "ok", true)
        .await;
    assert_eq!(h.episodes().await[0]["status"], "open");
    h.observe(31, vec![device("driver-2", Some("5000000000"))], "ok", true)
        .await;
    assert_eq!(h.episodes().await[0]["status"], "resolved");
    assert_eq!(h.queued().await, 0);
}

#[tokio::test]
async fn confirmed_fast_link_survives_registry_reconnect_and_warns_on_slow_link() {
    let mut h = Harness::new().await;
    h.observe(1, vec![device("driver-1", Some("5000000000"))], "ok", true)
        .await;
    h.observe(6, vec![device("driver-1", Some("5000000000"))], "ok", true)
        .await;
    h.observe(11, vec![device("driver-2", Some("480000000"))], "ok", true)
        .await;
    assert!(h.episodes().await.is_empty());
    h.observe(16, vec![device("driver-2", Some("480000000"))], "ok", true)
        .await;
    let episodes = h.episodes().await;
    assert_eq!(
        episodes.len(),
        1,
        "A confirmed same-enclosure downshift must create a link concern"
    );
    assert!(
        episodes[0]["source_key"]
            .as_str()
            .unwrap()
            .starts_with("usb_link:")
    );
    assert_eq!(
        episodes[0]["evidence"]["baseline"]["negotiated_bps"],
        "5000000000"
    );
    assert_eq!(
        episodes[0]["evidence"]["current"]["negotiated_bps"],
        "480000000"
    );
    assert_eq!(h.queued().await, 1);
}

#[tokio::test]
async fn partial_failure_replay_and_context_change_cannot_certify_absence() {
    let mut h = Harness::new().await;
    h.observe(1, vec![device("driver-1", None)], "ok", true)
        .await;
    h.observe(6, vec![device("driver-1", None)], "ok", true)
        .await;
    let first = h.observe(11, vec![], "ok", true).await;
    let mut replay = h.heartbeat(16, vec![], "ok", true);
    replay["collections"] = first["collections"].clone();
    replay["collector_states"] = first["collector_states"].clone();
    h.send(&replay).await;
    for (second, status, complete) in [(21, "ok", false), (26, "failed", false), (31, "ok", true)] {
        h.observe(second, vec![], status, complete).await;
        assert!(h.episodes().await.is_empty());
    }
    h.generation = 2;
    h.session = "session-2".into();
    h.sequence = 0;
    h.observe(36, vec![], "ok", true).await;
    h.observe(41, vec![], "ok", true).await;
    assert!(h.episodes().await.is_empty());
}

fn weak_device() -> Value {
    let mut d = device("driver-1", Some("5000000000"));
    d["identity"] = json!("22222222-2222-4222-8222-222222222222");
    d["identity_basis"] = json!("boot_registry");
    d["identity_scope"] = json!("driver_incarnation");
    d
}
#[tokio::test]
async fn same_present_driver_with_weaker_identity_cannot_create_false_disappearance() {
    let mut h = Harness::new().await;
    h.observe(1, vec![device("driver-1", Some("5000000000"))], "ok", true)
        .await;
    h.observe(6, vec![device("driver-1", Some("5000000000"))], "ok", true)
        .await;
    h.observe(11, vec![weak_device()], "ok", true).await;
    h.observe(16, vec![weak_device()], "ok", true).await;
    assert!(
        h.episodes().await.is_empty(),
        "An observed driver with ambiguous identity is not absent"
    );
}
#[tokio::test]
async fn weak_identity_exposes_reported_speed_without_historical_comparison() {
    let mut h = Harness::new().await;
    h.observe(1, vec![weak_device()], "ok", true).await;
    let mut tx = h.state.db.begin().await.unwrap();
    let rows = orchard_server::device_watch::summaries(&mut tx, Utc::now().timestamp_millis())
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["projection"]["negotiated_bps"], "5000000000");
    assert_eq!(rows[0]["projection"]["link_state"], "unsupported");
    assert!(rows[0]["projection"]["baseline_bps"].is_null());
}

#[tokio::test]
async fn existing_disk_route_exposes_watch_independently_of_missing_io_samples() {
    let mut h = Harness::new().await;
    let mut hb = h.heartbeat(1, vec![device("driver-1", Some("5000000000"))], "ok", true);
    let at = hb["created_at"].clone();
    hb["resources"].as_array_mut().unwrap().push(json!({"resource_id":"disk4","resource_type":"physical_device","revision":"1","observed_at":at,"identity_confidence":"host_local","attributes":{"source":"diskutil.list.physical","bsd_name":"disk4"}}));
    hb["relationships"] = json!([{"relationship_id":"usb-disk-edge","revision":"1","observed_at":at,"from_resource_id":"driver-1","to_resource_id":"disk4","relation":"attached_to","attributes":{"source":"derived.storage","state":"resolved","mapping_method":"fixture"}}]);
    h.send(&hb).await;
    h.observe(6, vec![device("driver-1", Some("5000000000"))], "ok", true)
        .await;
    let (code, body) = request(
        &h.app,
        "GET",
        &format!("/api/v1/nodes/{}/disks", h.node),
        ADMIN,
        Value::Null,
    )
    .await;
    assert_eq!(code, 200, "{body}");
    assert_eq!(body["data"].as_array().unwrap().len(), 1);
    let disk = &body["data"][0];
    assert!(disk["io"]["read_bytes_per_second"]["value"].is_null());
    assert_eq!(disk["device_watch"]["presence"], "present");
    assert_eq!(disk["device_watch"]["baseline_bps"], "5000000000");
    let node = h.node_view().await;
    assert_eq!(node["device_watch"]["watched_devices"], 1);
    assert_eq!(node["device_watch"]["admission_limited"], false);
}

#[tokio::test]
async fn confirmed_baseline_and_open_episode_survive_restart_then_explicit_recovery() {
    let mut h = Harness::new().await;
    for second in [1, 6] {
        h.observe(
            second,
            vec![device("driver-1", Some("5000000000"))],
            "ok",
            true,
        )
        .await;
    }
    h.restart().await;
    for second in [11, 16] {
        h.observe(
            second,
            vec![device("driver-2", Some("480000000"))],
            "ok",
            true,
        )
        .await;
    }
    let episode = h.episodes().await[0].clone();
    assert_eq!(h.queued().await, 1);
    h.restart().await;
    h.observe(21, vec![device("driver-2", Some("5000000000"))], "ok", true)
        .await;
    assert_eq!(h.episodes().await[0]["status"], "open");
    h.observe(26, vec![device("driver-2", Some("5000000000"))], "ok", true)
        .await;
    let rows = h.episodes().await;
    assert_eq!(rows[0]["id"], episode["id"]);
    assert_eq!(rows[0]["status"], "resolved");
    for second in [31, 36] {
        h.observe(
            second,
            vec![device("driver-2", Some("480000000"))],
            "ok",
            true,
        )
        .await;
    }
    assert_eq!(h.episodes().await.len(), 2);
    assert_eq!(h.queued().await, 1);
}

#[tokio::test]
async fn stale_failed_missing_speed_and_owner_loss_preserve_open_history_as_unknown() {
    let mut h = Harness::new().await;
    for (second, speed) in [
        (1, "5000000000"),
        (6, "5000000000"),
        (11, "480000000"),
        (16, "480000000"),
    ] {
        h.observe(second, vec![device("driver-1", Some(speed))], "ok", true)
            .await;
    }
    let now = Utc::now().timestamp_millis();
    let old = h.watch_views(now + 16000).await;
    assert_eq!(old[0]["projection"]["observation"]["state"], "stale");
    assert_eq!(old[0]["projection"]["presence"], "unknown");
    h.observe(21, vec![device("driver-1", None)], "ok", true)
        .await;
    assert_eq!(h.episodes().await[0]["observation_state"], "unknown");
    assert_eq!(h.episodes().await[0]["status"], "open");
    h.observe(26, vec![], "failed", false).await;
    assert_eq!(h.episodes().await[0]["status"], "open");
    for second in [31, 36] {
        h.observe(
            second,
            vec![device("driver-1", Some("480000000"))],
            "ok",
            true,
        )
        .await;
    }
    sqlx::query("UPDATE nodes SET goodbye_at=? WHERE node_id=?")
        .bind(now)
        .bind(&h.node)
        .execute(&h.state.db)
        .await
        .unwrap();
    orchard_server::attention::reconcile(&h.state, Utc::now().timestamp_millis())
        .await
        .unwrap();
    assert_eq!(h.episodes().await[0]["observation_state"], "stale");
    let view = h.watch_views(Utc::now().timestamp_millis()).await;
    assert_eq!(view[0]["projection"]["link_state"], "unknown");
    assert_eq!(view[0]["projection"]["baseline_bps"], "5000000000");
}

#[tokio::test]
async fn cold_slow_link_and_single_high_outlier_do_not_create_degradation() {
    let mut h = Harness::new().await;
    for (second, speed) in [
        (1, "480000000"),
        (6, "480000000"),
        (11, "5000000000"),
        (16, "480000000"),
        (21, "480000000"),
    ] {
        h.observe(second, vec![device("driver-1", Some(speed))], "ok", true)
            .await;
    }
    assert!(h.episodes().await.is_empty());
    assert_eq!(
        h.watch_views(Utc::now().timestamp_millis()).await[0]["projection"]["baseline_bps"],
        "480000000"
    );
}

#[tokio::test]
async fn invalid_optional_snapshot_never_turns_retained_inventory_into_absence() {
    let mut h = Harness::new().await;
    for second in [1, 6] {
        h.observe(second, vec![device("driver-1", None)], "ok", true)
            .await;
    }
    for (index, mode) in ["unsupported", "duplicate", "reference", "version", "stale"]
        .into_iter()
        .enumerate()
    {
        let second = 11 + index as u64 * 10;
        let mut hb = h.heartbeat(second, vec![], "ok", true);
        match mode {
            "unsupported" => {
                hb["collections"][0]
                    .as_object_mut()
                    .unwrap()
                    .remove("extensions");
            }
            "duplicate" => {
                hb["collections"][0]["extensions"]["usb_device_snapshot"]["devices"] =
                    json!([device("driver-1", None), device("driver-2", None)])
            }
            "reference" => {
                hb["collections"][0]["extensions"]["usb_device_snapshot"]["devices"] =
                    json!([device("missing-driver", None)])
            }
            "version" => {
                hb["collections"][0]["extensions"]["usb_device_snapshot"]["version"] = json!(2)
            }
            _ => {
                hb["monotonic_ns"] = json!(((second + 16) * 1_000_000_000).to_string());
            }
        }
        h.send(&hb).await;
        h.observe(second + 5, vec![], "ok", true).await;
        assert!(h.episodes().await.is_empty(), "{mode}");
    }
}

#[tokio::test]
async fn receipt_failure_rolls_back_watch_attention_and_outbox_together() {
    let mut h = Harness::new().await;
    for second in [1, 6] {
        h.observe(second, vec![device("driver-1", None)], "ok", true)
            .await;
    }
    h.observe(11, vec![], "ok", true).await;
    sqlx::query("CREATE TRIGGER reject_watch_receipt BEFORE INSERT ON cider_receipts BEGIN SELECT RAISE(ABORT,'synthetic persistence failure'); END").execute(&h.state.db).await.unwrap();
    let hb = h.heartbeat(16, vec![], "ok", true);
    let (code, _) = request(
        &h.app,
        "POST",
        "/api/v2/ciderd/heartbeat",
        &h.credential,
        hb.clone(),
    )
    .await;
    assert_eq!(code, 500);
    assert!(h.episodes().await.is_empty());
    assert_eq!(h.queued().await, 0);
    sqlx::query("DROP TRIGGER reject_watch_receipt")
        .execute(&h.state.db)
        .await
        .unwrap();
    h.send(&hb).await;
    assert_eq!(h.episodes().await.len(), 1);
    assert_eq!(h.queued().await, 1);
}

#[tokio::test]
async fn admission_limit_is_visible_and_cannot_evict_established_watch() {
    let mut h = Harness::new().await;
    let devices: Vec<_> = (0..128)
        .map(|i| {
            let mut d = device(&format!("driver-{i}"), None);
            d["identity"] = json!(
                Uuid::new_v5(&Uuid::NAMESPACE_OID, format!("admission-{i}").as_bytes()).to_string()
            );
            d
        })
        .collect();
    let mut hb = h.heartbeat(1, devices.clone(), "ok", true);
    let at = hb["created_at"].clone();
    let host = hb["resources"][0].clone();
    hb["resources"] = json!([host]);
    for i in 0..=128 {
        hb["resources"].as_array_mut().unwrap().push(json!({"resource_id":format!("driver-{i}"),"resource_type":"controller","revision":"1","observed_at":at,"identity_confidence":"host_local","attributes":{"source":"IOBlockStorageDriver","scope":"driver","registry_entry_id":i.to_string()}}));
    }
    h.send(&hb).await;
    h.observe(6, vec![devices[0].clone()], "ok", true).await;
    let mut extra = device("driver-128", None);
    extra["identity"] = json!(Uuid::new_v4().to_string());
    for second in [11, 16] {
        h.observe(second, vec![extra.clone()], "ok", true).await;
    }
    let node = h.node_view().await;
    assert_eq!(node["device_watch"]["admission_limited"], true);
    assert_eq!(node["device_watch"]["watched_devices"], 128);
    assert_eq!(h.episodes().await.len(), 1);
    assert_eq!(h.queued().await, 1);
}

#[test]
fn disk_projection_rejects_two_current_drivers_even_when_only_one_is_watched() {
    let node = json!({"node_id":"node","boot_id":"boot","agent_generation":"1","agent_session_id":"session"});
    let context = json!({"boot_id":"boot","agent_generation":"1","agent_session_id":"session"});
    let disk = json!({"object_id":"disk","io":{"linkage_state":"unknown"}});
    let object = |id| json!({"object_id":id,"node_id":"node","active":true,"topology_state":"resolved","physical_disk_ids":["disk"],"properties":{"ciderd_resource_type":"controller","scope":"driver","source":"IOBlockStorageDriver","ciderd_acquisition":context}});
    let rows = vec![
        json!({"node_id":"node","driver_object_id":"driver-1","attributed":true,"context":{"boot":orchard_server::store::fingerprint(b"boot"),"generation":"1","session":orchard_server::store::fingerprint(b"session"),"clock":"clock"},"projection":{"watch_id":"watch"}}),
    ];
    assert_eq!(
        orchard_server::device_watch::disk_projection(&node, &disk, &[object("driver-1")], &rows)["watch_id"],
        "watch"
    );
    assert!(
        orchard_server::device_watch::disk_projection(
            &node,
            &disk,
            &[object("driver-1"), object("driver-2")],
            &rows
        )
        .is_null()
    );
}

#[tokio::test]
async fn failed_subsecond_attempt_breaks_absence_streak_without_advancing_it() {
    let mut h = Harness::new().await;
    for second in [1, 6] {
        h.observe(second, vec![device("driver-1", None)], "ok", true)
            .await;
    }
    h.observe(11, vec![], "ok", true).await;
    let mut failed = h.heartbeat(12, vec![], "failed", false);
    failed["collections"][0]["started_monotonic_ns"] = json!("11500000000");
    failed["collections"][0]["finished_monotonic_ns"] = json!("11500000000");
    h.send(&failed).await;
    h.observe(16, vec![], "ok", true).await;
    assert!(
        h.episodes().await.is_empty(),
        "A failed latest acquisition interrupts consecutive absence evidence"
    );
    h.observe(21, vec![], "ok", true).await;
    assert_eq!(h.episodes().await.len(), 1);
}

#[tokio::test]
async fn strong_weak_strong_identity_change_preserves_unique_current_projection() {
    let mut h = Harness::new().await;
    for (second, entry) in [
        (1, device("driver-1", Some("5000000000"))),
        (6, device("driver-1", Some("5000000000"))),
        (11, weak_device()),
        (16, weak_device()),
        (21, device("driver-1", Some("5000000000"))),
        (26, device("driver-1", Some("5000000000"))),
    ] {
        h.observe(second, vec![entry], "ok", true).await;
    }
    let rows = h.watch_views(Utc::now().timestamp_millis()).await;
    let context = json!({"boot_id":"boot-1","agent_generation":"1","agent_session_id":"session-1"});
    let node = json!({"node_id":h.node,"boot_id":"boot-1","agent_generation":"1","agent_session_id":"session-1"});
    let driver = Uuid::new_v5(&Uuid::parse_str(&h.node).unwrap(), b"ciderd:driver-1").to_string();
    let objects = vec![
        json!({"object_id":driver,"node_id":h.node,"active":true,"topology_state":"resolved","physical_disk_ids":["disk"],"properties":{"source":"IOBlockStorageDriver","scope":"driver","ciderd_resource_type":"controller","ciderd_acquisition":context}}),
    ];
    let disk = json!({"object_id":"disk"});
    let view = orchard_server::device_watch::disk_projection(&node, &disk, &objects, &rows);
    assert_eq!(view["identity_basis"], "reported_usb_serial");
    assert_eq!(view["negotiated_bps"], "5000000000");
    h.observe(31, vec![], "ok", true).await;
    h.observe(36, vec![], "ok", true).await;
    assert_eq!(
        h.episodes().await.len(),
        1,
        "An obsolete identity must not duplicate the current connection loss"
    );
}

#[tokio::test]
async fn clock_change_without_host_attempt_invalidates_watch_without_restamping() {
    let mut h = Harness::new().await;
    for second in [1, 6] {
        h.observe(
            second,
            vec![device("driver-1", Some("5000000000"))],
            "ok",
            true,
        )
        .await;
    }
    let before = h.watch_views(Utc::now().timestamp_millis()).await[0]["projection"].clone();
    let mut hb = h.heartbeat(11, vec![], "ok", true);
    hb["clock_id"] = json!("clock-2");
    hb["collections"] = json!([]);
    hb["collector_states"] = json!([]);
    h.send(&hb).await;
    let after = h.watch_views(Utc::now().timestamp_millis()).await[0]["projection"].clone();
    assert_ne!(after["observation"]["state"], "current");
    assert_eq!(after["presence"], "unknown");
    assert_eq!(
        after["observation"]["received_at"],
        before["observation"]["received_at"]
    );
    assert_eq!(after["baseline_bps"], "5000000000");
}

#[tokio::test]
async fn retained_old_clock_attempt_cannot_block_fresh_lower_monotonic_segment() {
    let mut h = Harness::new().await;
    h.observe(
        100,
        vec![device("driver-1", Some("5000000000"))],
        "ok",
        true,
    )
    .await;
    let mut old = h.heartbeat(
        105,
        vec![device("driver-1", Some("5000000000"))],
        "ok",
        true,
    );
    old["clock_id"] = json!("clock-2");
    h.send(&old).await;
    let mut fresh = h.heartbeat(1, vec![device("driver-1", Some("5000000000"))], "ok", true);
    fresh["clock_id"] = json!("clock-2");
    fresh["collections"][0]["clock_id"] = json!("clock-2");
    h.send(&fresh).await;
    let rows = h.watch_views(Utc::now().timestamp_millis()).await;
    assert_eq!(rows[0]["projection"]["observation"]["state"], "current");
    assert_eq!(rows[0]["projection"]["presence"], "present");
    for (second, devices) in [
        (6, vec![device("driver-1", Some("5000000000"))]),
        (11, vec![]),
    ] {
        let mut current = h.heartbeat(second, devices, "ok", true);
        current["clock_id"] = json!("clock-2");
        current["collections"][0]["clock_id"] = json!("clock-2");
        h.send(&current).await;
    }
    // A distinct foreign-clock attempt cannot be discarded as an older sample
    // in the current clock, leaving the prior absence streak intact.
    let mut foreign = h.heartbeat(12, vec![], "ok", true);
    foreign["clock_id"] = json!("clock-2");
    foreign["collections"][0]["started_monotonic_ns"] = json!("2000000000");
    foreign["collections"][0]["finished_monotonic_ns"] = json!("2000000000");
    h.send(&foreign).await;
    let rows = h.watch_views(Utc::now().timestamp_millis()).await;
    assert_eq!(rows[0]["projection"]["observation"]["state"], "unavailable");
    let mut current = h.heartbeat(16, vec![], "ok", true);
    current["clock_id"] = json!("clock-2");
    current["collections"][0]["clock_id"] = json!("clock-2");
    h.send(&current).await;
    assert!(h.episodes().await.is_empty());
}

#[tokio::test]
async fn successful_subsecond_presence_breaks_consecutive_absence() {
    let mut h = Harness::new().await;
    for second in [1, 6] {
        h.observe(second, vec![device("driver-1", None)], "ok", true)
            .await;
    }
    h.observe(11, vec![], "ok", true).await;
    let mut present = h.heartbeat(12, vec![device("driver-1", None)], "ok", true);
    present["collections"][0]["started_monotonic_ns"] = json!("11500000000");
    present["collections"][0]["finished_monotonic_ns"] = json!("11500000000");
    h.send(&present).await;
    h.observe(16, vec![], "ok", true).await;
    assert!(h.episodes().await.is_empty());
}

#[tokio::test]
async fn valid_utf8_ids_and_long_timestamp_do_not_overflow_optional_watch_storage() {
    let mut h = Harness::new().await;
    let mut hb = h.heartbeat(1, vec![device("driver-1", Some("5000000000"))], "ok", true);
    let long = "🙂".repeat(512);
    for key in ["boot_id", "agent_session_id", "clock_id"] {
        hb[key] = json!(long);
    }
    hb["collections"][0]["clock_id"] = json!(long);
    hb["collections"][0]["collection_id"] = json!(long);
    hb["collector_states"][0]["last_attempt_id"] = json!(long);
    let date = format!(
        "{}.{}Z",
        Utc::now().format("%Y-%m-%dT%H:%M:%S"),
        "0".repeat(9000)
    );
    hb["collections"][0]["started_at"] = json!(date);
    hb["collections"][0]["finished_at"] = json!(date);
    h.send(&hb).await;
    let view = h.watch_views(Utc::now().timestamp_millis()).await;
    assert_eq!(view[0]["projection"]["negotiated_bps"], "5000000000");
    assert_eq!(view[0]["projection"]["evidence"]["collection_id"], long);
}
