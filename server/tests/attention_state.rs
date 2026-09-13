use orchard_server::{
    attention::{self, Condition},
    store::AppState,
};
use serde_json::json;

async fn fixture() -> (tempfile::TempDir, AppState) {
    let d = tempfile::tempdir().unwrap();
    let s = AppState::open(&d.path().join("test.db"), "admin")
        .await
        .unwrap();
    (d, s)
}
fn condition(status: &str, severity: &str) -> Condition {
    Condition {
        key: "disk:one".into(),
        kind: "reliability".into(),
        node_id: "node".into(),
        object_id: Some("disk".into()),
        status: status.into(),
        severity: severity.into(),
        observation_state: "ok".into(),
        summary: "Explicit disk warning".into(),
        evidence: json!({"reason":"critical_warning"}),
    }
}
#[tokio::test]
async fn episode_ack_escalation_recovery_recurrence_are_durable() {
    let (d, s) = fixture().await;
    let mut tx = s.db.begin().await.unwrap();
    let first = attention::observe_condition(&mut tx, &condition("open", "warning"), 1000)
        .await
        .unwrap()
        .unwrap();
    let duplicate = attention::observe_condition(&mut tx, &condition("open", "warning"), 2000)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first["id"], duplicate["id"]);
    assert_eq!(first["revision"], duplicate["revision"]);
    let ack = attention::acknowledge(
        &mut tx,
        first["id"].as_str().unwrap(),
        first["revision"].as_str().unwrap(),
        "Investigating",
        "actor",
        2500,
    )
    .await
    .unwrap();
    assert!(ack["acknowledgement"].is_object());
    assert!(
        attention::acknowledge(
            &mut tx,
            first["id"].as_str().unwrap(),
            "stale",
            "",
            "actor",
            2501
        )
        .await
        .is_err()
    );
    let escalated = attention::observe_condition(&mut tx, &condition("open", "critical"), 3000)
        .await
        .unwrap()
        .unwrap();
    assert!(escalated["acknowledgement"].is_null());
    let mut lost = condition("interrupted", "critical");
    lost.observation_state = "unknown".into();
    let unknown = attention::observe_condition(&mut tx, &lost, 4000)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(unknown["status"], "open");
    assert_eq!(unknown["observation_state"], "unknown");
    let closed = attention::observe_condition(&mut tx, &condition("resolved", "critical"), 5000)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(closed["status"], "resolved");
    let next = attention::observe_condition(&mut tx, &condition("open", "warning"), 6000)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(first["id"], next["id"]);
    tx.commit().await.unwrap();
    s.db.close().await;
    let reopened = AppState::open(&d.path().join("test.db"), "admin")
        .await
        .unwrap();
    let (rows, _) = attention::list(&reopened, &Default::default(), 7000)
        .await
        .unwrap();
    assert_eq!(rows.as_array().unwrap().len(), 2);
}
#[tokio::test]
async fn capacity_missing_does_not_recover_and_invalid_ratio_is_ignored() {
    let (_d, s) = fixture().await;
    let mut tx = s.db.begin().await.unwrap();
    attention::observe_capacity(
        &mut tx,
        "capacity:pool",
        "node",
        Some("pool"),
        &json!({"state":"ok","value":0.96}),
        1000,
    )
    .await
    .unwrap();
    attention::observe_capacity(
        &mut tx,
        "capacity:pool",
        "node",
        Some("pool"),
        &json!({"state":"unknown","value":null}),
        2000,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let (rows, _) = attention::list(&s, &Default::default(), 3000)
        .await
        .unwrap();
    assert_eq!(rows[0]["status"], "open");
    assert_eq!(rows[0]["observation_state"], "unknown");
}

#[tokio::test]
async fn reconcile_preserves_source_staleness_and_node_loss_identity() {
    let (_d, s) = fixture().await;
    sqlx::query("INSERT INTO nodes(node_id,name,agent_json,credential_hash,enrolled_at,last_seen_at) VALUES('node','Node','{}','test',0,1000)").execute(&s.db).await.unwrap();
    sqlx::query("INSERT INTO detection_sources(source_id,node_id,object_id,resource_id,direction,active,support_state,state_json,summary_json,boot_id,agent_generation,agent_session_id,updated_at) VALUES('source','node','disk','resource','read',1,'supported','{}',?,'boot','generation','session',1000)")
 .bind(json!({"source_id":"source","active":true,"observation":{"state":"current","age_seconds":0,"stale_after_seconds":15}}).to_string()).execute(&s.db).await.unwrap();
    sqlx::query("INSERT INTO detection_findings(finding_id,source_id,node_id,object_id,status,first_seen_at,updated_at,finding_json) VALUES('finding','source','node','disk','open',1000,1000,?)").bind(json!({"severity":"warning","summary":"Activity elevated","source_id":"source"}).to_string()).execute(&s.db).await.unwrap();
    attention::reconcile(&s, 101_000).await.unwrap();
    attention::reconcile(&s, 106_000).await.unwrap();
    let (rows, _) = attention::list(&s, &Default::default(), 106_000)
        .await
        .unwrap();
    let activity = rows
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["kind"] == "activity")
        .unwrap();
    assert_eq!(activity["observation_state"], "stale");
    assert_eq!(
        rows.as_array()
            .unwrap()
            .iter()
            .filter(|v| v["kind"] == "node_loss")
            .count(),
        1
    );
}

