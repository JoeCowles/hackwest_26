use axum::{
    Router,
    body::{Body, to_bytes},
    http::Request,
};
use chrono::{Duration, Utc};
use orchard_server::{api, cider_api, cider_wire, read_api, store::AppState};
use serde_json::{Value, json};
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

#[tokio::test]
async fn migration_and_native_ingestion_admit_both_directions() {
    let h = Harness::new().await;
    let hb = h.heartbeat();
    assert_eq!(h.send(&hb).await.0, 200);
    let version: i64 = sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(&h.state.db)
        .await
        .unwrap();
    assert_eq!(
        version, 5,
        "Native detector state must be migrated atomically at startup"
    );
    let rows = sqlx::query(
        "SELECT direction,active,support_state FROM detection_sources ORDER BY direction",
    )
    .fetch_all(&h.state.db)
    .await
    .unwrap();
    assert_eq!(
        rows.len(),
        2,
        "Missing write observations must still have explicit source coverage"
    );
    assert_eq!(rows[0].get::<String, _>("direction"), "read");
    assert_eq!(rows[1].get::<String, _>("direction"), "write");
    for row in rows {
        assert_eq!(row.get::<i64, _>("active"), 1);
        assert_eq!(row.get::<String, _>("support_state"), "supported");
    }
}

async fn summaries(h: &Harness, now: i64) -> Vec<Value> {
    let mut tx = h.state.db.begin().await.unwrap();
    orchard_server::detection_store::source_summaries(&mut tx, now)
        .await
        .unwrap()
}

#[tokio::test]
async fn failed_empty_attempt_invalidates_both_directions_without_fabricating_zero() {
    let h = Harness::new().await;
    let mut hb = h.heartbeat();
    assert_eq!(h.send(&hb).await.0, 200);
    hb["sequence"] = json!("2");
    hb["collections"][0]["collection_id"] = json!("failed-2");
    hb["collections"][0]["status"] = json!("failed");
    hb["collections"][0]["metrics"] = json!([]);
    hb["collections"][0]["started_monotonic_ns"] = json!("2000000000");
    hb["collections"][0]["finished_monotonic_ns"] = json!("2000000000");
    hb["monotonic_ns"] = json!("2200000000");
    hb["collector_states"][0]["last_attempt_id"] = json!("failed-2");
    assert_eq!(h.send(&hb).await.0, 200);
    let rows = summaries(&h, Utc::now().timestamp_millis()).await;
    assert_eq!(rows.len(), 2);
    for row in rows {
        assert_eq!(row["observation"]["state"], "unavailable");
        assert_eq!(row["observation"]["reason"], "collection_failed");
        assert!(row["observation"]["rate_bytes_per_second"].is_null());
        assert_eq!(row["baseline"]["interval_count"], 0);
    }
}

#[tokio::test]
async fn replay_preserves_state_and_failure_rolls_back_detector_with_receipt() {
    let h = Harness::new().await;
    let mut hb = h.heartbeat();
    assert_eq!(h.send(&hb).await.0, 200);
    let before: Vec<(String, String)> =
        sqlx::query_as("SELECT source_id,state_json FROM detection_sources ORDER BY source_id")
            .fetch_all(&h.state.db)
            .await
            .unwrap();
    assert_eq!(h.send(&hb).await.0, 200);
    hb["sequence"] = json!("2");
    assert_eq!(h.send(&hb).await.0, 200);
    assert_eq!(before.len(), 2);
    let after: Vec<(String, String)> =
        sqlx::query_as("SELECT source_id,state_json FROM detection_sources ORDER BY source_id")
            .fetch_all(&h.state.db)
            .await
            .unwrap();
    assert_eq!(before, after);
    sqlx::raw_sql("CREATE TRIGGER reject_detection_receipt BEFORE INSERT ON cider_receipts BEGIN SELECT RAISE(ABORT,'test rollback'); END;").execute(&h.state.db).await.unwrap();
    hb["sequence"] = json!("3");
    hb["monotonic_ns"] = json!("6200000000");
    hb["collections"][0]["collection_id"] = json!("rollback-3");
    hb["collections"][0]["started_monotonic_ns"] = json!("6000000000");
    hb["collections"][0]["finished_monotonic_ns"] = json!("6000000000");
    hb["collections"][0]["started_at"] = json!(Utc::now().to_rfc3339());
    hb["collections"][0]["finished_at"] = hb["collections"][0]["started_at"].clone();
    hb["collections"][0]["metrics"][0]["value"] = json!((LARGE + 10_000_000).to_string());
    hb["collector_states"][0]["last_attempt_id"] = json!("rollback-3");
    assert_eq!(h.send(&hb).await.0, 500);
    let after: Vec<(String, String)> =
        sqlx::query_as("SELECT source_id,state_json FROM detection_sources ORDER BY source_id")
            .fetch_all(&h.state.db)
            .await
            .unwrap();
    assert_eq!(before, after);
    let rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM cider_records WHERE entity_id='rollback-3'")
            .fetch_one(&h.state.db)
            .await
            .unwrap();
    assert_eq!(rows, 0);
}

