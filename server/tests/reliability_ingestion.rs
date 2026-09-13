use axum::{
    Router,
    body::{Body, to_bytes},
    http::Request,
};
use chrono::{Duration, Utc};
use orchard_server::{api, cider_api, cider_wire, read_api, store::AppState};
use serde_json::{Value, json};
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

async fn request(
    app: &Router,
    method: &str,
    path: &str,
    credential: &str,
    body: Value,
) -> (u16, Value) {
    let request = Request::builder()
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
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status().as_u16();
    let bytes = to_bytes(response.into_body(), 2 * 1024 * 1024)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

impl Harness {
    async fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let state = AppState::open(&directory.path().join("db.sqlite3"), ADMIN)
            .await
            .unwrap();
        let app = routes(&state);
        let token = state.local_enrollment_token().await.unwrap();
        let (status, body) = request(&app, "POST", "/api/v1/nodes/enroll", ADMIN,
            json!({"enrollment_token":token["enrollment_token"],"name":"ciderd integration test","agent":{"version":"0.1.0"}})).await;
        assert_eq!(status, 201, "{body}");
        Self {
            directory,
            state,
            app,
            node: body["data"]["node_id"].as_str().unwrap().into(),
            credential: body["data"]["credential"].as_str().unwrap().into(),
        }
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
        serde_json::from_value::<cider_wire::Heartbeat>(hb.clone())
            .unwrap()
            .validate()
            .unwrap();
        hb
    }

    async fn send(&self, hb: &Value) -> (u16, Value) {
        // The real schema-2 sender does not supply v1 request-ID/timestamp headers.
        let response = self
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v2/ciderd/heartbeat")
                    .header("authorization", format!("Bearer {}", self.credential))
                    .header("content-type", "application/json")
                    .body(Body::from(hb.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status().as_u16();
        assert_eq!(response.headers().get("cache-control").unwrap(), "no-store");
        let bytes = to_bytes(response.into_body(), 2 * 1024 * 1024)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    #[allow(dead_code)]
    async fn samples(&self) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM metric_samples WHERE node_id=?")
            .bind(&self.node)
            .fetch_one(&self.state.db)
            .await
            .unwrap()
    }

    #[allow(dead_code)]
    async fn device(&self) -> Value {
        let id = Uuid::new_v5(&Uuid::parse_str(&self.node).unwrap(), b"ciderd:driver-1");
        let (status, body) = request(
            &self.app,
            "GET",
            &format!("/api/v1/objects/{id}"),
            ADMIN,
            Value::Null,
        )
        .await;
        assert_eq!(status, 200, "{body}");
        body["data"].clone()
    }
}

fn exact_counter(name: &str, unit: &str, value: u128) -> Value {
    json!({
        "name": name,
        "kind": "counter",
        "unit": unit,
        "availability": "available",
        "attributes": {},
        "freshness": "live",
        "value_type": "integer",
        "value": value.to_string(),
        "counter_epoch": "disk-epoch-1"
    })
}

fn smart_pending(value: u128, identity: &str) -> Value {
    json!({
        "name": "storage.ata.current_pending_sectors",
        "kind": "gauge",
        "unit": "sectors",
        "availability": "available",
        "attributes": {},
        "freshness": "live",
        "value_type": "integer",
        "value": value.to_string(),
        "source_field": "ata_smart_attributes.table[id=197,name=Current_Pending_Sector].raw.value",
        "extensions": {
            "device_identity": identity,
            "device_identity_confidence": "reported_serial",
            "smartctl_exit_status": 0,
            "smartctl_exit_status_class": "clear"
        }
    })
}

fn smart_passed(value: bool, identity: &str) -> Value {
    json!({
        "name": "storage.media.smart_passed",
        "kind": "state",
        "unit": "1",
        "availability": "available",
        "attributes": {},
        "freshness": "live",
        "value_type": "boolean",
        "value": value,
        "source_field": "smart_status.passed",
        "extensions": {
            "device_identity": identity,
            "device_identity_confidence": "reported_serial",
            "smartctl_exit_status": 0,
            "smartctl_exit_status_class": "clear"
        }
    })
}

fn make_smart(hb: &mut Value, value: u128, identity: &str) {
    let at = hb["resources"][0]["observed_at"].clone();
    hb["resources"][0] = json!({
        "resource_id": "disk-1",
        "resource_type": "physical_device",
        "revision": "1",
        "observed_at": at,
        "identity_confidence": "host_local",
        "attributes": {
            "source": "diskutil.list.physical",
            "scope": "physical_device",
            "bsd_name": "disk0",
            "smart_eligible": true
        }
    });
    hb["collections"][0] = json!({
        "collection_id": "smart-1",
        "resource_id": "disk-1",
        "collector": "smartctl",
        "adapter_version": "0.1.0",
        "source_version": "smartctl-7.5-json-v1",
        "started_at": at,
        "finished_at": at,
        "clock_id": "test-clock",
        "started_monotonic_ns": "1000000000",
        "finished_monotonic_ns": "1000000000",
        "status": "ok",
        "metrics": [smart_pending(value, identity)],
        "exit_code": 0
    });
    hb["collector_states"][0] = json!({
        "collector": "smartctl",
        "resource_id": "disk-1",
        "phase": "idle",
        "poll_interval_seconds": 300,
        "stale_after_seconds": 900,
        "last_attempt_id": "smart-1"
    });
    serde_json::from_value::<cider_wire::Heartbeat>(hb.clone())
        .unwrap()
        .validate()
        .unwrap();
}

fn next_collection(hb: &mut Value, sequence: u64, collection_id: &str, monotonic_ns: u64) {
    let at = (Utc::now() - Duration::seconds(1)).to_rfc3339();
    hb["sequence"] = json!(sequence.to_string());
    hb["created_at"] = json!(Utc::now().to_rfc3339());
    hb["monotonic_ns"] = json!((monotonic_ns + 200_000_000).to_string());
    hb["inventory"]["included"] = json!(false);
    hb["resources"] = json!([]);
    hb["relationships"] = json!([]);
    hb["tombstones"] = json!([]);
    hb["collections"][0]["collection_id"] = json!(collection_id);
    hb["collections"][0]["started_at"] = json!(at);
    hb["collections"][0]["finished_at"] = json!(at);
    hb["collections"][0]["started_monotonic_ns"] = json!(monotonic_ns.to_string());
    hb["collections"][0]["finished_monotonic_ns"] = json!(monotonic_ns.to_string());
    hb["collector_states"][0]["last_attempt_id"] = json!(collection_id);
}

async fn reliability_rows(h: &Harness) -> Vec<Value> {
    let (status, body) = request(
        &h.app,
        "GET",
        "/api/v1/reliability/sources",
        ADMIN,
        Value::Null,
    )
    .await;
    assert_eq!(status, 200, "{body}");
    body["data"].as_array().unwrap().clone()
}

async fn disk_rows(h: &Harness) -> Vec<Value> {
    let (status, body) = request(
        &h.app,
        "GET",
        &format!("/api/v1/nodes/{}/disks", h.node),
        ADMIN,
        Value::Null,
    )
    .await;
    assert_eq!(status, 200, "{body}");
    body["data"].as_array().unwrap().clone()
}

fn signal<'a>(source: &'a Value, rule_id: &str) -> &'a Value {
    source["signals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|signal| signal["rule_id"] == rule_id)
        .unwrap()
}

#[tokio::test]
async fn accepted_smart_ata_gauge_is_projected_and_opens_exact_evidence() {
    let h = Harness::new().await;
    let mut hb = h.heartbeat();
    let device_identity = "8c5377d4-e78a-50f0-8313-e11388bc1752";
    make_smart(&mut hb, 2, device_identity);
    let (status, body) = h.send(&hb).await;
    assert_eq!(status, 200, "{body}");

    let projected: (String, String, String) =
        sqlx::query_as("SELECT name,kind,unit FROM metric_samples WHERE node_id=?")
            .bind(&h.node)
            .fetch_one(&h.state.db)
            .await
            .unwrap();
    assert_eq!(
        projected,
        (
            "ata_current_pending_sectors".into(),
            "gauge".into(),
            "sectors".into()
        )
    );

    let sources = reliability_rows(&h).await;
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0]["collector"], "smartctl");
    assert_eq!(sources[0]["device_identity_confidence"], "reported_serial");
    assert!(sources[0].get("device_identity").is_none());
    let private_state: Value = serde_json::from_str(
        &sqlx::query_scalar::<_, String>("SELECT state_json FROM reliability_sources")
            .fetch_one(&h.state.db)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(private_state["device_identity"], device_identity);
    let (status, findings) = request(
        &h.app,
        "GET",
        "/api/v1/reliability/findings",
        ADMIN,
        Value::Null,
    )
    .await;
    assert_eq!(status, 200, "{findings}");
    assert_eq!(findings["data"].as_array().unwrap().len(), 1);
    let finding = &findings["data"][0];
    assert_eq!(finding["rule_id"], "ata.pending_sectors");
    assert_eq!(finding["status"], "open");
    assert_eq!(finding["evidence"]["value"], "2");
    assert_eq!(finding["evidence"]["smartctl_exit_status"], 0);
    assert_eq!(finding["evidence"]["scope"], "device");
}

