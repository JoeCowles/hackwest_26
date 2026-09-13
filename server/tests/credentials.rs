use axum::{
    Router,
    body::{Body, to_bytes},
    http::Request,
};
use chrono::Utc;
use cider_server::{api, cider_api, read_api, store::AppState};
use serde_json::{Value, json};
use std::{fs, path::Path};
use tower::ServiceExt;
use uuid::Uuid;

const ADMIN: &str = "credential-restart-test-administrator";
const LEGACY_VIEWER: &str =
    "viewer_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn routes(state: &AppState) -> Router {
    api::router(state.clone())
        .merge(cider_api::router(state.clone()))
        .merge(read_api::router(state.clone()))
}

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
    let body = to_bytes(response.into_body(), 2 * 1024 * 1024)
        .await
        .unwrap();
    (status, serde_json::from_slice(&body).unwrap())
}

fn private_file(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }
}

#[tokio::test]
async fn copied_viewer_survives_restart_and_has_no_fixed_expiry() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("db.sqlite3");
    let first = AppState::open(&db, ADMIN).await.unwrap();
    let copied = first.viewer_token.clone();
    first.db.close().await;
    drop(first);
    let restarted = AppState::open(&db, ADMIN).await.unwrap();
    let app = routes(&restarted);
    let (status, body) = request(&app, "GET", "/api/v1/capabilities", &copied, Value::Null).await;
    assert_eq!(status, 200, "{body}");
    assert!(
        body["data"]
            .as_object()
            .unwrap()
            .contains_key("viewer_credential_expires_at")
    );
    assert_eq!(body["data"]["viewer_credential_expires_at"], Value::Null);
    assert_eq!(body["data"]["viewer_scope"], "telemetry:read");
    assert_eq!(body["data"]["poll_interval_seconds"], 3);
    assert_eq!(body["data"]["heartbeat_interval_seconds"], 5);
    assert_eq!(
        request(
            &app,
            "PUT",
            "/api/v1/notifications/settings",
            &copied,
            json!({})
        )
        .await
        .0,
        403
    );
    assert_eq!(
        request(&app, "GET", "/api/v1/cluster", ADMIN, Value::Null)
            .await
            .0,
        200
    );
    assert_eq!(
        request(&app, "GET", "/api/v1/cluster", "invalid", Value::Null)
            .await
            .0,
        401
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("viewer-token")).unwrap(),
        copied
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(dir.path().join("viewer-token"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}

#[tokio::test]
async fn legacy_viewer_is_reused_and_explicit_replacement_invalidates_old_copy() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("db.sqlite3");
    let credential = dir.path().join("viewer-token");
    private_file(&credential, LEGACY_VIEWER.as_bytes());
    let first = AppState::open(&db, ADMIN).await.unwrap();
    assert_eq!(
        request(
            &routes(&first),
            "GET",
            "/api/v1/cluster",
            LEGACY_VIEWER,
            Value::Null
        )
        .await
        .0,
        200
    );
    assert_eq!(fs::read_to_string(&credential).unwrap(), LEGACY_VIEWER);
    first.db.close().await;
    drop(first);
    let replacement = "viewer_abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";
    private_file(&credential, replacement.as_bytes());
    let restarted = AppState::open(&db, ADMIN).await.unwrap();
    let app = routes(&restarted);
    assert_eq!(
        request(&app, "GET", "/api/v1/cluster", LEGACY_VIEWER, Value::Null)
            .await
            .0,
        401
    );
    assert_eq!(
        request(&app, "GET", "/api/v1/cluster", replacement, Value::Null)
            .await
            .0,
        200
    );
}

