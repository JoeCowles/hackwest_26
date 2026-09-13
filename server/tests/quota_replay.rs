//! Replays safe real rquotad observations with synthetic enrollment/acquisition metadata.
use axum::{
    Router,
    body::{Body, to_bytes},
    http::Request,
};
use chrono::Utc;
use orchard_server::{api, cider_api, cider_wire, read_api, store::AppState};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;
async fn request(app: &Router, method: &str, path: &str, token: &str, body: Value) -> (u16, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("authorization", format!("Bearer {token}"))
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
    let bytes = to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}
#[tokio::test]
async fn recorded_real_quotas_survive_authenticated_ingestion_and_read_projection() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../crates/ciderd/tests/fixtures/rquota-live.json"
    ))
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::open(&dir.path().join("quota.sqlite"), "test-admin")
        .await
        .unwrap();
    let app = api::router(state.clone())
        .merge(cider_api::router(state.clone()))
        .merge(read_api::router(state.clone()));
    for (index, row) in fixture["observations"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
    {
        let enrollment = state.local_enrollment_token().await.unwrap();
        let(code,enrolled)=request(&app,"POST","/api/v1/nodes/enroll","test-admin",json!({"enrollment_token":enrollment["enrollment_token"],"name":format!("Recorded quota client {} (UID {})",index+1,row["process_uid"]),"agent":{"version":"0.1.0"}})).await;
        assert_eq!(code, 201, "{enrolled}");
        let node = enrolled["data"]["node_id"].as_str().unwrap();
        let credential = enrolled["data"]["credential"].as_str().unwrap();
        let at = Utc::now().to_rfc3339();
        let mut metrics = vec![];
        for name in [
            "status",
            "active",
            "used_bytes",
            "block_size_bytes",
            "block_soft_limit_bytes",
            "block_hard_limit_bytes",
            "used_inodes",
            "inode_soft_limit",
            "inode_hard_limit",
            "block_grace_seconds_raw",
            "inode_grace_seconds_raw",
        ] {
            let full = format!("storage.nfs.quota.{name}");
            let def = &cider_wire::catalog()[&full];
            if let Some(value) = row.get(name) {
                metrics.push(json!({"name":full,"kind":def.kind,"unit":def.unit,"availability":"available","attributes":{},"freshness":"live","value_type":def.value_type,"value":value}));
            }
        }
        let status = match row["status"].as_str().unwrap() {
            "available" => "ok",
            "no_quota" => "partial",
            s => s,
        };
        let heartbeat = json!({"schema_version":"2.0","message_type":"heartbeat","node_id":node,"boot_id":"fixture-boot","agent_session_id":"fixture-session","agent_generation":"1","sequence":"1","created_at":at,"clock_id":"fixture-clock","monotonic_ns":"2000000000",
   "agent":{"version":"0.1.0","target":"fixture-rpc-replay","os_version":"fixture","os_build":"fixture","delivery_mode":"latest","heartbeat_interval_seconds":5,"quarantined_workers":0,"discarded_samples_total":"0","dropped_events_total":"0","payload_limited":false},"inventory":{"revision":"1","included":true},
   "resources":[{"resource_id":"host","resource_type":"host","revision":"1","observed_at":at,"identity_confidence":"host_local","attributes":{"diagnostics_configuration":{"nfs_quotas_enabled":true,"configured_quota_target_count":1}}},
    {"resource_id":"quota","resource_type":"nfs_user_quota","revision":"1","observed_at":at,"identity_confidence":"host_local","attributes":{"server":row["server"],"export_path":row["export_path"],"uid":row["uid"].to_string(),"nfs_source":format!("{}:{}",row["server"].as_str().unwrap(),row["export_path"].as_str().unwrap()),"protocol":"rquota-v1-udp","source":"nfs.rquota","query_identity_uid":row["process_uid"].to_string(),"query_identity_gid":row["process_gid"].to_string(),"query_groups_truncated":false}}],"relationships":[],
   "collections":[{"collection_id":"quota-c1","resource_id":"quota","collector":"nfs.rquota","adapter_version":"0.1.0","source_version":"recorded-rquotad-response","started_at":at,"finished_at":at,"clock_id":"fixture-clock","started_monotonic_ns":"1000000000","finished_monotonic_ns":"1000000000","status":status,"metrics":metrics}],
   "collector_states":[{"collector":"nfs.rquota","resource_id":"quota","phase":"idle","poll_interval_seconds":60,"stale_after_seconds":180,"last_attempt_id":"quota-c1"}],"events":[],"tombstones":[]});
        serde_json::from_value::<cider_wire::Heartbeat>(heartbeat.clone())
            .unwrap()
            .validate()
            .unwrap();
        let (code, reply) = request(
            &app,
            "POST",
            "/api/v2/ciderd/heartbeat",
            credential,
            heartbeat.clone(),
        )
        .await;
        assert_eq!(code, 200, "{reply}");
        assert_eq!(
            request(
                &app,
                "POST",
                "/api/v2/ciderd/heartbeat",
                credential,
                heartbeat
            )
            .await
            .0,
            200
        );
    }
    let (code, response) = request(
        &app,
        "GET",
        "/api/v1/quotas",
        &state.viewer_token,
        Value::Null,
    )
    .await;
    assert_eq!(code, 200, "{response}");
    let rows = response["data"].as_array().unwrap();
    assert_eq!(rows.len(), 5, "{response}");
    assert_eq!(
        rows.iter().filter(|r| r["state"] == "available").count(),
        3,
        "{response}"
    );
    assert_eq!(
        rows.iter()
            .filter(|r| r["state"] == "permission_denied")
            .count(),
        1
    );
    assert_eq!(rows.iter().filter(|r| r["state"] == "no_quota").count(), 1);
    let unlimited = rows.iter().find(|r| r["uid"] == "503").unwrap();
    assert_eq!(unlimited["limits"]["block_hard"]["state"], "unlimited");
    let other = rows
        .iter()
        .find(|r| r["uid"] == "502" && r["state"] == "available")
        .unwrap();
    assert_eq!(other["metrics"]["used_bytes"]["value"], "132096");
    assert!(
        rows.iter()
            .filter(|r| r["state"] != "available")
            .all(|r| r["limits"]["block_hard"]["state"] == "unknown")
    );
    let (code, cluster) = request(
        &app,
        "GET",
        "/api/v1/cluster",
        &state.viewer_token,
        Value::Null,
    )
    .await;
    assert_eq!(code, 200);
    assert_eq!(
        cluster["data"]["capacity"]["local"]["capacity_bytes"]["state"],
        "unknown"
    );
    let (code, filesystems) = request(
        &app,
        "GET",
        "/api/v1/filesystems",
        &state.viewer_token,
        Value::Null,
    )
    .await;
    assert_eq!(code, 200);
    assert_eq!(filesystems["data"], json!([]));
    if let Ok(path) = std::env::var("ORCHARD_QUOTA_CAPTURE") {
        let mut capture = response;
        capture["fixture_note"] = json!(
            "Real isolated rquotad quota values replayed through authenticated ingestion with synthetic node IDs and fresh replay acquisition dates; not a live cluster capture."
        );
        std::fs::write(path, serde_json::to_string_pretty(&capture).unwrap()).unwrap();
    }
}