#[tokio::test]
async fn current_resource_removal_and_unrelated_inventory_revision_are_distinct() {
    let h = Harness::new().await;
    let mut hb = h.heartbeat();
    assert_eq!(h.send(&hb).await.0, 200);
    let before: Vec<(String, String)> =
        sqlx::query_as("SELECT source_id,state_json FROM detection_sources ORDER BY source_id")
            .fetch_all(&h.state.db)
            .await
            .unwrap();
    hb["sequence"] = json!("2");
    hb["inventory"]["revision"] = json!("2");
    hb["resources"] = json!([]);
    hb["collections"] = json!([]);
    hb["collector_states"] = json!([]);
    assert_eq!(h.send(&hb).await.0, 200);
    assert_eq!(before.len(), 2);
    let after: Vec<(String, String)> =
        sqlx::query_as("SELECT source_id,state_json FROM detection_sources ORDER BY source_id")
            .fetch_all(&h.state.db)
            .await
            .unwrap();
    assert_eq!(
        before, after,
        "An inventory revision alone does not change detector identity"
    );
    hb["sequence"] = json!("3");
    hb["inventory"]["revision"] = json!("3");
    hb["tombstones"] = json!([{"entity_type":"resource","entity_id":"driver-1","revision":"2","observed_at":Utc::now().to_rfc3339(),"reason":"removed"}]);
    assert_eq!(h.send(&hb).await.0, 200);
    let rows = summaries(&h, Utc::now().timestamp_millis()).await;
    assert_eq!(rows.len(), 2);
    for row in rows {
        assert_eq!(row["active"], false);
        assert_ne!(row["observation"]["state"], "current");
    }
}

#[tokio::test]
async fn rebaseline_is_revision_guarded_audited_and_survives_restart() {
    let h = Harness::new().await;
    let hb = h.heartbeat();
    assert_eq!(h.send(&hb).await.0, 200);
    let source: String =
        sqlx::query_scalar("SELECT source_id FROM detection_sources WHERE direction='read'")
            .fetch_one(&h.state.db)
            .await
            .unwrap();
    let mut tx = h.state.db.begin().await.unwrap();
    let err = orchard_server::detection_store::rebaseline(
        &mut tx,
        &source,
        2,
        "planned_workload_change",
        "actor",
        Utc::now().timestamp_millis(),
    )
    .await
    .unwrap_err();
    assert_eq!(err.status.as_u16(), 409);
    tx.rollback().await.unwrap();
    let mut tx = h.state.db.begin().await.unwrap();
    let summary = orchard_server::detection_store::rebaseline(
        &mut tx,
        &source,
        1,
        "planned_workload_change",
        "actor",
        Utc::now().timestamp_millis(),
    )
    .await
    .unwrap();
    assert_eq!(summary["baseline_revision"], "2");
    tx.commit().await.unwrap();
    let count:i64=sqlx::query_scalar("SELECT count(*) FROM detection_rebaseline_audit WHERE previous_revision='1' AND new_revision='2' AND actor_hash='actor'").fetch_one(&h.state.db).await.unwrap();
    assert_eq!(count, 1);
    let reopened = AppState::open(&h.directory.path().join("db.sqlite3"), ADMIN)
        .await
        .unwrap();
    let mut tx = reopened.db.begin().await.unwrap();
    let rows =
        orchard_server::detection_store::source_summaries(&mut tx, Utc::now().timestamp_millis())
            .await
            .unwrap();
    assert_eq!(
        rows.iter().find(|v| v["source_id"] == source).unwrap()["baseline_revision"],
        "2"
    );
}

