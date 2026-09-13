use axum::{
    Router,
    body::{Body, to_bytes},
    http::Request,
};
use chrono::Utc;
use cider_server::{api, read_api, store::AppState};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const ADMIN: &str = "detection-api-test-admin";
struct Harness {
    _directory: tempfile::TempDir,
    state: AppState,
    app: Router,
}
impl Harness {
    async fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let state = AppState::open(&directory.path().join("test.sqlite"), ADMIN)
            .await
            .unwrap();
        let app = api::router(state.clone()).merge(read_api::router(state.clone()));
        Self {
            _directory: directory,
            state,
            app,
        }
    }
    async fn request(
        &self,
        method: &str,
        path: &str,
        token: &str,
        body: Value,
        id: Option<&str>,
    ) -> (u16, Value) {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header("authorization", format!("Bearer {token}"))
            .header("content-type", "application/json");
        if let Some(id) = id {
            request = request
                .header("x-request-id", id)
                .header("x-request-timestamp", Utc::now().to_rfc3339());
        }
        let response = self
            .app
            .clone()
            .oneshot(
                request
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
        assert_eq!(response.headers().get("cache-control").unwrap(), "no-store");
        let body = serde_json::from_slice(
            &to_bytes(response.into_body(), 4 * 1024 * 1024)
                .await
                .unwrap(),
        )
        .unwrap();
        (status, body)
    }
    async fn get(&self, path: &str) -> (u16, Value) {
        self.request("GET", path, &self.state.viewer_token, Value::Null, None)
            .await
    }
    async fn source(&self) -> (String, String, String) {
        use cider_server::detection::{SourceIdentity, SourceState};
        let node = Uuid::new_v4().to_string();
        let source = Uuid::new_v4().to_string();
        let object = Uuid::new_v4().to_string();
        let now = Utc::now().timestamp_millis();
        sqlx::query("INSERT INTO nodes(node_id,name,agent_json,credential_hash,enrolled_at,last_seen_at) VALUES (?,'source owner','{}',?,0,?)")
            .bind(&node).bind(format!("credential-{node}")).bind(now).execute(&self.state.db).await.unwrap();
        let state = SourceState::new(SourceIdentity {
            source_id: source.clone(),
            node_id: node.clone(),
            object_id: object.clone(),
            resource_id: "native-driver".into(),
            direction: "read".into(),
            metric: "storage.device.read_bytes_total".into(),
            collector: "iokit.block".into(),
            scope: "driver".into(),
            attributes: Default::default(),
        });
        sqlx::query("INSERT INTO detection_sources(source_id,node_id,object_id,resource_id,direction,active,support_state,state_json,summary_json,boot_id,agent_generation,agent_session_id,updated_at) VALUES (?,?,?,'native-driver','read',1,'supported',?,?,'boot','1','session',?)")
            .bind(&source).bind(&node).bind(&object).bind(serde_json::to_string(&state).unwrap()).bind(serde_json::to_string(&state.summary(now,true)).unwrap()).bind(now)
            .execute(&self.state.db).await.unwrap();
        (source, node, object)
    }
    async fn finding(
        &self,
        source: &str,
        node: &str,
        object: &str,
        status: &str,
        at: i64,
    ) -> Value {
        let finding = json!({"finding_id":Uuid::new_v4().to_string(),"source_id":source,"node_id":node,"object_id":object,
            "status":status,"first_seen_at":cider_server::store::timestamp(at),
            "evidence":{"counter_start":"1844674407370955161600","counter_end":"1844674407370995161600"}});
        sqlx::query("INSERT INTO detection_findings(finding_id,source_id,node_id,object_id,status,first_seen_at,updated_at,ended_at,finding_json) VALUES (?,?,?,?,?,?,?,NULL,?)")
            .bind(finding["finding_id"].as_str().unwrap()).bind(source).bind(node).bind(object).bind(status).bind(at).bind(at).bind(finding.to_string())
            .execute(&self.state.db).await.unwrap();
        finding
    }
}

