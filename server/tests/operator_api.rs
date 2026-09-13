use axum::{Router, body::{Body, to_bytes}, http::Request};
use cider_server::{api, read_api, store::AppState};
use serde_json::{Value,json};
use tower::ServiceExt;

async fn get(app: &Router, path: &str, token: &str) -> (u16, Value) {
    let response = app.clone().oneshot(Request::builder().uri(path)
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty()).unwrap()).await.unwrap();
    let status = response.status().as_u16();
    let body = to_bytes(response.into_body(), 4 * 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&body).unwrap())
}

#[tokio::test]
async fn reachable_node_does_not_imply_complete_healthy_storage() {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::open(&dir.path().join("db"), "admin").await.unwrap();
    let now = chrono::Utc::now().timestamp_millis();
    sqlx::query("INSERT INTO nodes(node_id,name,agent_json,credential_hash,enrolled_at,last_seen_at) VALUES (?,?,?,?,?,?)")
        .bind("22222222-2222-4222-8222-222222222222").bind("Test Mac").bind("{\"version\":\"test\"}")
        .bind("hash").bind(now).bind(now).execute(&state.db).await.unwrap();
    let app = api::router(state.clone()).merge(read_api::router(state.clone()));
    let (status, body) = get(&app, "/api/v1/nodes/22222222-2222-4222-8222-222222222222", &state.viewer_token).await;
    assert_eq!(status, 200);
    assert_eq!(body["data"]["availability"], "online");
    assert_eq!(body["data"]["health"]["overall"], "unknown");
    assert_eq!(body["data"]["health"]["assessment_state"], "incomplete");
    assert_eq!(body["data"]["health"]["dimensions"]["availability"]["status"], "healthy");
}

#[tokio::test]
async fn operator_reads_require_a_viewer_and_preserve_empty_states() {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::open(&dir.path().join("db"), "admin").await.unwrap();
    let app = api::router(state.clone()).merge(read_api::router(state.clone()));
    for path in ["/api/v1/attention", "/api/v1/attention/summary", "/api/v1/notifications/settings", "/api/v1/quotas", "/api/v1/diagnostics"] {
        assert_eq!(get(&app, path, "bad").await.0, 401, "{path}");
        let (status, body) = get(&app, path, &state.viewer_token).await;
        assert_eq!(status, 200, "{path}: {body}");
        assert!(body["meta"]["server_time"].is_string());
    }
}

async fn mutate(app:&Router,method:&str,path:&str,token:&str,body:Value,id:&str)->(u16,Value) {
    let response=app.clone().oneshot(Request::builder().method(method).uri(path)
        .header("authorization",format!("Bearer {token}"))
        .header("content-type","application/json").header("x-request-id",id)
        .header("x-request-timestamp",chrono::Utc::now().to_rfc3339())
        .body(Body::from(body.to_string())).unwrap()).await.unwrap();
    let status=response.status().as_u16();
    let bytes=to_bytes(response.into_body(),1024*1024).await.unwrap();
    (status,serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn settings_are_admin_only_revision_checked_masked_and_replay_protected() {
    let dir=tempfile::tempdir().unwrap();
    let state=AppState::open(&dir.path().join("db"),"admin").await.unwrap();
    let app=api::router(state.clone()).merge(read_api::router(state.clone()));
    let body=json!({"expected_revision":"initial","enabled":true,"sender":"+15555550101","recipient":"+15555550202"});
    let path="/api/v1/notifications/settings";
    for (token,expected) in [(&state.viewer_token[..],403),("wrong",401)] {
        assert_eq!(mutate(&app,"PUT",path,token,body.clone(),&uuid::Uuid::new_v4().to_string()).await.0,expected);
    }
    let id=uuid::Uuid::new_v4().to_string();
    let (code,saved)=mutate(&app,"PUT",path,"admin",body.clone(),&id).await;
    assert_eq!(code,200,"{saved}");
    assert_eq!(saved["data"]["recipient"],"***0202");
    assert_eq!(saved["data"]["enabled"],true);
    assert!(!saved.to_string().contains("+1555555"));
    let (code,replayed)=mutate(&app,"PUT",path,"admin",body.clone(),&id).await;
    assert_eq!(code,409);assert_eq!(replayed["error"]["code"],"request_replayed");
    assert_eq!(mutate(&app,"PUT",path,"admin",body,&uuid::Uuid::new_v4().to_string()).await.0,409);
    let invalid=json!({"expected_revision":saved["data"]["revision"],"enabled":true,"recipient":"5551234"});
    assert_eq!(mutate(&app,"PUT",path,"admin",invalid,&uuid::Uuid::new_v4().to_string()).await.0,400);
}

#[tokio::test]
async fn acknowledgement_needs_admin_and_matching_episode_revision() {
    let dir=tempfile::tempdir().unwrap();let state=AppState::open(&dir.path().join("db"),"admin").await.unwrap();
    let now=chrono::Utc::now().timestamp_millis();
    let mut tx=state.db.begin().await.unwrap();
    let condition=cider_server::attention::Condition{key:"test-source".into(),kind:"capacity".into(),node_id:uuid::Uuid::new_v4().to_string(),object_id:None,status:"open".into(),severity:"warning".into(),observation_state:"ok".into(),summary:"Observed usage above threshold".into(),evidence:json!({"used_ratio":0.95})};
    let episode=cider_server::attention::observe_condition(&mut tx,&condition,now).await.unwrap().unwrap();tx.commit().await.unwrap();
    let app=api::router(state.clone()).merge(read_api::router(state.clone()));
    let path=format!("/api/v1/attention/{}/acknowledgement",episode["id"].as_str().unwrap());
    let body=json!({"expected_revision":episode["revision"],"note":"Replacement ordered"});
    assert_eq!(mutate(&app,"POST",&path,&state.viewer_token,body.clone(),&uuid::Uuid::new_v4().to_string()).await.0,403);
    let (code,ack)=mutate(&app,"POST",&path,"admin",body.clone(),&uuid::Uuid::new_v4().to_string()).await;
    assert_eq!(code,200,"{ack}");assert_eq!(ack["data"]["status"],"open");
    assert_eq!(ack["data"]["acknowledgement"]["note"],"Replacement ordered");
    assert_eq!(mutate(&app,"POST",&path,"admin",body,&uuid::Uuid::new_v4().to_string()).await.0,409);
}
