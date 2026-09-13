//! Configured NFS filesystem/UID quota read model, separate from APFS volume quotas.
use crate::{
    error::{ApiError, ApiResult},
    store::AppState,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
const FIELDS: &[&str] = &[
    "active",
    "used_bytes",
    "block_soft_limit_bytes",
    "block_hard_limit_bytes",
    "used_inodes",
    "inode_soft_limit",
    "inode_hard_limit",
    "block_grace_seconds_raw",
    "inode_grace_seconds_raw",
    "block_size_bytes",
];
fn metric(object: &Value, name: &str) -> Value {
    object["latest_metrics"]
        .get(format!("nfs_quota_{name}"))
        .or_else(|| object["latest_metrics"].get(format!("storage.nfs.quota.{name}")))
        .cloned()
        .unwrap_or_else(|| json!({"state":"unknown","value":null,"reason":"not_observed"}))
}
fn same_sample(m: &Value, status: &Value) -> bool {
    m["state"] == "ok"
        && m["ciderd"]["collection_id"]
            .as_str()
            .is_some_and(|id| status["ciderd"]["collection_id"] == id)
}
fn limit(m: &Value, current: bool) -> Value {
    let number = m["value"].as_str().and_then(|s| s.parse::<u128>().ok());
    if !current || number.is_none() {
        return json!({"state":"unknown","value":null,"unit":m["unit"]});
    }
    json!({"state":if number==Some(0){"unlimited"}else{"limited"},"value":m["value"],"unit":m["unit"],"basis":"available_server_response"})
}
fn grace(m: &Value, current: bool) -> Value {
    let raw = m["value"].as_str().and_then(|s| s.parse::<u32>().ok());
    if !current || raw.is_none() {
        return json!({"state":"unknown","seconds_raw":null,"seconds_signed":null,"deadline":null});
    }
    let raw = raw.unwrap();
    let seconds = raw as i32;
    let remaining = m["age_seconds"]
        .as_f64()
        .filter(|age| age.is_finite() && *age >= 0.0)
        .map(|age| f64::from(seconds) - age);
    let deadline = if raw == 0 {
        None
    } else {
        m["observed_at"]
            .as_str()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .and_then(|t| t.checked_add_signed(chrono::Duration::seconds(seconds.into())))
            .map(|t| t.to_rfc3339())
    };
    json!({"state":if raw==0{"not_active"}else if remaining.is_some_and(|n|n<=0.0){"expired"}else if remaining.is_some(){"running"}else{"unknown"},"seconds_raw":raw.to_string(),"seconds_signed":seconds.to_string(),"seconds_remaining_estimate":remaining,"deadline":deadline,"observed_at":m["observed_at"],"basis":"rquota_signed_32_bit_relative_seconds"})
}
/// Values retain acquisition metadata; every current conclusion requires one complete sample.
pub fn project(node: &Value, object: &Value, objects: &[Value], _now: i64) -> Value {
    let status = metric(object, "status");
    let props = &object["properties"];
    let fields: serde_json::Map<String, Value> = FIELDS
        .iter()
        .map(|name| (name.to_string(), metric(object, name)))
        .collect();
    let mut state = if status["state"] == "ok" {
        status["value"].as_str().unwrap_or("unknown")
    } else if status["state"] == "stale" {
        "stale"
    } else {
        "unknown"
    };
    let mut reason = if state == "unknown" {
        "not_observed"
    } else {
        "server_response"
    };
    let scope = props["ciderd_collector_states"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|s| s["state"]["collector"] == "nfs.rquota");
    let last = scope.map(|s| &s["last_attempt"]);
    let current_attempt = scope.is_some_and(|s| {
        ["boot_id", "agent_generation", "agent_session_id"]
            .iter()
            .all(|key| {
                node[*key]
                    .as_str()
                    .is_some_and(|value| !value.is_empty() && s["acquisition"][*key] == value)
            })
    });
    if let Some(attempt) = last.filter(|_| current_attempt) {
        if attempt["collection_id"] != status["ciderd"]["collection_id"] {
            if let Some(failure) = attempt["status"].as_str().filter(|s| {
                matches!(
                    *s,
                    "timeout" | "permission_denied" | "unsupported" | "parse_error" | "failed"
                )
            }) {
                state = if failure == "failed" {
                    "unavailable"
                } else {
                    failure
                };
                reason = "latest_worker_attempt_failed";
            }
        }
    }
    if node["availability"] != "online" || object["active"] != true {
        state = "stale";
        reason = "owner_or_resource_unavailable";
    }
    if state == "available" && !fields.values().all(|m| same_sample(m, &status)) {
        state = "partial";
        reason = "incomplete_or_mixed_observation";
    }
    let current = state == "available";
    let linked: Vec<Value> = objects
        .iter()
        .filter(|o| {
            o["node_id"] == object["node_id"]
                && o["active"] == true
                && o["properties"]["filesystem_type"] == "nfs"
                && props["nfs_source"]
                    .as_str()
                    .is_some_and(|s| o["properties"]["source"] == s)
        })
        .map(|o| o["object_id"].clone())
        .collect();
    let limits = json!({"block_soft":limit(&fields["block_soft_limit_bytes"],current),"block_hard":limit(&fields["block_hard_limit_bytes"],current),"inode_soft":limit(&fields["inode_soft_limit"],current),"inode_hard":limit(&fields["inode_hard_limit"],current)});
    let grace = json!({"block":grace(&fields["block_grace_seconds_raw"],current),"inode":grace(&fields["inode_grace_seconds_raw"],current)});
    json!({"quota_id":object["object_id"],"node_id":object["node_id"],"node_name":node["name"],"server":props["server"],"export_path":props["export_path"],"uid":props["uid"],"display_label":props["display_label"],"nfs_source":props["nfs_source"],"protocol":"rquota-v1-udp","scope":"server_filesystem_user","state":state,"reason":reason,"observed_at":status["observed_at"],"age_seconds":status["age_seconds"],"last_attempt":last,"active":if current{fields["active"]["value"].clone()}else{Value::Null},"query_identity":{"uid":props["query_identity_uid"],"gid":props["query_identity_gid"],"groups_truncated":props["query_groups_truncated"]},"metrics":fields,"limits":limits,"grace":grace,"linked_mount_ids":linked,"linkage_state":if linked.is_empty(){"configured_source_only"}else{"exact_configured_source"},"uncertainty":"Classic rquota trusts the configured server and uses 32-bit wire counts. Export paths can share one filesystem quota; rows must not be summed."})
}
pub async fn list(
    state: &AppState,
    query: &BTreeMap<String, String>,
    now: i64,
) -> ApiResult<(Value, Value)> {
    if let Some(uid) = query.get("uid") {
        if uid
            .parse::<u32>()
            .ok()
            .filter(|v| *v <= i32::MAX as u32 && v.to_string() == *uid)
            .is_none()
        {
            return Err(ApiError::field(
                "uid",
                "Expected a canonical UID in 0..2147483647",
            ));
        }
    }
    let (nodes, objects) = crate::read_api::current_objects(state).await?;
    let mut rows = Vec::new();
    for object in objects.iter().filter(|o| {
        o["active"] == true && o["properties"]["ciderd_resource_type"] == "nfs_user_quota"
    }) {
        if query
            .get("node_id")
            .is_some_and(|id| object["node_id"] != *id)
            || query
                .get("uid")
                .is_some_and(|uid| object["properties"]["uid"] != *uid)
        {
            continue;
        }
        if let Some(node) = nodes.iter().find(|n| n["node_id"] == object["node_id"]) {
            rows.push(project(node, object, &objects, now));
        }
    }
    rows.sort_by(|a, b| {
        a["node_id"]
            .as_str()
            .cmp(&b["node_id"].as_str())
            .then(a["server"].as_str().cmp(&b["server"].as_str()))
            .then(a["export_path"].as_str().cmp(&b["export_path"].as_str()))
            .then(a["uid"].as_str().cmp(&b["uid"].as_str()))
    });
    let meta = json!({"coverage":coverage(&nodes,&objects,&rows,query),"policy":{"collector":"nfs.rquota","protocol":"rquota-v1-udp","read_only":true,"zero_limit":"unlimited_only_on_available_response","aggregation":"never_sum_export_or_uid_rows","authentication":"viewer_or_admin"}});
    Ok((json!(rows), meta))
}
/// Configuration coverage cannot infer disabled collection from missing inventory.
pub fn coverage(
    nodes: &[Value],
    objects: &[Value],
    rows: &[Value],
    query: &BTreeMap<String, String>,
) -> Value {
    let selected: Vec<_> = nodes
        .iter()
        .filter(|n| query.get("node_id").is_none_or(|id| n["node_id"] == *id))
        .collect();
    let configuration: Vec<Option<bool>> = selected
        .iter()
        .map(|node| {
            if node["availability"] != "online" {
                return None;
            }
            let host = objects.iter().find(|o| {
                o["active"] == true && o["kind"] == "node" && o["node_id"] == node["node_id"]
            })?;
            let context = &host["properties"]["ciderd_acquisition"];
            if !["boot_id", "agent_generation", "agent_session_id"]
                .iter()
                .all(|key| {
                    node[*key]
                        .as_str()
                        .is_some_and(|value| !value.is_empty() && context[*key] == value)
                })
            {
                return None;
            }
            host["properties"]["diagnostics_configuration"]["nfs_quotas_enabled"].as_bool()
        })
        .collect();
    let configured = objects
        .iter()
        .filter(|o| {
            o["active"] == true
                && o["properties"]["ciderd_resource_type"] == "nfs_user_quota"
                && query.get("node_id").is_none_or(|id| o["node_id"] == *id)
        })
        .count();
    let available = rows.iter().filter(|r| r["state"] == "available").count();
    let known = configuration.iter().filter(|c| c.is_some()).count();
    let state = if configured == 0 {
        if !selected.is_empty() && configuration.iter().all(|c| *c == Some(false)) {
            "unconfigured"
        } else {
            "unknown"
        }
    } else if rows.is_empty() {
        "empty"
    } else if available == rows.len() && known == selected.len() {
        "available"
    } else {
        "partial"
    };
    json!({"configured_targets":configured,"returned_targets":rows.len(),"available_targets":available,"selected_nodes":selected.len(),"configuration_known_nodes":known,"configuration_unknown_nodes":selected.len()-known,"state":state})
}