#[tokio::test]
async fn detection_reads_use_existing_roles_envelopes_and_explicit_empty_coverage() {
    let h = Harness::new().await;
    for path in [
        "/api/v1/findings",
        "/api/v1/detectors/storage-activity/sources",
        "/api/v1/security/rule-findings",
        "/api/v1/security/rule-sources",
    ] {
        let (status, body) = h.get(path).await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["data"], json!([]));
        assert_eq!(body["meta"]["api_version"], "1");
        assert!(body["meta"]["next_cursor"].is_null());
        assert_eq!(
            h.request("GET", path, "wrong", Value::Null, None).await.0,
            401
        );
    }
    let (_, sources) = h.get("/api/v1/detectors/storage-activity/sources").await;
    assert_eq!(sources["meta"]["coverage"]["known_sources"], 0);
    assert_eq!(
        sources["meta"]["coverage"]["security_assessment"],
        "unknown"
    );
    assert!(sources["meta"]["policy"].is_object());
    let (_, capabilities) = h.get("/api/v1/capabilities").await;
    assert_eq!(
        capabilities["data"]["features"]["storage_activity_detection"],
        true
    );
    assert_eq!(capabilities["data"]["features"]["findings"], true);
    assert_eq!(capabilities["data"]["features"]["alerts"], true);
    let node = Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO nodes(node_id,name,agent_json,credential_hash,enrolled_at) VALUES (?,'node','{}',?,0)")
        .bind(node).bind(cider_server::store::fingerprint(b"node-token")).execute(&h.state.db).await.unwrap();
    assert_eq!(
        h.request("GET", "/api/v1/findings", "node-token", Value::Null, None)
            .await
            .0,
        403
    );
}

#[tokio::test]
async fn detection_query_boundaries_and_mutation_roles_are_strict() {
    let h = Harness::new().await;
    for path in [
        "/api/v1/findings?status=quiet",
        "/api/v1/findings?status=open&status=all",
        "/api/v1/findings?from=nope",
        "/api/v1/findings?node_id=wrong",
        "/api/v1/findings?limit=501",
        "/api/v1/detectors/storage-activity/sources?status=open",
        "/api/v1/detectors/storage-activity/sources?object_id=wrong",
    ] {
        assert_eq!(h.get(path).await.0, 400, "{path}");
    }
    let id = Uuid::new_v4().to_string();
    assert_eq!(h.get(&format!("/api/v1/findings/{id}")).await.0, 404);
    let path = format!("/api/v1/detectors/storage-activity/sources/{id}/rebaseline");
    let body = json!({"expected_baseline_revision":"1","reason":"operator_reassessment"});
    let request_id = Uuid::new_v4().to_string();
    assert_eq!(
        h.request(
            "POST",
            &path,
            &h.state.viewer_token,
            body.clone(),
            Some(&request_id)
        )
        .await
        .0,
        403
    );
    assert_eq!(
        h.request("POST", &path, ADMIN, body.clone(), None).await.0,
        400
    );
    assert_eq!(
        h.request("POST", &path, ADMIN, body, Some(&request_id))
            .await
            .0,
        404
    );
    for body in [
        json!({"expected_baseline_revision":1,"reason":"operator_reassessment"}),
        json!({"expected_baseline_revision":"01","reason":"operator_reassessment"}),
        json!({"expected_baseline_revision":"1","reason":"silence"}),
        json!({"expected_baseline_revision":"1","reason":"operator_reassessment","extra":true}),
    ] {
        assert_eq!(
            h.request(
                "POST",
                &path,
                ADMIN,
                body,
                Some(&Uuid::new_v4().to_string())
            )
            .await
            .0,
            400
        );
    }
}

#[tokio::test]
async fn findings_keep_old_open_episodes_exact_evidence_and_frozen_pages() {
    let h = Harness::new().await;
    let (source, node, object) = h.source().await;
    let now = Utc::now().timestamp_millis();
    let old = h
        .finding(&source, &node, &object, "open", now - 40 * 86400_000)
        .await;
    let recent = h
        .finding(&source, &node, &object, "resolved", now - 1000)
        .await;
    let (status, open) = h.get("/api/v1/findings").await;
    assert_eq!(status, 200);
    assert_eq!(open["data"], json!([old.clone()]));
    let (_, detail) = h
        .get(&format!(
            "/api/v1/findings/{}",
            recent["finding_id"].as_str().unwrap()
        ))
        .await;
    assert_eq!(detail["data"], recent);
    let (_, first) = h.get("/api/v1/findings?status=all&limit=1").await;
    assert_eq!(first["data"], json!([recent]));
    let cursor = first["meta"]["next_cursor"].as_str().unwrap();
    let newer = h.finding(&source, &node, &object, "open", now).await;
    sqlx::query("UPDATE detection_findings SET finding_json=json_set(finding_json,'$.status','interrupted'),status='interrupted' WHERE finding_id=?")
        .bind(old["finding_id"].as_str().unwrap()).execute(&h.state.db).await.unwrap();
    let (status, second) = h
        .get(&format!(
            "/api/v1/findings?status=all&limit=1&cursor={cursor}"
        ))
        .await;
    assert_eq!(status, 200);
    assert_eq!(second["data"], json!([old]));
    assert_eq!(second["meta"]["server_time"], first["meta"]["server_time"]);
    assert!(second["meta"]["next_cursor"].is_null());
    assert_eq!(
        h.get(&format!(
            "/api/v1/findings?status=open&limit=1&cursor={cursor}"
        ))
        .await
        .0,
        400
    );
    assert_eq!(
        h.get(&format!("/api/v1/findings?cursor={}.0.1", Uuid::new_v4()))
            .await
            .0,
        410
    );
    let (_, filtered) = h
        .get(&format!(
            "/api/v1/findings?object_id={object}&from={}",
            cider_server::store::timestamp(now - 500)
        ))
        .await;
    assert_eq!(filtered["data"], json!([newer]));
    assert_eq!(
        h.get(&format!("/api/v1/findings?node_id={}", Uuid::new_v4()))
            .await
            .1["data"],
        json!([])
    );
}

