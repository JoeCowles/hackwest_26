use cider_server::quota::project;
use serde_json::{Value, json};
fn node() -> Value {
    json!({"node_id":"n","name":"Node","availability":"online","boot_id":"b","agent_generation":"1","agent_session_id":"s"})
}
fn object() -> Value {
    let mut o = json!({"object_id":"q","node_id":"n","kind":"quota","active":true,"properties":{"ciderd_resource_type":"nfs_user_quota","server":"nfs.example","export_path":"/data","nfs_source":"nfs.example:/data","uid":"501"},"latest_metrics":{}});
    for (k, v) in [
        ("status", json!("available")),
        ("active", json!(true)),
        ("used_bytes", json!("65536")),
        ("block_soft_limit_bytes", json!("0")),
        ("block_hard_limit_bytes", json!("9223372030412324865")),
        ("used_inodes", json!("2")),
        ("inode_soft_limit", json!("10")),
        ("inode_hard_limit", json!("20")),
        ("block_grace_seconds_raw", json!("4294967295")),
        ("inode_grace_seconds_raw", json!("0")),
        ("block_size_bytes", json!("1024")),
    ] {
        o["latest_metrics"][format!("nfs_quota_{k}")] = json!({"value":v,"state":"ok","observed_at":"2026-09-13T12:00:00Z","age_seconds":10,"ciderd":{"collection_id":"a"}});
    }
    o
}
#[test]
fn exact_limits_and_expired_grace_are_evidence_based() {
    let p = project(&node(), &object(), &[], 0);
    assert_eq!(p["state"], "available");
    assert_eq!(p["limits"]["block_soft"]["state"], "unlimited");
    assert_eq!(p["limits"]["block_hard"]["value"], "9223372030412324865");
    assert_eq!(p["grace"]["block"]["seconds_signed"], "-1");
    assert_eq!(p["grace"]["block"]["state"], "expired");
    assert_eq!(p["grace"]["inode"]["state"], "not_active");
}
#[test]
fn failed_or_noquota_cannot_reuse_zero_as_unlimited() {
    for status in [
        "no_quota",
        "permission_denied",
        "timeout",
        "unavailable",
        "unsupported",
    ] {
        let mut o = object();
        o["latest_metrics"]["nfs_quota_status"]["value"] = json!(status);
        let p = project(&node(), &o, &[], 0);
        assert_eq!(p["state"], status);
        assert_eq!(p["limits"]["block_soft"]["state"], "unknown");
    }
}
#[test]
fn mismatched_sample_and_offline_owner_are_not_current() {
    let mut o = object();
    o["latest_metrics"]["nfs_quota_used_bytes"]["ciderd"]["collection_id"] = json!("old");
    assert_eq!(project(&node(), &o, &[], 0)["state"], "partial");
    let mut n = node();
    n["availability"] = json!("offline");
    let p = project(&n, &object(), &[], 0);
    assert_eq!(p["state"], "stale");
    assert_eq!(p["limits"]["block_soft"]["state"], "unknown");
}
#[test]
fn configured_source_links_only_exact_current_same_node_nfs_mount() {
    let a = json!({"object_id":"mount","node_id":"n","active":true,"properties":{"filesystem_type":"nfs","source":"nfs.example:/data"}});
    let mut b = a.clone();
    b["node_id"] = json!("other");
    b["object_id"] = json!("other");
    let p = project(&node(), &object(), &[a, b], 0);
    assert_eq!(p["linked_mount_ids"], json!(["mount"]));
    assert_eq!(p["linkage_state"], "exact_configured_source");
}
#[test]
fn latest_worker_timeout_overrides_retained_available_status() {
    let mut o = object();
    o["properties"]["ciderd_collector_states"] = json!([{"state":{"collector":"nfs.rquota"},"last_attempt":{"collection_id":"new","status":"timeout","finished_at":"2026-09-13T12:00:05Z"},"acquisition":{"boot_id":"b","agent_generation":"1","agent_session_id":"s"}}]);
    let p = project(&node(), &o, &[], 0);
    assert_eq!(p["state"], "timeout");
    assert_eq!(p["limits"]["block_soft"]["state"], "unknown");
}
#[test]
fn grace_expiry_accounts_for_observation_age() {
    let mut o = object();
    o["latest_metrics"]["nfs_quota_block_grace_seconds_raw"]["value"] = json!("5");
    let p = project(&node(), &o, &[], 0);
    assert_eq!(p["grace"]["block"]["state"], "expired");
    assert_eq!(p["grace"]["block"]["seconds_signed"], "5");
}
fn configured_node(id: &str) -> (Value, Value) {
    let context = json!({"boot_id":"b","agent_generation":"1","agent_session_id":"s"});
    let n = json!({"node_id":id,"availability":"online","boot_id":"b","agent_generation":"1","agent_session_id":"s"});
    let o = json!({"node_id":id,"kind":"node","active":true,"properties":{"diagnostics_configuration":{"nfs_quotas_enabled":false},"ciderd_acquisition":context}});
    (n, o)
}
#[test]
fn quota_unconfigured_requires_every_selected_current_node_configuration() {
    use cider_server::quota::coverage;
    let (n, a) = configured_node("a");
    let (m, mut b) = configured_node("b");
    let q = std::collections::BTreeMap::new();
    assert_eq!(
        coverage(&[n.clone()], &[a.clone()], &[], &q)["state"],
        "unconfigured"
    );
    b["properties"]["diagnostics_configuration"] = Value::Null;
    assert_eq!(
        coverage(&[n.clone(), m.clone()], &[a.clone(), b.clone()], &[], &q)["state"],
        "unknown"
    );
    let mut old = a.clone();
    old["properties"]["ciderd_acquisition"]["boot_id"] = json!("prior");
    assert_eq!(coverage(&[n.clone()], &[old], &[], &q)["state"], "unknown");
    let mut off = n.clone();
    off["availability"] = json!("offline");
    assert_eq!(coverage(&[off], &[a.clone()], &[], &q)["state"], "unknown");
    let q = std::collections::BTreeMap::from([("node_id".into(), "a".into())]);
    assert_eq!(coverage(&[n, m], &[a, b], &[], &q)["state"], "unconfigured");
}
#[test]
fn quota_uid_filter_with_no_matching_rows_is_not_available_coverage() {
    use cider_server::quota::coverage;
    let (n, mut h) = configured_node("n");
    h["properties"]["diagnostics_configuration"]["nfs_quotas_enabled"] = json!(true);
    let q = std::collections::BTreeMap::from([("uid".into(), "999".into())]);
    assert_eq!(coverage(&[n], &[h, object()], &[], &q)["state"], "empty");
}
#[test]
fn old_boot_worker_failure_cannot_override_stale_quota_observation() {
    let mut n = node();
    n["boot_id"] = json!("new");
    n["agent_generation"] = json!("2");
    n["agent_session_id"] = json!("new-session");
    let mut o = object();
    o["latest_metrics"]["nfs_quota_status"]["state"] = json!("stale");
    o["properties"]["ciderd_collector_states"] = json!([{"state":{"collector":"nfs.rquota"},"last_attempt":{"collection_id":"old","status":"timeout"},"acquisition":{"boot_id":"old","agent_generation":"1","agent_session_id":"old-session"}}]);
    assert_eq!(project(&n, &o, &[], 0)["state"], "stale");
}