#[tokio::test]
async fn timing_unknown_survives_persistence_and_read_freshness() {
    let h = Harness::new().await;
    let mut hb = h.heartbeat();
    // A timestamp accepted by transport can still be ineligible for present-activity evidence.
    hb["created_at"] = json!((Utc::now() - Duration::seconds(30)).to_rfc3339());
    assert_eq!(h.send(&hb).await.0, 200);
    let rows = summaries(&h, Utc::now().timestamp_millis()).await;
    assert_eq!(rows.len(), 2);
    for row in rows {
        assert_eq!(row["observation"]["state"], "unavailable");
        assert_eq!(
            row["observation"]["reason"],
            if row["direction"] == "read" {
                "timing_unknown"
            } else {
                "missing_metric"
            }
        );
    }
    hb["sequence"] = json!("2");
    hb["created_at"] = json!(Utc::now().to_rfc3339());
    hb["collections"][0]["collection_id"] = json!("clock-mismatch");
    hb["collections"][0]["clock_id"] = json!("different-clock");
    hb["collector_states"][0]["last_attempt_id"] = json!("clock-mismatch");
    // Cross-clock collections are a legal transport record, but never a detector endpoint.
    let parsed: cider_wire::Heartbeat = serde_json::from_value(hb).unwrap();
    let mut tx = h.state.db.begin().await.unwrap();
    orchard_server::detection_store::observe_collection(
        &mut tx,
        &parsed,
        &parsed.resources[0],
        &parsed.collections[0],
        Utc::now().timestamp_millis(),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let states: Vec<String> = sqlx::query_scalar("SELECT state_json FROM detection_sources")
        .fetch_all(&h.state.db)
        .await
        .unwrap();
    for state in states {
        serde_json::from_str::<orchard_server::detection::SourceState>(&state)
            .expect("All persisted numeric values must remain deserializable");
    }
}

#[tokio::test]
async fn admission_limit_exposes_exclusion_and_preserves_native_telemetry() {
    let h = Harness::new().await;
    let mut hb = h.heartbeat();
    assert_eq!(h.send(&hb).await.0, 200);
    // Fill the real production limit using compact valid states; no shortened test policy.
    sqlx::raw_sql("WITH RECURSIVE numbers(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM numbers WHERE n<8190) INSERT INTO detection_sources(source_id,node_id,object_id,resource_id,direction,active,support_state,state_json,summary_json,boot_id,agent_generation,agent_session_id,updated_at) SELECT printf('00000000-0000-4000-8000-%012d',n),s.node_id,s.object_id,s.resource_id,s.direction,1,'supported',json_set(s.state_json,'$.source.source_id',printf('00000000-0000-4000-8000-%012d',n)),json_set(s.summary_json,'$.source_id',printf('00000000-0000-4000-8000-%012d',n)),s.boot_id,s.agent_generation,s.agent_session_id,s.updated_at FROM numbers CROSS JOIN (SELECT * FROM detection_sources WHERE direction='read' LIMIT 1) s;")
        .execute(&h.state.db).await.unwrap();
    hb["sequence"] = json!("2");
    hb["inventory"]["revision"] = json!("2");
    hb["resources"][0]["resource_id"] = json!("driver-2");
    hb["collections"][0]["resource_id"] = json!("driver-2");
    hb["collections"][0]["collection_id"] = json!("limited-2");
    hb["collector_states"][0]["resource_id"] = json!("driver-2");
    hb["collector_states"][0]["last_attempt_id"] = json!("limited-2");
    assert_eq!(
        h.send(&hb).await.0,
        200,
        "Admission exhaustion must never reject otherwise valid native telemetry"
    );
    assert_eq!(h.samples().await, 2);
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM detection_sources WHERE active=1 AND support_state='supported'",
    )
    .fetch_one(&h.state.db)
    .await
    .unwrap();
    assert_eq!(count, 8192);
    let rows: Vec<String> = sqlx::query_scalar(
        "SELECT summary_json FROM detection_sources WHERE resource_id='driver-2'",
    )
    .fetch_all(&h.state.db)
    .await
    .unwrap();
    assert_eq!(rows.len(), 2);
    for row in rows {
        let row: Value = serde_json::from_str(&row).unwrap();
        assert_eq!(row["support_state"], "capacity_limited");
        assert_eq!(row["support_reason"], "active_source_limit");
        assert!(row["observation"]["rate_bytes_per_second"].is_null());
    }
    sqlx::query("UPDATE detection_sources SET active=0 WHERE source_id IN ('00000000-0000-4000-8000-000000000001','00000000-0000-4000-8000-000000000002')").execute(&h.state.db).await.unwrap();
    assert_eq!(h.send(&hb).await.0, 200);
    let count:i64=sqlx::query_scalar("SELECT count(*) FROM detection_sources WHERE resource_id='driver-2' AND support_state='capacity_limited'").fetch_one(&h.state.db).await.unwrap();
    assert_eq!(count, 2, "Retry cannot readmit an old attempt");
    hb["sequence"] = json!("3");
    hb["collections"][0]["collection_id"] = json!("admitted-3");
    hb["collector_states"][0]["last_attempt_id"] = json!("admitted-3");
    hb["collections"][0]["started_monotonic_ns"] = json!("6000000000");
    hb["collections"][0]["finished_monotonic_ns"] = json!("6000000000");
    hb["monotonic_ns"] = json!("6200000000");
    hb["collections"][0]["started_at"] = json!(Utc::now().to_rfc3339());
    hb["collections"][0]["finished_at"] = hb["collections"][0]["started_at"].clone();
    assert_eq!(h.send(&hb).await.0, 200);
    let count:i64=sqlx::query_scalar("SELECT count(*) FROM detection_sources WHERE resource_id='driver-2' AND support_state='supported'").fetch_one(&h.state.db).await.unwrap();
    assert_eq!(count, 2);
}