#[tokio::test]
async fn malformed_viewer_file_fails_startup_without_replacement() {
    for bytes in [
        vec![],
        b"short".to_vec(),
        vec![b'a'; 64],
        b"viewer_unsupported.punctuation_with_padding".to_vec(),
        b"viewer_token_with_internal whitespace_and_padding".to_vec(),
        vec![b'a'; 4097],
        vec![0xff; 40],
        ADMIN.as_bytes().to_vec(),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("viewer-token");
        private_file(&path, &bytes);
        assert!(AppState::open(&dir.path().join("db"), ADMIN).await.is_err());
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
}

#[tokio::test]
async fn viewer_credential_cannot_grant_administrator_scope() {
    let dir = tempfile::tempdir().unwrap();
    private_file(&dir.path().join("viewer-token"), LEGACY_VIEWER.as_bytes());
    assert!(
        AppState::open(&dir.path().join("db"), LEGACY_VIEWER)
            .await
            .is_err()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn viewer_file_rejects_symlinks_and_nonprivate_permissions() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let dir = tempfile::tempdir().unwrap();
    let original = dir.path().join("original");
    let credential = dir.path().join("viewer-token");
    private_file(&original, LEGACY_VIEWER.as_bytes());
    symlink(&original, &credential).unwrap();
    assert!(AppState::open(&dir.path().join("db"), ADMIN).await.is_err());
    fs::remove_file(&credential).unwrap();
    private_file(&credential, LEGACY_VIEWER.as_bytes());
    fs::set_permissions(&credential, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(AppState::open(&dir.path().join("db"), ADMIN).await.is_err());
    assert_eq!(fs::read_to_string(&original).unwrap(), LEGACY_VIEWER);
}

fn heartbeat(node: &str, generation: &str, session: &str) -> Value {
    json!({
        "schema_version":"2.0","message_type":"heartbeat","node_id":node,
        "boot_id":"same-boot","agent_session_id":session,"agent_generation":generation,
        "sequence":"1","created_at":Utc::now().to_rfc3339(),"clock_id":format!("{session}/clock"),"monotonic_ns":"1000000000",
        "agent":{"version":"0.1.0","target":"aarch64-apple-darwin","os_version":"test","os_build":"test",
            "delivery_mode":"latest","heartbeat_interval_seconds":5,"quarantined_workers":0,
            "discarded_samples_total":"0","dropped_events_total":"0","payload_limited":false},
        "inventory":{"revision":"1","included":true},
        "resources":[{"resource_id":"host","resource_type":"host","revision":"1",
            "observed_at":Utc::now().to_rfc3339(),"identity_confidence":"host_local","attributes":{}}],
        "relationships":[],"collections":[],"collector_states":[],"events":[],"tombstones":[]
    })
}

#[tokio::test]
async fn enrolled_node_and_receipts_survive_server_and_client_restarts_and_revocation() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("db.sqlite3");
    let first = AppState::open(&db, ADMIN).await.unwrap();
    let token = first.local_enrollment_token().await.unwrap();
    let app = routes(&first);
    let (status, enrolled) = request(&app, "POST", "/api/v1/nodes/enroll", "",
        json!({"enrollment_token":token["enrollment_token"],"name":"Restart Mac","agent":{"version":"test"}})).await;
    assert_eq!(status, 201, "{enrolled}");
    let node = enrolled["data"]["node_id"].as_str().unwrap();
    let credential = enrolled["data"]["credential"].as_str().unwrap();
    let mut original = heartbeat(node, "1", "first-session");
    let path = "/api/v2/ciderd/heartbeat";
    let (status, receipt) = request(&app, "POST", path, credential, original.clone()).await;
    assert_eq!(status, 200, "{receipt}");
    first.db.close().await;
    drop(app);
    drop(first);
    let restarted = AppState::open(&db, ADMIN).await.unwrap();
    let app = routes(&restarted);
    assert_eq!(
        request(&app, "POST", path, credential, original.clone()).await,
        (200, receipt)
    );
    original["sequence"] = json!("2");
    original["inventory"]["included"] = json!(false);
    original["resources"] = json!([]);
    let (status, body) = request(&app, "POST", path, credential, original.clone()).await;
    assert_eq!(status, 200, "{body}");
    let mut next_session = heartbeat(node, "2", "restarted-session");
    let (status, body) = request(&app, "POST", path, credential, next_session.clone()).await;
    assert_eq!(status, 200, "{body}");
    original["sequence"] = json!("3");
    assert_eq!(
        request(&app, "POST", path, credential, original).await.0,
        409
    );
    assert_eq!(
        request(&app, "GET", "/api/v1/cluster", credential, Value::Null)
            .await
            .0,
        403
    );
    assert_eq!(
        request(
            &app,
            "POST",
            path,
            &restarted.viewer_token,
            next_session.clone()
        )
        .await
        .0,
        401
    );
    let nodes: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM nodes")
        .fetch_one(&restarted.db)
        .await
        .unwrap();
    assert_eq!(nodes, 1, "Restart must reuse the enrolled node");
    let (status, body) = request(
        &app,
        "DELETE",
        &format!("/api/v1/nodes/{node}/credential"),
        ADMIN,
        Value::Null,
    )
    .await;
    assert_eq!(status, 200, "{body}");
    restarted.db.close().await;
    drop(app);
    drop(restarted);
    let revoked = AppState::open(&db, ADMIN).await.unwrap();
    next_session["sequence"] = json!("2");
    assert_eq!(
        request(&routes(&revoked), "POST", path, credential, next_session)
            .await
            .0,
        401
    );
}
