use cider_server::{
    history,
    store::{AppState, timestamp},
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
const NOW: i64 = 1_789_300_000_000;
async fn setup() -> (tempfile::TempDir, AppState) {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::open(&dir.path().join("test.db"), "secret")
        .await
        .unwrap();
    sqlx::query("INSERT INTO nodes(node_id,name,agent_json,credential_hash,enrolled_at,last_seen_at,boot_id,inventory_generation) VALUES ('n','n','{}','h',?,?,'boot',1)").bind(NOW).bind(NOW).execute(&state.db).await.unwrap();
    sqlx::query("INSERT INTO objects(object_id,node_id,local_id,kind,parents_json,properties_json,generation) VALUES ('o','n','o','apfs_container','[]','{}',1)").execute(&state.db).await.unwrap();
    (dir, state)
}
async fn sample(
    s: &AppState,
    name: &str,
    at: i64,
    value: Value,
    source: &str,
    boot: &str,
    state: &str,
) {
    sqlx::query("INSERT INTO metric_samples(node_id,object_id,boot_id,generation,name,kind,unit,state,source,scope,labels_json,value_json,numeric_value,observed_at,received_at,derivation_state) VALUES ('n','o',?,1,?,'gauge','bytes',?,?,'container','{}',?,?,?,?,'unavailable')")
 .bind(boot).bind(name).bind(state).bind(source).bind(value.to_string()).bind(value.as_str().and_then(|v|v.parse::<f64>().ok())).bind(at).bind(at).execute(&s.db).await.unwrap();
}
fn query(res: &str, from: i64, to: i64) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("metric".into(), "used_bytes".into()),
        ("resolution".into(), res.into()),
        ("from".into(), timestamp(from)),
        ("to".into(), timestamp(to)),
    ])
}
async fn trend(s: &AppState, values: &[u128], capacity: u128, step: i64) {
    for (i, v) in values.iter().enumerate() {
        let at = NOW - (values.len() - 1 - i) as i64 * step;
        sample(
            s,
            "used_bytes",
            at,
            json!(v.to_string()),
            "test",
            "boot",
            "ok",
        )
        .await;
        sample(
            s,
            "capacity_bytes",
            at,
            json!(capacity.to_string()),
            "test",
            "boot",
            "ok",
        )
        .await;
    }
}
#[tokio::test]
async fn raw_preserves_exact_integers_and_series_identity() {
    let (_d, s) = setup().await;
    let wide = "340282366920938463463374607431768211450";
    sample(
        &s,
        "used_bytes",
        NOW - 15000,
        serde_json::from_str("9007199254740993").unwrap(),
        "a",
        "boot",
        "ok",
    )
    .await;
    sample(
        &s,
        "used_bytes",
        NOW - 10000,
        json!(wide),
        "a",
        "boot",
        "ok",
    )
    .await;
    sample(
        &s,
        "used_bytes",
        NOW - 5000,
        json!("10"),
        "b",
        "boot",
        "failed",
    )
    .await;
    sample(&s, "used_bytes", NOW, json!("11"), "a", "older", "ok").await;
    let (data, meta) = history::read(&s, "o", &query("raw", NOW - 20000, NOW), NOW)
        .await
        .unwrap();
    assert_eq!(data["series"].as_array().unwrap().len(), 3);
    assert_eq!(meta["selected_points"], 4);
    let pts: Vec<_> = data["series"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|x| x["points"].as_array().unwrap())
        .collect();
    assert!(pts.iter().any(|p| p["value"] == wide));
    assert!(pts.iter().any(|p| p["value"] == "9007199254740993"));
    assert!(pts.iter().any(|p| p["state"] == "failed"));
}
#[tokio::test]
async fn rollups_expose_state_counts_and_coverage_without_raw_precision_claim() {
    let (_d, s) = setup().await;
    sqlx::query("INSERT INTO metric_rollups VALUES (300,?,'o','boot',1,'used_bytes','gauge','bytes','a','container','{}',10,20,15,3,2,'{\"ok\":2,\"failed\":1}')").bind(NOW-300000).execute(&s.db).await.unwrap();
    let (d, _) = history::read(&s, "o", &query("5m", NOW - 600000, NOW), NOW)
        .await
        .unwrap();
    let p = &d["series"][0]["points"][0];
    assert_eq!(p["state_counts"]["failed"], 1);
    assert_eq!(p["coverage"], json!({"observed":2,"expected":3}));
    assert_eq!(p["state"], "partial");
    assert_eq!(p["mean"], 15.0);
}
#[tokio::test]
async fn windows_and_point_limit_reject_instead_of_truncating() {
    let (_d, s) = setup().await;
    for q in [
        BTreeMap::new(),
        query("bad", NOW - 1000, NOW),
        query("raw", NOW - 1000, NOW + 1),
        query("raw", NOW, NOW - 1),
        query("raw", NOW - 2 * 86400000, NOW),
    ] {
        assert!(history::read(&s, "o", &q, NOW).await.is_err());
    }
    let mut q = query("raw", NOW - 3000000, NOW);
    q.insert("unexpected".into(), "x".into());
    assert!(history::read(&s, "o", &q, NOW).await.is_err());
    for i in 0..2001 {
        sample(
            &s,
            "used_bytes",
            NOW - i * 1000,
            json!("1"),
            "a",
            "boot",
            "ok",
        )
        .await;
    }
    assert_eq!(
        history::read(&s, "o", &query("raw", NOW - 3000000, NOW), NOW)
            .await
            .unwrap_err()
            .code,
        "history_too_large"
    );
}
#[tokio::test]
async fn linear_growth_reports_capacity_date_range_and_basis() {
    let (_d, s) = setup().await;
    trend(&s, &[100, 110, 120, 130, 140, 150, 160, 170], 1000, 600000).await;
    let f = history::forecast(&s, "o", NOW).await.unwrap();
    assert_eq!(f["state"], "estimated", "{f}");
    assert!(f["estimated_exhaustion_at"].is_string());
    assert!(f["scenario_range"]["earliest_at"].is_string());
    assert_eq!(f["basis"]["sample_count"], 8);
}
#[tokio::test]
async fn forecast_rejects_insufficient_flat_decreasing_nonlinear_and_gaps() {
    for (values, step, reason) in [
        (vec![100, 110], 600000, "insufficient_data"),
        (vec![100; 8], 600000, "no_reliable_growth"),
        (
            vec![170, 160, 150, 140, 130, 120, 110, 100],
            600000,
            "no_reliable_growth",
        ),
        (vec![1, 2, 4, 8, 16, 32, 64, 128], 600000, "unstable_growth"),
        (
            vec![100, 110, 120, 130, 140, 150, 160, 170],
            4000000,
            "history_gap",
        ),
    ] {
        let (_d, s) = setup().await;
        trend(&s, &values, 1000, step).await;
        let f = history::forecast(&s, "o", NOW).await.unwrap();
        assert_eq!(f["state"], "unknown", "{f}");
        assert!(
            f["reasons"].as_array().unwrap().contains(&json!(reason)),
            "{f}"
        );
        assert!(f["estimated_exhaustion_at"].is_null());
    }
}
#[tokio::test]
async fn forecast_rejects_changed_capacity_old_owner_and_ambiguous_sources() {
    for mode in ["capacity", "owner", "source", "bad_observation"] {
        let (_d, s) = setup().await;
        trend(&s, &[100, 110, 120, 130, 140, 150, 160, 170], 1000, 600000).await;
        match mode {
            "capacity" => {
                sqlx::query("UPDATE metric_samples SET value_json='\"2000\"' WHERE name='capacity_bytes' AND observed_at=?").bind(NOW).execute(&s.db).await.unwrap();
            }
            "owner" => {
                sqlx::query("UPDATE nodes SET last_seen_at=?")
                    .bind(NOW - 60000)
                    .execute(&s.db)
                    .await
                    .unwrap();
                sqlx::query("INSERT INTO nodes(node_id,name,agent_json,credential_hash,enrolled_at,last_seen_at) VALUES ('unrelated','other','{}','other',0,?)").bind(NOW).execute(&s.db).await.unwrap();
            }
            "source" => sample(&s, "used_bytes", NOW, json!("170"), "other", "boot", "ok").await,
            _ => {
                sqlx::query("UPDATE metric_samples SET state='failed' WHERE name='used_bytes' AND observed_at=?").bind(NOW-600000).execute(&s.db).await.unwrap();
            }
        }
        let f = history::forecast(&s, "o", NOW).await.unwrap();
        assert_eq!(f["state"], "unknown", "{mode}: {f}");
        assert!(f["estimated_exhaustion_at"].is_null());
    }
}
#[tokio::test]
async fn forecast_subtracts_wide_integers_before_float_conversion() {
    let (_d, s) = setup().await;
    let base = 1u128 << 100;
    trend(
        &s,
        &(0..8).map(|i| base + i * 10).collect::<Vec<_>>(),
        base + 1000,
        600000,
    )
    .await;
    let f = history::forecast(&s, "o", NOW).await.unwrap();
    assert_eq!(f["state"], "estimated", "{f}");
    assert!((f["basis"]["growth_bytes_per_second"].as_f64().unwrap() - 1. / 60.).abs() < 1e-10);
}
#[tokio::test]
async fn raw_gaps_and_generation_label_scope_boundaries_remain_visible() {
    let (_d, s) = setup().await;
    sample(
        &s,
        "used_bytes",
        NOW - 7200000,
        json!("1"),
        "a",
        "boot",
        "ok",
    )
    .await;
    sample(&s, "used_bytes", NOW, json!("2"), "a", "boot", "ok").await;
    for (idx, change) in [
        "generation=2",
        "labels_json='{\"x\":\"y\"}'",
        "scope='other'",
    ]
    .iter()
    .enumerate()
    {
        let at = NOW - 1000 * (idx + 1) as i64;
        sample(&s, "used_bytes", at, json!("3"), "a", "boot", "ok").await;
        sqlx::query(&format!(
            "UPDATE metric_samples SET {change} WHERE observed_at=?"
        ))
        .bind(at)
        .execute(&s.db)
        .await
        .unwrap();
    }
    let (d, _) = history::read(&s, "o", &query("raw", NOW - 8000000, NOW), NOW)
        .await
        .unwrap();
    assert_eq!(d["series"].as_array().unwrap().len(), 4);
    assert!(
        d["series"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["points"][1]["gap_before"] == true)
    );
}
#[tokio::test]
async fn forecast_rejects_unpaired_short_span_and_changed_owner_identity() {
    for mode in ["unpaired", "short", "generation", "boot", "inactive"] {
        let (_d, s) = setup().await;
        trend(
            &s,
            &[100, 110, 120, 130, 140, 150, 160, 170],
            1000,
            if mode == "short" { 100000 } else { 600000 },
        )
        .await;
        let sql = match mode {
            "unpaired" => {
                "DELETE FROM metric_samples WHERE name='capacity_bytes' AND observed_at=(SELECT MIN(observed_at) FROM metric_samples)"
            }
            "generation" => "UPDATE nodes SET inventory_generation=2",
            "boot" => "UPDATE nodes SET boot_id='new'",
            "inactive" => "UPDATE objects SET active=0",
            _ => "SELECT 1",
        };
        sqlx::query(sql).execute(&s.db).await.unwrap();
        let f = history::forecast(&s, "o", NOW).await.unwrap();
        assert_eq!(f["state"], "unknown", "{mode}: {f}");
    }
}
async fn native(s: &AppState, age: f64, policy: u64, available: bool) {
    sqlx::query("UPDATE metric_samples SET source='ciderd:diskutil',labels_json='{\"ciderd_clock_id\":\"clock\"}'").execute(&s.db).await.unwrap();
    for (name, value) in [("used_bytes", "170"), ("capacity_bytes", "1000")] {
        let raw = json!({"sample":{"state":"ok","value":value},"boot_id":"boot","inventory_generation":"1","received_at":timestamp(NOW),"ciderd":{"age_at_receipt_seconds":age,"stale_after_seconds":policy,"clock_id":"clock","original_inventory_generation":"1","source_metric":{"availability":if available{"available"}else{"not_collected"},"freshness":"live"}}});
        sqlx::query(
            "INSERT INTO latest_samples VALUES ('o',?,'ciderd:diskutil','container','{}',?,?,?)",
        )
        .bind(name)
        .bind(raw.to_string())
        .bind(NOW)
        .bind(NOW)
        .execute(&s.db)
        .await
        .unwrap();
    }
}
#[tokio::test]
async fn native_forecast_obeys_monotonic_age_and_source_availability() {
    for (age, policy, available, expected) in [
        (120., 1800, true, "estimated"),
        (1801., 1800, true, "unknown"),
        (0., 1800, false, "unknown"),
    ] {
        let (_d, s) = setup().await;
        trend(&s, &[100, 110, 120, 130, 140, 150, 160, 170], 1000, 600000).await;
        native(&s, age, policy, available).await;
        let f = history::forecast(&s, "o", NOW).await.unwrap();
        assert_eq!(f["state"], expected, "{f}");
    }
}
#[tokio::test]
async fn native_retained_failure_and_missing_freshness_reject_forecast() {
    for mode in ["retained", "missing"] {
        let (_d, s) = setup().await;
        trend(&s, &[100, 110, 120, 130, 140, 150, 160, 170], 1000, 600000).await;
        native(&s, 0., 1800, true).await;
        let sql = if mode == "retained" {
            "UPDATE latest_samples SET sample_json=json_set(sample_json,'$.ciderd.retained_after_failure',json('true'))"
        } else {
            "DELETE FROM latest_samples"
        };
        sqlx::query(sql).execute(&s.db).await.unwrap();
        assert_eq!(
            history::forecast(&s, "o", NOW).await.unwrap()["state"],
            "unknown"
        );
    }
}