async fn observe_at(
    state: &AppState,
    hb: &mut cider_wire::Heartbeat,
    t: u64,
    counter: u128,
    base: i64,
) {
    let at = orchard_server::store::timestamp(base + t as i64 * 1000);
    hb.sequence = (t + 1).into();
    hb.created_at = at.clone();
    hb.resources[0].observed_at = at.clone();
    hb.monotonic_ns = (t * 1_000_000_000 + 200_000_000).into();
    let c = &mut hb.collections[0];
    c.collection_id = format!("synthetic-{t}");
    c.started_at = at.clone();
    c.finished_at = at;
    c.started_monotonic_ns = (t * 1_000_000_000).into();
    c.finished_monotonic_ns = c.started_monotonic_ns;
    c.metrics[0].value = Some(json!(counter.to_string()));
    hb.collector_states[0].last_attempt_id = Some(c.collection_id.clone());
    hb.validate().unwrap();
    let mut tx = state.db.begin().await.unwrap();
    orchard_server::detection_store::observe_collection(
        &mut tx,
        hb,
        &hb.resources[0],
        &hb.collections[0],
        base + t as i64 * 1000,
    )
    .await
    .unwrap();
    sqlx::query("UPDATE nodes SET last_seen_at=? WHERE node_id=?")
        .bind(base + t as i64 * 1000)
        .bind(&hb.node_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn full_duration_finding_survives_restart_and_retention_preserves_unresolved() {
    let h = Harness::new().await;
    let mut hb: cider_wire::Heartbeat = serde_json::from_value(h.heartbeat()).unwrap();
    let base = Utc::now().timestamp_millis() - 800_000;
    for n in 0..=120 {
        observe_at(
            &h.state,
            &mut hb,
            n * 5,
            LARGE + u128::from(n) * 10_000_000,
            base,
        )
        .await;
    }
    for n in 1..=24 {
        observe_at(
            &h.state,
            &mut hb,
            600 + n * 5,
            LARGE + 1_200_000_000 + u128::from(n) * 40_000_000,
            base,
        )
        .await;
    }
    let open: Value = serde_json::from_str(
        &sqlx::query_scalar::<_, String>("SELECT finding_json FROM detection_findings")
            .fetch_one(&h.state.db)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(open["status"], "open");
    assert_eq!(open["evidence"]["elevated_seconds"], 120.);
    assert_eq!(
        open["baseline"]["reference_rate_bytes_per_second"],
        2_000_000.
    );
    let reopened = AppState::open(&h.directory.path().join("db.sqlite3"), ADMIN)
        .await
        .unwrap();
    // Same monotonic watermark is a no-op after process persistence/reload.
    observe_at(&reopened, &mut hb, 720, LARGE + 2_160_000_000, base).await;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM detection_findings")
        .fetch_one(&reopened.db)
        .await
        .unwrap();
    assert_eq!(count, 1);
    let mut tx = reopened.db.begin().await.unwrap();
    orchard_server::detection_store::retain(&mut tx, base + 40 * 86_400_000)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM detection_findings WHERE status='open'")
            .fetch_one(&reopened.db)
            .await
            .unwrap();
    assert_eq!(
        count, 1,
        "Unresolved evidence survives past closed retention"
    );
    for n in 1..=12 {
        observe_at(
            &reopened,
            &mut hb,
            720 + n * 5,
            LARGE + 2_160_000_000 + u128::from(n) * 20_000_000,
            base,
        )
        .await;
    }
    let resolved: Value = serde_json::from_str(
        &sqlx::query_scalar::<_, String>("SELECT finding_json FROM detection_findings")
            .fetch_one(&reopened.db)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(resolved["finding_id"], open["finding_id"]);
    assert_eq!(resolved["status"], "resolved");
    assert_eq!(resolved["evidence"]["recovery_seconds"], 60.);
    let ended = base + 780_000;
    let mut tx = reopened.db.begin().await.unwrap();
    orchard_server::detection_store::retain(&mut tx, ended + 30 * 86_400_000)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM detection_findings")
        .fetch_one(&reopened.db)
        .await
        .unwrap();
    assert_eq!(count, 1);
    let mut tx = reopened.db.begin().await.unwrap();
    orchard_server::detection_store::retain(&mut tx, ended + 30 * 86_400_000 + 1)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM detection_findings")
        .fetch_one(&reopened.db)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn migration_from_schema_two_preserves_preexisting_data() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("old.sqlite3");
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&path)
        .create_if_missing(true);
    let db = sqlx::SqlitePool::connect_with(options).await.unwrap();
    sqlx::raw_sql(include_str!("../migrations/001_init.sql"))
        .execute(&db)
        .await
        .unwrap();
    sqlx::raw_sql(include_str!("../migrations/002_ciderd.sql"))
        .execute(&db)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO enrollment_tokens(token_hash,expires_at) VALUES ('preserved',9999999999999)",
    )
    .execute(&db)
    .await
    .unwrap();
    db.close().await;
    let reopened = AppState::open(&path, ADMIN).await.unwrap();
    let version: i64 = sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(&reopened.db)
        .await
        .unwrap();
    assert_eq!(version, 5);
    let value: String = sqlx::query_scalar("SELECT token_hash FROM enrollment_tokens")
        .fetch_one(&reopened.db)
        .await
        .unwrap();
    assert_eq!(value, "preserved");
}

#[tokio::test]
async fn unrelated_collection_clock_cannot_interrupt_a_current_open_finding() {
    let h = Harness::new().await;
    let mut hb: cider_wire::Heartbeat = serde_json::from_value(h.heartbeat()).unwrap();
    let base = Utc::now().timestamp_millis() - 725_000;
    for n in 0..=120 {
        observe_at(
            &h.state,
            &mut hb,
            n * 5,
            LARGE + u128::from(n) * 10_000_000,
            base,
        )
        .await;
    }
    for n in 1..=24 {
        observe_at(
            &h.state,
            &mut hb,
            600 + n * 5,
            LARGE + 1_200_000_000 + u128::from(n) * 40_000_000,
            base,
        )
        .await;
    }
    let original: String =
        sqlx::query_scalar("SELECT finding_id FROM detection_findings WHERE status='open'")
            .fetch_one(&h.state.db)
            .await
            .unwrap();
    // The schema explicitly allows retained acquisitions from another clock.
    // Its newer wall time is not authority to change the heartbeat's clock.
    hb.sequence = 726u64.into();
    hb.created_at = orchard_server::store::timestamp(base + 725_000);
    hb.monotonic_ns = 725_200_000_000u64.into();
    hb.collections[0].clock_id = "unrelated-retained-clock".into();
    hb.collections[0].collection_id = "wrong-clock-attempt".into();
    hb.collections[0].started_at = hb.created_at.clone();
    hb.collections[0].finished_at = hb.created_at.clone();
    hb.collector_states[0].last_attempt_id = Some("wrong-clock-attempt".into());
    hb.validate().unwrap();
    let (status, response) = h.send(&serde_json::to_value(&hb).unwrap()).await;
    assert_eq!(status, 200, "{response}");
    let finding: Value = serde_json::from_str(
        &sqlx::query_scalar::<_, String>(
            "SELECT finding_json FROM detection_findings WHERE finding_id=?",
        )
        .bind(&original)
        .fetch_one(&h.state.db)
        .await
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        finding["status"], "open",
        "Timing-ineligible evidence must preserve unresolved findings"
    );
    let read_state: Value = serde_json::from_str(
        &sqlx::query_scalar::<_, String>(
            "SELECT summary_json FROM detection_sources WHERE direction='read'",
        )
        .fetch_one(&h.state.db)
        .await
        .unwrap(),
    )
    .unwrap();
    assert_eq!(read_state["baseline_revision"], "1");
    assert_eq!(read_state["observation"]["reason"], "timing_unknown");
    hb.collections[0].clock_id = hb.clock_id.clone();
    observe_at(&h.state, &mut hb, 730, LARGE + 2_240_000_000, base).await;
    let rows = sqlx::query("SELECT finding_id,status FROM detection_findings")
        .fetch_all(&h.state.db)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get::<String, _>("finding_id"), original);
    assert_eq!(rows[0].get::<String, _>("status"), "open");
    let resumed: Value = serde_json::from_str(
        &sqlx::query_scalar::<_, String>(
            "SELECT summary_json FROM detection_sources WHERE direction='read'",
        )
        .fetch_one(&h.state.db)
        .await
        .unwrap(),
    )
    .unwrap();
    assert_eq!(resumed["baseline_revision"], "1");
}

#[tokio::test]
async fn oversized_source_selection_can_be_narrowed_without_breaking_core_warnings() {
    let h = Harness::new().await;
    assert_eq!(h.send(&h.heartbeat()).await.0, 200);
    let object: String = sqlx::query_scalar("SELECT object_id FROM detection_sources LIMIT 1")
        .fetch_one(&h.state.db)
        .await
        .unwrap();
    sqlx::raw_sql("WITH RECURSIVE numbers(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM numbers WHERE n<10000) INSERT INTO detection_sources(source_id,node_id,object_id,resource_id,direction,active,support_state,state_json,summary_json,boot_id,agent_generation,agent_session_id,updated_at) SELECT printf('00000000-0000-4000-8000-%012d',n),s.node_id,'00000000-0000-4000-8000-999999999999',s.resource_id,s.direction,0,'capacity_limited',s.state_json,json_set(s.summary_json,'$.source_id',printf('00000000-0000-4000-8000-%012d',n),'$.active',json('false'),'$.support_state','capacity_limited'),s.boot_id,s.agent_generation,s.agent_session_id,s.updated_at FROM numbers CROSS JOIN (SELECT * FROM detection_sources WHERE direction='read' LIMIT 1) s;")
        .execute(&h.state.db).await.unwrap();
    let now = Utc::now().timestamp_millis();
    let mut tx = h.state.db.begin().await.unwrap();
    let error = orchard_server::detection_store::source_summaries(&mut tx, now)
        .await
        .unwrap_err();
    assert_eq!(error.status.as_u16(), 503);
    assert_eq!(error.code, "source_limit");
    let narrowed = orchard_server::detection_store::source_summaries_filtered(
        &mut tx,
        now,
        None,
        Some(&object),
    )
    .await
    .unwrap();
    assert_eq!(narrowed.len(), 2);
    let warnings = orchard_server::detection_store::current_warning_summaries(&mut tx, now)
        .await
        .unwrap();
    assert!(warnings.is_empty());
}