#[tokio::test]
async fn partial_smart_keeps_unobserved_open_signal_unknown_and_stale_owner_cannot_reassure() {
    let h = Harness::new().await;
    let mut hb = h.heartbeat();
    let device_identity = "8c5377d4-e78a-50f0-8313-e11388bc1752";
    make_smart(&mut hb, 2, device_identity);
    assert_eq!(h.send(&hb).await.0, 200);

    next_collection(&mut hb, 2, "smart-partial-2", 2_000_000_000);
    hb["collections"][0]["status"] = json!("partial");
    hb["collections"][0]["metrics"] = json!([smart_passed(true, device_identity)]);
    assert_eq!(h.send(&hb).await.0, 200);

    let sources = reliability_rows(&h).await;
    let pending = signal(&sources[0], "ata.pending_sectors");
    assert_eq!(sources[0]["observation"]["state"], "current");
    assert_eq!(pending["state"], "unknown");
    assert_eq!(pending["observation"]["state"], "unavailable");
    assert!(pending["finding_id"].is_string());
    assert_eq!(sources[0]["findings"].as_array().unwrap().len(), 1);

    let disks = disk_rows(&h).await;
    let reliability = &disks[0]["reliability"];
    assert_eq!(reliability["assessment"], "unknown");
    assert_eq!(reliability["observation_state"], "current");
    assert_eq!(reliability["findings"][0]["current"], false);
    assert!(
        reliability["unknown_dimensions"]
            .as_array()
            .unwrap()
            .contains(&json!("media_health"))
    );

    sqlx::query("UPDATE nodes SET last_seen_at=? WHERE node_id=?")
        .bind((Utc::now() - Duration::seconds(31)).timestamp_millis())
        .bind(&h.node)
        .execute(&h.state.db)
        .await
        .unwrap();
    let disks = disk_rows(&h).await;
    let reliability = &disks[0]["reliability"];
    assert_eq!(reliability["assessment"], "unknown");
    assert_eq!(reliability["observation_state"], "stale");
    assert_eq!(reliability["findings"][0]["current"], false);
}

