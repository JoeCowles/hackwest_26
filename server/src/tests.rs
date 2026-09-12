use crate::{api, store::AppState};
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use chrono::{Duration, Utc};
use serde_json::{Value, json};
use sqlx::Row;
use tower::ServiceExt;
use uuid::Uuid;

const ADMIN: &str = "test_admin_credential_at_least_32_bytes_long";

struct Fixture {
    state: AppState,
    _directory: tempfile::TempDir,
}
impl Fixture {
    async fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let state = AppState::open(&directory.path().join("test.sqlite3"), ADMIN)
            .await
            .unwrap();
        Self {
            state,
            _directory: directory,
        }
    }
    async fn request(
        &self,
        method: &str,
        path: &str,
        credential: Option<&str>,
        value: Value,
    ) -> (StatusCode, Value) {
        self.request_id(method, path, credential, value, &Uuid::new_v4().to_string())
            .await
    }
    async fn request_id(
        &self,
        method: &str,
        path: &str,
        credential: Option<&str>,
        value: Value,
        id: &str,
    ) -> (StatusCode, Value) {
        let mut builder = Request::builder()
            .method(method)
            .uri(path)
            .header("content-type", "application/json")
            .header("x-request-id", id)
            .header("x-request-timestamp", Utc::now().to_rfc3339());
        if let Some(token) = credential {
            builder = builder.header("authorization", format!("Bearer {token}"));
        }
        let response = api::router(self.state.clone())
            .oneshot(builder.body(Body::from(value.to_string())).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let value =
            serde_json::from_slice(&to_bytes(response.into_body(), 2_000_000).await.unwrap())
                .unwrap();
        (status, value)
    }
    async fn token(&self) -> String {
        let (status, value) = self
            .request("POST", "/api/v1/enrollment-tokens", Some(ADMIN), json!({}))
            .await;
        assert_eq!(status, StatusCode::CREATED, "{value}");
        value["data"]["enrollment_token"]
            .as_str()
            .unwrap()
            .to_owned()
    }
    async fn node(&self) -> (String, String) {
        let token = self.token().await;
        let (status, value) = self
            .request(
                "POST",
                "/api/v1/nodes/enroll",
                None,
                json!({"enrollment_token":token,"name":"test-node","agent":{"version":"0.1.0"}}),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{value}");
        (
            value["data"]["node_id"].as_str().unwrap().to_owned(),
            value["data"]["credential"].as_str().unwrap().to_owned(),
        )
    }
    async fn device(&self, node: &str, token: &str, generation: i64) -> String {
        let (status,value)=self.request("PUT",&format!("/api/v1/nodes/{node}/inventory"),Some(token),json!({"generation":generation,"objects":[{"local_id":"disk-stable-01","kind":"device"}]})).await;
        assert_eq!(status, StatusCode::OK, "{value}");
        value["data"]["objects"][1]["object_id"]
            .as_str()
            .unwrap()
            .to_owned()
    }
}

fn batch(
    node: &str,
    object: &str,
    sequence: u64,
    boot: &str,
    generation: i64,
    at: chrono::DateTime<Utc>,
    value: Value,
) -> Value {
    json!({"schema_version":"1.0","node_id":node,"boot_id":boot,"sequence":sequence,
        "observed_at":at.to_rfc3339(),"sent_at":Utc::now().to_rfc3339(),"agent":{"version":"0.1.0"},"inventory_generation":generation,
        "samples":[{"object_id":object,"name":"device_read_bytes_total","kind":"counter","value":value,"unit":"bytes","state":"ok","source":"test","observed_at":at.to_rfc3339()}],"events":[]})
}

#[tokio::test]
async fn enrollment_is_single_use_and_atomic_under_concurrency() {
    let f = Fixture::new().await;
    let token = f.token().await;
    let body = json!({"enrollment_token":token,"name":"test","agent":{"version":"0.1.0"}});
    let (a, b) = tokio::join!(
        f.request("POST", "/api/v1/nodes/enroll", None, body.clone()),
        f.request("POST", "/api/v1/nodes/enroll", None, body)
    );
    assert!([a.0, b.0].contains(&StatusCode::CREATED));
    assert!([a.0, b.0].contains(&StatusCode::UNAUTHORIZED));
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM nodes")
        .fetch_one(&f.state.db)
        .await
        .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn credential_scopes_and_revocation_are_enforced() {
    let f = Fixture::new().await;
    let (a, ta) = f.node().await;
    let (b, tb) = f.node().await;
    let heartbeat = json!({"boot_id":"boot-a","agent":{"version":"1"}});
    assert_eq!(
        f.request(
            "POST",
            &format!("/api/v1/nodes/{b}/heartbeat"),
            Some(&ta),
            heartbeat.clone()
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        f.request("POST", "/api/v1/enrollment-tokens", Some(&ta), json!({}))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        f.request(
            "DELETE",
            &format!("/api/v1/nodes/{a}/credential"),
            Some(ADMIN),
            json!({})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        f.request(
            "POST",
            &format!("/api/v1/nodes/{a}/heartbeat"),
            Some(&ta),
            heartbeat.clone()
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        f.request(
            "POST",
            &format!("/api/v1/nodes/{b}/heartbeat"),
            Some(&tb),
            heartbeat
        )
        .await
        .0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn replay_rejected_but_same_batch_with_fresh_request_id_is_idempotent() {
    let f = Fixture::new().await;
    let (node, token) = f.node().await;
    let object = f.device(&node, &token, 1).await;
    let path = format!("/api/v1/nodes/{node}/telemetry");
    let id = Uuid::new_v4().to_string();
    let original = batch(&node, &object, 1, "boot-a", 1, Utc::now(), json!(100));
    let a = f
        .request_id("POST", &path, Some(&token), original.clone(), &id)
        .await;
    assert_eq!(a.0, StatusCode::ACCEPTED, "{}", a.1);
    let replay = f
        .request_id("POST", &path, Some(&token), original.clone(), &id)
        .await;
    assert_eq!(replay.0, StatusCode::CONFLICT);
    assert_eq!(replay.1["error"]["code"], "request_replayed");
    let duplicate = f
        .request("POST", &path, Some(&token), original.clone())
        .await;
    assert_eq!(duplicate.0, StatusCode::ACCEPTED);
    assert_eq!(duplicate.1["data"]["duplicate"], true);
    let mut changed = original;
    changed["samples"][0]["value"] = json!(200);
    assert_eq!(
        f.request("POST", &path, Some(&token), changed).await.0,
        StatusCode::CONFLICT
    );
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM metric_samples")
        .fetch_one(&f.state.db)
        .await
        .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn inventory_rejects_cycles_and_keeps_stable_object_ids() {
    let f = Fixture::new().await;
    let (node, token) = f.node().await;
    let cyclic = json!({"generation":1,"objects":[{"local_id":"a","kind":"device","parents":["b"]},{"local_id":"b","kind":"partition","parents":["a"]}]});
    assert_eq!(
        f.request(
            "PUT",
            &format!("/api/v1/nodes/{node}/inventory"),
            Some(&token),
            cyclic
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let first = f.device(&node, &token, 1).await;
    let second = f.device(&node, &token, 2).await;
    assert_eq!(first, second);
    assert_eq!(
        f.request(
            "POST",
            &format!("/api/v1/nodes/{node}/telemetry"),
            Some(&token),
            batch(&node, &first, 1, "boot-a", 1, Utc::now(), json!(1))
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
}

#[tokio::test]
async fn exact_large_counter_deltas_and_generation_boundaries() {
    let f = Fixture::new().await;
    let (node, token) = f.node().await;
    let object = f.device(&node, &token, 1).await;
    let path = format!("/api/v1/nodes/{node}/telemetry");
    let at = Utc::now() - Duration::seconds(20);
    for (seq, boot, offset, value) in [
        (1, "boot-a", 0, u64::MAX - 100),
        (2, "boot-a", 5, u64::MAX - 50),
        (3, "boot-a", 10, 10),
        (4, "boot-b", 15, 100),
    ] {
        let response = f
            .request(
                "POST",
                &path,
                Some(&token),
                batch(
                    &node,
                    &object,
                    seq,
                    boot,
                    1,
                    at + Duration::seconds(offset),
                    json!(value.to_string()),
                ),
            )
            .await;
        assert_eq!(response.0, StatusCode::ACCEPTED, "{}", response.1);
    }
    let rows = sqlx::query(
        "SELECT rate_per_second,derivation_state FROM metric_samples ORDER BY observed_at",
    )
    .fetch_all(&f.state.db)
    .await
    .unwrap();
    assert_eq!(rows[0].get::<Option<f64>, _>("rate_per_second"), None);
    assert_eq!(rows[1].get::<Option<f64>, _>("rate_per_second"), Some(10.0));
    assert_eq!(rows[2].get::<String, _>("derivation_state"), "reset");
    assert_eq!(rows[3].get::<Option<f64>, _>("rate_per_second"), None);
}

#[tokio::test]
async fn unknown_samples_remain_null_and_atomic_validation_preserves_database() {
    let f = Fixture::new().await;
    let (node, token) = f.node().await;
    let object = f.device(&node, &token, 1).await;
    let path = format!("/api/v1/nodes/{node}/telemetry");
    let mut value = batch(&node, &object, 1, "boot-a", 1, Utc::now(), Value::Null);
    value["samples"][0]["state"] = json!("unsupported");
    assert_eq!(
        f.request("POST", &path, Some(&token), value).await.0,
        StatusCode::ACCEPTED
    );
    let row = sqlx::query("SELECT numeric_value,rate_per_second,value_json FROM metric_samples")
        .fetch_one(&f.state.db)
        .await
        .unwrap();
    assert_eq!(row.get::<String, _>("value_json"), "null");
    assert!(row.get::<Option<f64>, _>("numeric_value").is_none());
    assert!(row.get::<Option<f64>, _>("rate_per_second").is_none());
    let mut invalid = batch(&node, &object, 2, "boot-a", 1, Utc::now(), json!(20));
    invalid["samples"][0]["unit"] = json!("wrong");
    assert_eq!(
        f.request("POST", &path, Some(&token), invalid).await.0,
        StatusCode::BAD_REQUEST
    );
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM batches")
        .fetch_one(&f.state.db)
        .await
        .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn privacy_validation_and_body_limits() {
    let f = Fixture::new().await;
    let (node, token) = f.node().await;
    let body = json!({"generation":1,"objects":[{"local_id":"a","kind":"device","properties":{"nested":{"file_contents":"private"}}}]});
    assert_eq!(
        f.request(
            "PUT",
            &format!("/api/v1/nodes/{node}/inventory"),
            Some(&token),
            body
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        f.request(
            "POST",
            "/api/v1/enrollment-tokens",
            Some(ADMIN),
            json!({"extra":"x".repeat(crate::MAX_BODY_BYTES)})
        )
        .await
        .0,
        StatusCode::PAYLOAD_TOO_LARGE
    );
}

#[tokio::test]
async fn heartbeat_cadence_goodbye_and_recovery() {
    let f = Fixture::new().await;
    let (node, token) = f.node().await;
    let payload = json!({"boot_id":"boot-a","agent":{"version":"1"}});
    let path = format!("/api/v1/nodes/{node}");
    let beat = f
        .request(
            "POST",
            &format!("{path}/heartbeat"),
            Some(&token),
            payload.clone(),
        )
        .await;
    assert_eq!(beat.1["data"]["heartbeat_interval_seconds"], 5);
    assert_eq!(
        f.request(
            "POST",
            &format!("{path}/goodbye"),
            Some(&token),
            json!({"boot_id":"old-boot"})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        f.request(
            "POST",
            &format!("{path}/goodbye"),
            Some(&token),
            json!({"boot_id":"boot-a"})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        f.request("POST", &format!("{path}/heartbeat"), Some(&token), payload)
            .await
            .0,
        StatusCode::OK
    );
    let goodbye: Option<i64> = sqlx::query_scalar("SELECT goodbye_at FROM nodes WHERE node_id=?")
        .bind(&node)
        .fetch_one(&f.state.db)
        .await
        .unwrap();
    assert_eq!(goodbye, None);
}

#[tokio::test]
async fn rollups_preserve_states_and_use_rates_instead_of_counter_averages() {
    let f = Fixture::new().await;
    let (node, token) = f.node().await;
    let object = f.device(&node, &token, 1).await;
    let now = Utc::now();
    let boundary = (now.timestamp_millis() / 300_000 - 1) * 300_000;
    for (seq, offset, value) in [(1, 1000, 100), (2, 6000, 150), (3, 11000, 200)] {
        let at = chrono::DateTime::from_timestamp_millis(boundary + offset).unwrap();
        let response = f
            .request(
                "POST",
                &format!("/api/v1/nodes/{node}/telemetry"),
                Some(&token),
                batch(&node, &object, seq, "boot-a", 1, at, json!(value)),
            )
            .await;
        assert_eq!(response.0, StatusCode::ACCEPTED, "{}", response.1);
    }
    crate::workers::maintain(&f.state, now.timestamp_millis())
        .await
        .unwrap();
    let row = sqlx::query("SELECT * FROM metric_rollups WHERE resolution_seconds=300")
        .fetch_one(&f.state.db)
        .await
        .unwrap();
    assert_eq!(row.get::<f64, _>("mean"), 10.0);
    assert_eq!(row.get::<i64, _>("sample_count"), 3);
    assert_eq!(row.get::<i64, _>("valid_sample_count"), 2);
}

#[tokio::test]
async fn disk_persistence_survives_reopen_and_planned_routes_stay_unavailable() {
    let f = Fixture::new().await;
    let (node, token) = f.node().await;
    f.state.db.close().await;
    let reopened = AppState::open(&f._directory.path().join("test.sqlite3"), ADMIN)
        .await
        .unwrap();
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM nodes WHERE node_id=?")
        .bind(node)
        .fetch_one(&reopened.db)
        .await
        .unwrap();
    assert_eq!(count, 1);
    let response = api::router(reopened)
        .oneshot(
            Request::builder()
                .uri("/api/v1/nodes")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn late_samples_recompute_successor_rate_and_preserve_latest_observation() {
    let f = Fixture::new().await;
    let (node, token) = f.node().await;
    let object = f.device(&node, &token, 1).await;
    let at = Utc::now() - Duration::seconds(10);
    let path = format!("/api/v1/nodes/{node}/telemetry");
    for (seq, offset, value) in [(1, 5, 200), (2, 0, 100)] {
        let result = f
            .request(
                "POST",
                &path,
                Some(&token),
                batch(
                    &node,
                    &object,
                    seq,
                    "boot-a",
                    1,
                    at + Duration::seconds(offset),
                    json!(value),
                ),
            )
            .await;
        assert_eq!(result.0, StatusCode::ACCEPTED, "{}", result.1);
    }
    let latest: String =
        sqlx::query_scalar("SELECT sample_json FROM latest_samples WHERE object_id=?")
            .bind(&object)
            .fetch_one(&f.state.db)
            .await
            .unwrap();
    let latest: Value = serde_json::from_str(&latest).unwrap();
    assert_eq!(latest["sample"]["value"], 200);
    assert_eq!(latest["derived_rate_per_second"], 20.0);
}

#[test]
fn remote_plaintext_binding_is_rejected() {
    let config = crate::config::Config {
        bind: "0.0.0.0:8787".parse().unwrap(),
        data_dir: None,
        headless: true,
        tls_cert: None,
        tls_key: None,
    };
    assert!(config.prepare().is_err());
}
