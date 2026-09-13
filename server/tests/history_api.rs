use axum::{
    Router,
    body::{Body, to_bytes},
    http::Request,
};
use cider_server::{
    api, read_api,
    store::{AppState, fingerprint, timestamp},
};
use serde_json::{Value, json};
use tower::ServiceExt;
async fn get(app: &Router, path: &str, token: &str) -> (u16, Value) {
    let r = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(path)
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let code = r.status().as_u16();
    let b = to_bytes(r.into_body(), 1_000_000).await.unwrap();
    (code, serde_json::from_slice(&b).unwrap_or(Value::Null))
}
#[tokio::test]
async fn history_and_forecast_routes_require_viewer_scope_and_strict_parameters() {
    let dir = tempfile::tempdir().unwrap();
    let s = AppState::open(&dir.path().join("api.db"), "admin")
        .await
        .unwrap();
    let now = chrono::Utc::now().timestamp_millis();
    let node = uuid::Uuid::new_v4().to_string();
    let object = uuid::Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO nodes(node_id,name,agent_json,credential_hash,enrolled_at,last_seen_at,boot_id,inventory_generation) VALUES (?,'n','{}',?,0,?,'boot',1)").bind(&node).bind(fingerprint(b"collector")).bind(now).execute(&s.db).await.unwrap();
    sqlx::query("INSERT INTO objects(object_id,node_id,local_id,kind,parents_json,properties_json,generation) VALUES (?,?,'pool','apfs_container','[]','{}',1)").bind(&object).bind(&node).execute(&s.db).await.unwrap();
    let app = api::router(s.clone()).merge(read_api::router(s.clone()));
    let hist = format!(
        "/api/v1/objects/{object}/history?metric=used_bytes&from={}&to={}&resolution=raw",
        timestamp(now - 3_600_000),
        timestamp(now - 1000)
    );
    let forecast = format!("/api/v1/objects/{object}/capacity-forecast");
    for path in [&hist, &forecast] {
        assert_eq!(get(&app, path, "wrong").await.0, 401, "{path}");
        assert_eq!(get(&app, path, "collector").await.0, 403, "{path}");
        let (code, body) = get(&app, path, &s.viewer_token).await;
        assert_eq!(code, 200, "{path}: {body}");
        assert_eq!(body["meta"]["api_version"], "1");
        assert_eq!(get(&app, path, "admin").await.0, 200);
    }
    let (_, h) = get(&app, &hist, &s.viewer_token).await;
    assert_eq!(h["data"]["series"], json!([]));
    assert_eq!(h["meta"]["selected_points"], 0);
    let (_, f) = get(&app, &forecast, &s.viewer_token).await;
    assert_eq!(f["data"]["state"], "unknown");
    assert_eq!(f["data"]["reasons"], json!(["insufficient_data"]));
    assert!(f["data"]["estimated_exhaustion_at"].is_null());
    for path in [
        format!("{hist}&metric=capacity_bytes"),
        format!("{hist}&limit=1"),
        format!("{forecast}?from=anything"),
        format!("/api/v1/objects/{object}/history?metric=used_bytes"),
    ] {
        assert_eq!(get(&app, &path, &s.viewer_token).await.0, 400, "{path}");
    }
    let missing = uuid::Uuid::new_v4().to_string();
    assert_eq!(
        get(&app, &forecast.replace(&object, &missing), &s.viewer_token)
            .await
            .0,
        404
    );
}
