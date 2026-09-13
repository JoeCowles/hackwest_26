use axum::{
    Router,
    body::{Body, to_bytes},
    http::Request,
};
use orchard_server::{api, read_api, store::AppState};
use serde_json::{Value, json};
use tower::ServiceExt;

async fn get(app: &Router, path: &str, token: &str) -> (u16, Value) {
    let response = app
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
    let status = response.status().as_u16();
    let bytes = to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn reliability_read_contract_is_authenticated_and_unknown_without_sources() {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::open(&dir.path().join("test.sqlite"), "admin")
        .await
        .unwrap();
    let app = api::router(state.clone()).merge(read_api::router(state.clone()));
    for path in [
        "/api/v1/reliability/sources",
        "/api/v1/reliability/findings",
    ] {
        let (status, body) = get(&app, path, &state.viewer_token).await;
        assert_eq!(status, 200, "{path}: {body}");
        assert_eq!(body["data"], json!([]));
        assert_eq!(body["meta"]["api_version"], "1");
        assert_eq!(get(&app, path, "wrong").await.0, 401);
    }
    let (_, body) = get(&app, "/api/v1/reliability/sources", &state.viewer_token).await;
    assert_eq!(body["meta"]["coverage"]["known_sources"], 0);
    assert_eq!(body["meta"]["coverage"]["assessment"], "unknown");
    assert!(body["meta"]["policy"].is_object());
    for query in [
        "status=quiet",
        "status=open&status=all",
        "from=nope",
        "node_id=wrong",
        "limit=501",
    ] {
        assert_eq!(
            get(
                &app,
                &format!("/api/v1/reliability/findings?{query}"),
                &state.viewer_token
            )
            .await
            .0,
            400,
            "{query}"
        );
    }
    assert_eq!(
        get(
            &app,
            "/api/v1/reliability/sources?status=open",
            &state.viewer_token
        )
        .await
        .0,
        400
    );
}

#[tokio::test]
async fn exact_findings_pages_are_frozen_filtered_and_credential_bound() {
    use orchard_server::reliability::{SourceIdentity, SourceState};
    use uuid::Uuid;
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::open(&dir.path().join("test.sqlite"), "admin")
        .await
        .unwrap();
    let app = api::router(state.clone()).merge(read_api::router(state.clone()));
    let node = Uuid::new_v4().to_string();
    let object = Uuid::new_v4().to_string();
    let source = Uuid::new_v4().to_string();
    let now = chrono::Utc::now().timestamp_millis();
    sqlx::query("INSERT INTO nodes(node_id,name,agent_json,credential_hash,enrolled_at,last_seen_at) VALUES (?,'test','{}',?,0,?)").bind(&node).bind(orchard_server::store::fingerprint(b"node-credential")).bind(now).execute(&state.db).await.unwrap();
    let model = SourceState::new(SourceIdentity {
        source_id: source.clone(),
        node_id: node.clone(),
        object_id: object.clone(),
        resource_id: "disk".into(),
        collector: "smartctl".into(),
        scope: "device".into(),
    });
    sqlx::query("INSERT INTO reliability_sources(source_id,node_id,object_id,resource_id,collector,active,support_state,state_json,summary_json,boot_id,agent_generation,agent_session_id,updated_at) VALUES (?,?,?,'disk','smartctl',1,'supported',?,?,'boot','1','session',?)")
        .bind(&source).bind(&node).bind(&object).bind(serde_json::to_string(&model).unwrap()).bind(model.summary(now,true).to_string()).bind(now).execute(&state.db).await.unwrap();
    let mut original = Vec::new();
    for (index, status) in ["open", "resolved"].iter().enumerate() {
        let id = Uuid::new_v4().to_string();
        let at = now - ((2 - index) as i64) * 1000;
        let row = json!({"finding_id":id,"source_id":source,"node_id":node,"object_id":object,"status":status,"first_seen_at":orchard_server::store::timestamp(at),"evidence":{"counter_start":"1844674407370955161600","counter_end":"1844674407370955161601","delta":"1"}});
        sqlx::query("INSERT INTO reliability_findings(finding_id,source_id,node_id,object_id,status,first_seen_at,updated_at,finding_json) VALUES (?,?,?,?,?,?,?,?)").bind(&id).bind(&source).bind(&node).bind(&object).bind(status).bind(at).bind(now).bind(row.to_string()).execute(&state.db).await.unwrap();
        original.push(row);
    }
    let (_, open) = get(&app, "/api/v1/reliability/findings", &state.viewer_token).await;
    assert_eq!(open["data"], json!([original[0]]));
    let path = "/api/v1/reliability/findings?status=all&limit=1";
    let (status, first) = get(&app, path, &state.viewer_token).await;
    assert_eq!(status, 200);
    assert_eq!(first["data"], json!([original[1]]));
    let cursor = first["meta"]["next_cursor"].as_str().unwrap();
    let next = format!("{path}&cursor={cursor}");
    sqlx::query("UPDATE reliability_findings SET finding_json=json_set(finding_json,'$.status','interrupted'),status='interrupted' WHERE finding_id=?").bind(original[0]["finding_id"].as_str().unwrap()).execute(&state.db).await.unwrap();
    let (_, second) = get(&app, &next, &state.viewer_token).await;
    assert_eq!(second["data"], json!([original[0]]));
    assert_eq!(second["meta"]["server_time"], first["meta"]["server_time"]);
    assert_eq!(get(&app, &next, "admin").await.0, 400);
    assert_eq!(
        get(
            &app,
            &format!("/api/v1/reliability/sources?limit=1&cursor={cursor}"),
            &state.viewer_token
        )
        .await
        .0,
        400
    );
    assert_eq!(
        get(&app, "/api/v1/reliability/findings", "node-credential")
            .await
            .0,
        403
    );
    let detail = format!(
        "/api/v1/reliability/findings/{}",
        original[1]["finding_id"].as_str().unwrap()
    );
    assert_eq!(
        get(&app, &detail, &state.viewer_token).await.1["data"],
        original[1]
    );
    assert_eq!(
        get(
            &app,
            &format!("/api/v1/reliability/findings/{}", Uuid::new_v4()),
            &state.viewer_token
        )
        .await
        .0,
        404
    );
    let (_, cap) = get(&app, "/api/v1/capabilities", &state.viewer_token).await;
    assert_eq!(cap["data"]["features"]["drive_reliability"], true);
    assert_eq!(cap["data"]["features"]["twilio_notifications"], true);
}