#[tokio::test]
async fn replacement_reusing_a_device_locator_never_inherits_prior_findings() {
    let h = Harness::new().await;
    let mut hb = h.heartbeat();
    make_smart(&mut hb, 2, "8c5377d4-e78a-50f0-8313-e11388bc1752");
    assert_eq!(h.send(&hb).await.0, 200);
    let old_object = reliability_rows(&h).await[0]["object_id"]
        .as_str()
        .unwrap()
        .to_owned();

    let at = (Utc::now() - Duration::seconds(1)).to_rfc3339();
    hb["sequence"] = json!("2");
    hb["created_at"] = json!(Utc::now().to_rfc3339());
    hb["monotonic_ns"] = json!("2200000000");
    hb["inventory"] = json!({"revision":"2","included":true});
    hb["resources"] = json!([{
        "resource_id": "disk-2",
        "resource_type": "physical_device",
        "revision": "1",
        "observed_at": at,
        "identity_confidence": "host_local",
        "attributes": {
            "source": "diskutil.list.physical",
            "scope": "physical_device",
            "bsd_name": "disk0",
            "smart_eligible": true
        }
    }]);
    hb["relationships"] = json!([]);
    hb["collections"] = json!([]);
    hb["collector_states"] = json!([]);
    hb["tombstones"] = json!([{
        "entity_type": "resource",
        "entity_id": "disk-1",
        "revision": "2",
        "observed_at": Utc::now().to_rfc3339(),
        "reason": "replaced"
    }]);
    serde_json::from_value::<cider_wire::Heartbeat>(hb.clone())
        .unwrap()
        .validate()
        .unwrap();
    assert_eq!(h.send(&hb).await.0, 200);

    let disks = disk_rows(&h).await;
    assert_eq!(disks.len(), 1);
    assert_eq!(disks[0]["bsd_name"], "disk0");
    assert_eq!(disks[0]["reliability"]["assessment"], "unknown");
    assert_eq!(disks[0]["reliability"]["source_count"], 0);
    assert!(
        disks[0]["reliability"]["findings"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    let (status, body) = request(
        &h.app,
        "GET",
        "/api/v1/reliability/findings?status=all",
        ADMIN,
        Value::Null,
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let findings = body["data"].as_array().unwrap();
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0]["status"], "interrupted");
    assert_eq!(findings[0]["reason"], "source_removed");
    assert_eq!(findings[0]["object_id"], old_object);
}

#[tokio::test]
async fn iokit_history_then_exact_delta_is_idempotent_across_receipt_and_batch_replay() {
    let h = Harness::new().await;
    let mut hb = h.heartbeat();
    hb["collections"][0]["metrics"] = json!([
        exact_counter("storage.device.read_bytes_total", "bytes", LARGE),
        exact_counter("storage.device.read_errors_total", "errors", LARGE)
    ]);
    assert_eq!(h.send(&hb).await.0, 200);
    let initial_findings: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM reliability_findings")
        .fetch_one(&h.state.db)
        .await
        .unwrap();
    assert_eq!(initial_findings, 0, "a lifetime total is only history");

    next_collection(&mut hb, 2, "errors-2", 2_000_000_000);
    hb["collections"][0]["metrics"][1]["value"] = json!((LARGE + 1).to_string());
    assert_eq!(h.send(&hb).await.0, 200);
    let finding: Value = serde_json::from_str(
        &sqlx::query_scalar::<_, String>(
            "SELECT finding_json FROM reliability_findings WHERE status='open'",
        )
        .fetch_one(&h.state.db)
        .await
        .unwrap(),
    )
    .unwrap();
    assert_eq!(finding["rule_id"], "iokit.read_errors");
    assert_eq!(finding["evidence"]["previous"], LARGE.to_string());
    assert_eq!(finding["evidence"]["value"], (LARGE + 1).to_string());
    assert_eq!(finding["evidence"]["delta"], "1");
    let source_before: String = sqlx::query_scalar("SELECT state_json FROM reliability_sources")
        .fetch_one(&h.state.db)
        .await
        .unwrap();

    let replay_ack = h.send(&hb).await;
    assert_eq!(replay_ack.0, 200, "{}", replay_ack.1);
    hb["sequence"] = json!("3");
    hb["created_at"] = json!(Utc::now().to_rfc3339());
    hb["monotonic_ns"] = json!("3200000000");
    assert_eq!(h.send(&hb).await.0, 200);
    let source_after: String = sqlx::query_scalar("SELECT state_json FROM reliability_sources")
        .fetch_one(&h.state.db)
        .await
        .unwrap();
    assert_eq!(source_after, source_before);
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM reliability_findings")
        .fetch_one(&h.state.db)
        .await
        .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn receipt_failure_rolls_back_reliability_state_and_can_retry() {
    let h = Harness::new().await;
    let mut hb = h.heartbeat();
    hb["collections"][0]["metrics"] = json!([
        exact_counter("storage.device.read_bytes_total", "bytes", LARGE),
        exact_counter("storage.device.read_errors_total", "errors", 7)
    ]);
    assert_eq!(h.send(&hb).await.0, 200);
    let before: String = sqlx::query_scalar("SELECT state_json FROM reliability_sources")
        .fetch_one(&h.state.db)
        .await
        .unwrap();
    sqlx::raw_sql(
        "CREATE TRIGGER reject_reliability_receipt BEFORE INSERT ON cider_receipts BEGIN SELECT RAISE(ABORT,'test rollback'); END;",
    )
    .execute(&h.state.db)
    .await
    .unwrap();
    next_collection(&mut hb, 2, "rollback-errors-2", 2_000_000_000);
    hb["collections"][0]["metrics"][1]["value"] = json!("8");
    assert_eq!(h.send(&hb).await.0, 500);
    let after: String = sqlx::query_scalar("SELECT state_json FROM reliability_sources")
        .fetch_one(&h.state.db)
        .await
        .unwrap();
    assert_eq!(after, before);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM reliability_findings")
            .fetch_one(&h.state.db)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM cider_records WHERE entity_id='rollback-errors-2'",
        )
        .fetch_one(&h.state.db)
        .await
        .unwrap(),
        0
    );
    sqlx::query("DROP TRIGGER reject_reliability_receipt")
        .execute(&h.state.db)
        .await
        .unwrap();
    assert_eq!(h.send(&hb).await.0, 200);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM reliability_findings")
            .fetch_one(&h.state.db)
            .await
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn new_agent_session_interrupts_old_evidence_and_restart_retains_open_finding() {
    let h = Harness::new().await;
    let mut hb = h.heartbeat();
    let device_identity = "8c5377d4-e78a-50f0-8313-e11388bc1752";
    make_smart(&mut hb, 2, device_identity);
    assert_eq!(h.send(&hb).await.0, 200);

    let at = (Utc::now() - Duration::seconds(1)).to_rfc3339();
    hb["boot_id"] = json!("test-boot-2");
    hb["agent_session_id"] = json!("test-session-2");
    hb["agent_generation"] = json!("2");
    hb["sequence"] = json!("1");
    hb["clock_id"] = json!("test-clock-2");
    hb["created_at"] = json!(Utc::now().to_rfc3339());
    hb["monotonic_ns"] = json!("2200000000");
    hb["inventory"]["revision"] = json!("1");
    hb["inventory"]["included"] = json!(true);
    hb["resources"][0]["observed_at"] = json!(at);
    hb["collections"][0]["collection_id"] = json!("smart-session-2");
    hb["collections"][0]["clock_id"] = json!("test-clock-2");
    hb["collections"][0]["started_at"] = json!(at);
    hb["collections"][0]["finished_at"] = json!(at);
    hb["collections"][0]["started_monotonic_ns"] = json!("2000000000");
    hb["collections"][0]["finished_monotonic_ns"] = json!("2000000000");
    hb["collector_states"][0]["last_attempt_id"] = json!("smart-session-2");
    assert_eq!(h.send(&hb).await.0, 200);

    let statuses: Vec<String> =
        sqlx::query_scalar("SELECT status FROM reliability_findings ORDER BY status")
            .fetch_all(&h.state.db)
            .await
            .unwrap();
    assert_eq!(statuses, vec!["interrupted", "open"]);
    let interrupted: Value = serde_json::from_str(
        &sqlx::query_scalar::<_, String>(
            "SELECT finding_json FROM reliability_findings WHERE status='interrupted'",
        )
        .fetch_one(&h.state.db)
        .await
        .unwrap(),
    )
    .unwrap();
    assert_eq!(interrupted["reason"], "identity_changed");

    let reopened = AppState::open(&h.directory.path().join("db.sqlite3"), ADMIN)
        .await
        .unwrap();
    let reopened_app = routes(&reopened);
    let (status, body) = request(
        &reopened_app,
        "GET",
        "/api/v1/reliability/findings?status=all",
        ADMIN,
        Value::Null,
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["data"].as_array().unwrap().len(), 2);

    let ended_at: i64 =
        sqlx::query_scalar("SELECT ended_at FROM reliability_findings WHERE status='interrupted'")
            .fetch_one(&reopened.db)
            .await
            .unwrap();
    let retention_ms = orchard_server::reliability::Policy::default()
        .closed_finding_retention_seconds as i64
        * 1000;
    let mut tx = reopened.db.begin().await.unwrap();
    orchard_server::reliability_store::retain(&mut tx, ended_at + retention_ms)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM reliability_findings")
            .fetch_one(&reopened.db)
            .await
            .unwrap(),
        2
    );
    let mut tx = reopened.db.begin().await.unwrap();
    orchard_server::reliability_store::retain(&mut tx, ended_at + retention_ms + 1)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let retained: Vec<String> =
        sqlx::query_scalar("SELECT status FROM reliability_findings ORDER BY status")
            .fetch_all(&reopened.db)
            .await
            .unwrap();
    assert_eq!(retained, vec!["open"]);
}
