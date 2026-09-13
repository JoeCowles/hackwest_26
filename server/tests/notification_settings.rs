use orchard_server::{notifications, store::AppState};
use serde_json::json;
#[tokio::test]
async fn settings_default_disabled_masked_and_revision_guarded() {
    let dir = tempfile::tempdir().unwrap();
    let s = AppState::open(&dir.path().join("db"), "admin")
        .await
        .unwrap();
    let initial = notifications::settings(&s).await.unwrap();
    assert_eq!(initial["enabled"], false);
    let mut tx = s.db.begin().await.unwrap();
    assert!(notifications::configure(&mut tx,&json!({"expected_revision":"initial","enabled":true,"sender":"not E164","recipient":"+15555550123"}),"actor",100).await.is_err());
    let result=notifications::configure(&mut tx,&json!({"expected_revision":"initial","enabled":true,"sender":"+15555550124","recipient":"+15555550123"}),"actor",101).await.unwrap();
    assert_eq!(result["enabled"], true);
    assert_ne!(result["recipient"], "+15555550123");
    assert_eq!(result["recipient"], "***0123");
    assert!(
        notifications::configure(
            &mut tx,
            &json!({"expected_revision":"initial","enabled":false}),
            "actor",
            102
        )
        .await
        .is_err()
    );
    tx.commit().await.unwrap();
    let view = notifications::settings(&s).await.unwrap();
    assert!(!view.to_string().contains("1555555"));
}
