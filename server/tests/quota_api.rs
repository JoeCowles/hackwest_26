use axum::{
    Router,
    body::{Body, to_bytes},
    http::Request,
};
use cider_server::{api, read_api, store::AppState};
use serde_json::Value;
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
    let status = r.status().as_u16();
    let bytes = to_bytes(r.into_body(), 1024 * 1024).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}
#[tokio::test]
async fn quota_reads_require_auth_and_reject_invalid_subject_filters() {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::open(&dir.path().join("test.sqlite"), "admin")
        .await
        .unwrap();
    let app = api::router(state.clone()).merge(read_api::router(state.clone()));
    assert_eq!(get(&app, "/api/v1/quotas", "wrong").await.0, 401);
    let (s, b) = get(&app, "/api/v1/quotas", &state.viewer_token).await;
    assert_eq!(s, 200, "{b}");
    assert_eq!(b["data"], serde_json::json!([]));
    assert_eq!(b["meta"]["coverage"]["state"], "unknown");
    for q in [
        "uid=-1",
        "uid=01",
        "uid=2147483648",
        "uid=501&uid=502",
        "limit=501",
    ] {
        assert_eq!(
            get(&app, &format!("/api/v1/quotas?{q}"), &state.viewer_token)
                .await
                .0,
            400,
            "{q}"
        );
    }
}