#[tokio::test]
async fn current_diagnostics_can_recover_and_delivery_is_visible() {
    let (_d, s) = fixture().await;
    let mut tx = s.db.begin().await.unwrap();
    let mut c = condition("open", "warning");
    c.observation_state = "current".into();
    attention::observe_condition(&mut tx, &c, 1000)
        .await
        .unwrap();
    c.status = "resolved".into();
    let row = attention::observe_condition(&mut tx, &c, 2000)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row["status"], "resolved");
    tx.commit().await.unwrap();
    let (rows, _) = attention::list(&s, &Default::default(), 3000)
        .await
        .unwrap();
    assert_eq!(rows[0]["notification"]["state"], "suppressed");
    let bad = std::collections::BTreeMap::from([("acknowledged".into(), "banana".into())]);
    assert!(attention::list(&s, &bad, 3000).await.is_err());
}

#[tokio::test]
async fn disappeared_source_stays_unresolved_but_unknown() {
    let (_d, s) = fixture().await;
    let mut tx = s.db.begin().await.unwrap();
    attention::observe_condition(&mut tx, &condition("open", "warning"), 1000)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    attention::reconcile(&s, 2000).await.unwrap();
    let (rows, _) = attention::list(&s, &Default::default(), 2000)
        .await
        .unwrap();
    assert_eq!(rows[0]["status"], "open");
    assert_eq!(rows[0]["observation_state"], "unknown");
}
#[tokio::test]
async fn list_supplies_all_matched_rows_to_frozen_paginator() {
    let (_d, s) = fixture().await;
    let mut tx = s.db.begin().await.unwrap();
    for index in 0..3 {
        let mut c = condition("open", "warning");
        c.key = format!("disk:{index}");
        attention::observe_condition(&mut tx, &c, 1000 + index)
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();
    let (rows, _) = attention::list(
        &s,
        &std::collections::BTreeMap::from([("limit".into(), "1".into())]),
        2000,
    )
    .await
    .unwrap();
    assert_eq!(rows.as_array().unwrap().len(), 3);
}

#[tokio::test]
async fn queue_freshness_tracks_successful_reconcile_not_read_time() {
    let (_d, s) = fixture().await;
    let first = attention::summary(&s, 1000).await.unwrap();
    assert_eq!(first["reconciliation"]["state"], "unknown");
    attention::reconcile(&s, 2000).await.unwrap();
    assert_eq!(
        attention::summary(&s, 3000).await.unwrap()["reconciliation"]["state"],
        "current"
    );
    assert_eq!(
        attention::summary(&s, 20_000).await.unwrap()["reconciliation"]["state"],
        "stale"
    );
}

#[tokio::test]
async fn failed_reconcile_keeps_last_success_and_recovers_without_changing_evidence() {
    let (_d, s) = fixture().await;
    attention::record_reconcile_failure(&s, 1000).await.unwrap();
    let unknown = attention::summary(&s, 1001).await.unwrap();
    assert_eq!(unknown["reconciliation"]["state"], "unknown");
    assert_eq!(unknown["reconciliation"]["error"], "reconciliation_failed");
    attention::reconcile(&s, 2000).await.unwrap();
    let mut tx = s.db.begin().await.unwrap();
    attention::observe_condition(&mut tx, &condition("open", "warning"), 2000)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    attention::record_reconcile_failure(&s, 3000).await.unwrap();
    let failed = attention::summary(&s, 4000).await.unwrap();
    assert_eq!(failed["reconciliation"]["state"], "failed");
    assert_eq!(
        failed["reconciliation"]["last_successful_at"],
        "1970-01-01T00:00:02.000Z"
    );
    assert_eq!(
        failed["reconciliation"]["last_attempted_at"],
        "1970-01-01T00:00:03.000Z"
    );
    let (rows, meta) = attention::list(&s, &Default::default(), 4000)
        .await
        .unwrap();
    assert_eq!(rows[0]["observation_state"], "ok");
    assert_eq!(rows[0]["reconciliation_state"], "failed");
    assert_eq!(meta["reconciliation"]["state"], "failed");
    attention::reconcile(&s, 5000).await.unwrap();
    let recovered = attention::summary(&s, 5001).await.unwrap();
    assert_eq!(recovered["reconciliation"]["state"], "current");
    assert!(recovered["reconciliation"]["error"].is_null());
}

#[tokio::test]
async fn shared_observer_changes_update_attribution_without_duplicate_episode() {
    let (_d, s) = fixture().await;
    let mut tx = s.db.begin().await.unwrap();
    let mut c = condition("open", "warning");
    c.key = "shared:filesystem".into();
    let a = attention::observe_condition(&mut tx, &c, 1000)
        .await
        .unwrap()
        .unwrap();
    c.node_id = "another-node".into();
    c.object_id = Some("another-mount".into());
    c.evidence = json!({"observer":"another-node"});
    let b = attention::observe_condition(&mut tx, &c, 2000)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(a["id"], b["id"]);
    assert_eq!(b["node_id"], "another-node");
    assert_eq!(b["object_id"], "another-mount");
}

#[tokio::test]
async fn clock_rollback_keeps_active_episode_and_future_seen_is_unknown() {
    let (_d, s) = fixture().await;
    let mut tx = s.db.begin().await.unwrap();
    attention::observe_condition(&mut tx, &condition("open", "warning"), 10000)
        .await
        .unwrap();
    attention::observe_condition(&mut tx, &condition("resolved", "warning"), 11000)
        .await
        .unwrap();
    let active = attention::observe_condition(&mut tx, &condition("open", "warning"), 9000)
        .await
        .unwrap()
        .unwrap();
    let repeated = attention::observe_condition(&mut tx, &condition("open", "warning"), 9001)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(active["id"], repeated["id"]);
    tx.commit().await.unwrap();
    sqlx::query("INSERT INTO nodes(node_id,name,agent_json,credential_hash,enrolled_at,last_seen_at) VALUES('node','Node','{}','test',0,1000)").execute(&s.db).await.unwrap();
    attention::reconcile(&s, 100000).await.unwrap();
    sqlx::query("UPDATE nodes SET last_seen_at=200000 WHERE node_id='node'")
        .execute(&s.db)
        .await
        .unwrap();
    attention::reconcile(&s, 101000).await.unwrap();
    let (rows, _) = attention::list(&s, &Default::default(), 101000)
        .await
        .unwrap();
    let loss = rows
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["kind"] == "node_loss")
        .unwrap();
    assert_eq!(loss["status"], "open");
    assert_eq!(loss["observation_state"], "unknown");
}