#[tokio::test]
async fn administrator_rebaseline_is_revision_guarded_audited_and_replay_safe() {
    let h = Harness::new().await;
    let (source, node, _) = h.source().await;
    let path = format!("/api/v1/detectors/storage-activity/sources/{source}/rebaseline");
    let body = json!({"expected_baseline_revision":"1","reason":"planned_workload_change"});
    let id = Uuid::new_v4().to_string();
    let (status, result) = h
        .request("POST", &path, ADMIN, body.clone(), Some(&id))
        .await;
    assert_eq!(status, 200, "{result}");
    assert_eq!(result["data"]["baseline_revision"], "2");
    let (status, replay) = h
        .request("POST", &path, ADMIN, body.clone(), Some(&id))
        .await;
    assert_eq!(status, 409);
    assert_eq!(replay["error"]["code"], "request_replayed");
    assert_eq!(
        h.request(
            "POST",
            &path,
            ADMIN,
            body,
            Some(&Uuid::new_v4().to_string())
        )
        .await
        .0,
        409
    );
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM detection_rebaseline_audit WHERE source_id=?")
            .bind(&source)
            .fetch_one(&h.state.db)
            .await
            .unwrap();
    assert_eq!(count, 1);
    let (_, sources) = h
        .get(&format!(
            "/api/v1/detectors/storage-activity/sources?node_id={node}"
        ))
        .await;
    assert_eq!(sources["data"][0]["baseline_revision"], "2");
    let (_, cluster) = h.get("/api/v1/cluster").await;
    assert_eq!(
        cluster["data"]["health"]["dimensions"]["security_activity"]["status"],
        "unknown"
    );
}

#[tokio::test]
async fn only_current_open_findings_raise_security_warning() {
    let h = Harness::new().await;
    let (source, node, _) = h.source().await;
    let now = Utc::now().timestamp_millis();
    let stored: String =
        sqlx::query_scalar("SELECT summary_json FROM detection_sources WHERE source_id=?")
            .bind(&source)
            .fetch_one(&h.state.db)
            .await
            .unwrap();
    let mut summary: Value = serde_json::from_str(&stored).unwrap();
    summary["episode"] = json!({"state":"open","finding_id":Uuid::new_v4().to_string(),"elevated_seconds":120,"recovery_seconds":0});
    summary["observation"] = json!({"state":"current","reason":null,"observed_at":cider_server::store::timestamp(now),
        "received_at":cider_server::store::timestamp(now),"age_seconds":0,"stale_after_seconds":15,"rate_bytes_per_second":8_000_000});
    for (state, expected) in [
        ("current", "warning"),
        ("unavailable", "unknown"),
        ("stale", "unknown"),
    ] {
        summary["observation"]["state"] = json!(state);
        sqlx::query("UPDATE detection_sources SET summary_json=? WHERE source_id=?")
            .bind(summary.to_string())
            .bind(&source)
            .execute(&h.state.db)
            .await
            .unwrap();
        for path in [
            "/api/v1/cluster".to_owned(),
            format!("/api/v1/nodes/{node}"),
        ] {
            let (status, body) = h.get(&path).await;
            assert_eq!(status, 200, "{body}");
            assert_eq!(
                body["data"]["health"]["dimensions"]["security_activity"]["status"],
                expected
            );
        }
    }
    summary["observation"]["state"] = json!("current");
    summary["episode"]["state"] = json!("quiet");
    sqlx::query("UPDATE detection_sources SET summary_json=? WHERE source_id=?")
        .bind(summary.to_string())
        .bind(&source)
        .execute(&h.state.db)
        .await
        .unwrap();
    assert_eq!(
        h.get("/api/v1/cluster").await.1["data"]["health"]["dimensions"]["security_activity"]["status"],
        "unknown"
    );
}
