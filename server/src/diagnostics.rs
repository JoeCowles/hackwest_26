//! Passive collection readiness and filesystem-accessibility observations.
use serde_json::{Value, json};
use crate::{attention::Condition, error::{ApiError, ApiResult}, store::AppState};
use std::collections::BTreeMap;

fn same_context(context: &Value, node: &Value) -> bool {
    ["boot_id", "agent_generation", "agent_session_id"].iter().all(|key|
        context[*key].as_str().is_some_and(|s| !s.is_empty()) && context[*key] == node[*key])
}
fn at(value: &Value) -> Option<i64> {
    value.as_str().and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok()).map(|d| d.timestamp_millis())
}
fn collector_check(node: &Value, object: Option<&Value>, collector: &str, configured: Option<bool>, now: i64) -> Value {
    let mut out=json!({"collector":collector,"state":"unknown","reason":"configuration_not_reported",
        "observed_at":null,"age_seconds":null,"stale_after_seconds":null});
    if configured == Some(false) {
        out["state"]=json!("disabled"); out["reason"]=json!("disabled_in_collector_configuration"); return out;
    }
    let Some(object)=object else {out["reason"]=json!("source_association_unavailable");return out;};
    let candidate=object["properties"]["ciderd_collector_states"].as_array().into_iter().flatten()
        .find(|s| s["state"]["collector"]==collector);
    let Some(observation)=candidate else {
        if configured == Some(true) {out["state"]=json!("waiting");out["reason"]=json!("no_completed_attempt");}
        return out;
    };
    let attempt=&observation["last_attempt"];
    out["observed_at"]=attempt["finished_at"].clone();
    let observed=at(&attempt["finished_at"]);
    let received=at(&observation["acquisition"]["received_at"]);
    let elapsed=received.filter(|t|*t<=now).map(|t| (now-t) as f64/1000.0);
    let monotonic=observation["acquisition"]["age_at_receipt_seconds"].as_f64().filter(|age|age.is_finite() && *age>=0.0);
    let age=monotonic.or_else(||observed.map(|t|(now-t).max(0) as f64/1000.0)).zip(elapsed)
        .map(|(source,server)|if monotonic.is_some(){source+server}else{source.max(server)});
    let threshold=observation["state"]["stale_after_seconds"].as_u64().unwrap_or(90);
    out["age_seconds"]=json!(age);out["stale_after_seconds"]=json!(threshold);
    if node["availability"]!="online" || !same_context(&observation["acquisition"],node)
        || age.is_some_and(|a|a>threshold as f64) {
        out["state"]=json!("stale");out["reason"]=json!("source_or_owner_not_current");return out;
    }
    if observed.is_some_and(|t| t>now+300_000) {
        out["reason"]=json!("source_timestamp_in_future");return out;
    }
    if attempt["status"].is_string() && age.is_none() {
        out["reason"]=json!("source_age_unavailable");return out;
    }
    let (state, reason)=match attempt["status"].as_str() {
        Some("ok") if age.is_some() => ("ready","last_attempt_succeeded"),
        Some("partial") if age.is_some() => ("partial","some_fields_unavailable"),
        Some("unsupported") => ("unsupported","source_unsupported"),
        Some("permission_denied") => ("permission_denied","source_permission_denied"),
        Some("timeout") => ("timeout","source_timed_out"),
        Some("failed"|"parse_error"|"gone") => ("failed","source_acquisition_failed"),
        None => ("waiting","no_completed_attempt"),
        _ => ("unknown","source_state_unavailable"),
    };
    out["state"]=json!(state);out["reason"]=json!(reason);out
}

pub fn disk(node: &Value, id: &str, objects: &[Value], now: i64) -> Value {
    let owned=|o: &&Value|o["node_id"]==node["node_id"] && o["active"]==true;
    let host=objects.iter().filter(owned).find(|o|o["kind"]=="node" && same_context(&o["properties"]["ciderd_acquisition"],node));
    let enabled=host.and_then(|h|h["properties"]["diagnostics_configuration"]["smart_enabled"].as_bool());
    let physical=objects.iter().filter(owned).find(|o|o["object_id"]==id);
    let drivers:Vec<_>=objects.iter().filter(owned).filter(|o| o["properties"]["throughput_scope"]=="iokit_driver"
        && o["topology_state"]=="resolved" && same_context(&o["properties"]["ciderd_acquisition"],node)
        && o["physical_disk_ids"].as_array().is_some_and(|ids|ids.len()==1 && ids[0]==id)).collect();
    let driver=(drivers.len()==1).then(||drivers[0]);
    json!({"checks":[collector_check(node,physical,"smartctl",enabled,now),
        collector_check(node,driver,"iokit.block",driver.map(|_|true),now)],
        "assessment":"readiness_only","integrity":"unknown"})
}

pub fn filesystem(node: &Value, object: &Value, now: i64) -> Value {
    let m=&object["latest_metrics"];
    let dead=&m["storage.nfs.mount.dead"];
    let unresponsive=&m["storage.nfs.mount.not_responding"];
    let current=|v:&Value|node["availability"]=="online" && object["active"]==true && v["state"]=="ok" && v["value"].is_boolean();
    let (state,observation,reason)=if [dead,unresponsive].iter().any(|v|current(v)&&v["value"]==true) {
        ("warning","current",if current(dead)&&dead["value"]==true {"nfs_mount_reported_dead"} else {"nfs_mount_not_responding"})
    } else if current(dead)&&current(unresponsive) {
        ("no_current_warning","current","nfs_mount_response_flags_clear")
    } else if node["availability"]!="online" || [dead,unresponsive].iter().any(|v|v["state"]=="stale") {
        ("unknown","stale","observation_not_current")
    } else {("unknown","unavailable","mount_response_evidence_unavailable")};
    let observed_at=if current(dead) && dead["value"]==true {dead["observed_at"].clone()}
        else if current(unresponsive) && unresponsive["value"]==true {unresponsive["observed_at"].clone()}
        else if current(dead) && current(unresponsive) {
            match (at(&dead["observed_at"]),at(&unresponsive["observed_at"])) {
                (Some(a),Some(b))=>json!(crate::store::timestamp(a.min(b))),_=>Value::Null
            }
        } else {Value::Null};
    let capacity=collector_check(node,Some(object),"filesystem.capacity",None,now);
    json!({"accessibility":{"state":state,"observation_state":observation,"reason":reason,
        "observed_at":observed_at,"evidence":{"dead":dead,"not_responding":unresponsive}},
        "capacity_collection":capacity,"integrity":"unknown"})
}

fn filesystem_with_configuration(node: &Value, object: &Value, objects: &[Value], now: i64) -> Value {
    let mut out=filesystem(node,object,now);
    let configuration=objects.iter().find(|o|o["active"]==true && o["kind"]=="node" && o["node_id"]==node["node_id"]
        && same_context(&o["properties"]["ciderd_acquisition"],node))
        .and_then(|o|o["properties"]["diagnostics_configuration"].as_object());
    let configured=configuration.and_then(|c|c.get("nfs_path_refresh_enabled")).and_then(Value::as_bool)
        .map(|enabled|object["kind"]!="nfs_mount" || enabled);
    out["capacity_collection"]=collector_check(node,Some(object),"filesystem.capacity",configured,now);
    out
}

pub async fn list(app: &AppState, query: &BTreeMap<String,String>, now: i64) -> ApiResult<(Value,Value)> {
    for key in query.keys() {if !["node_id","limit"].contains(&key.as_str()) {return Err(ApiError::field(key,"Unknown query parameter"));}}
    if let Some(id)=query.get("node_id") {uuid::Uuid::parse_str(id).map_err(|_|ApiError::field("node_id","Expected a node UUID"))?;}
    let (nodes,objects)=crate::read_api::current_objects(app).await?;
    let mut rows=Vec::new();
    for node in nodes.iter().filter(|n|query.get("node_id").is_none_or(|id|n["node_id"]==*id)) {
        for object in objects.iter().filter(|o|o["active"]==true && o["node_id"]==node["node_id"]) {
            let kind=if object["properties"]["ciderd_resource_type"]=="physical_device" {"disk"}
                else if matches!(object["kind"].as_str(),Some("mount"|"nfs_mount"|"apfs_volume")) {"filesystem"} else {continue;};
            let details=if kind=="disk" {disk(node,object["object_id"].as_str().unwrap_or(""),&objects,now)} else {filesystem_with_configuration(node,object,&objects,now)};
            rows.push(json!({"node_id":node["node_id"],"node_name":node["name"],"object_id":object["object_id"],"kind":kind,
                "label":object["properties"]["mount_point"].as_str().or(object["properties"]["bsd_name"].as_str()).unwrap_or("Storage object"),"diagnostics":details}));
            if rows.len()>10000 {return Err(ApiError::new(axum::http::StatusCode::SERVICE_UNAVAILABLE,"source_limit","Narrow diagnostics to a node"));}
        }
    }
    Ok((json!(rows),json!({"assessment":"readiness_and_accessibility_only","filesystem_integrity":"unknown"})))
}

pub async fn attention_conditions(app: &AppState, now: i64) -> ApiResult<Vec<Condition>> {
    let (nodes,objects)=crate::read_api::current_objects(app).await?;
    let mut out=Vec::new();
    for object in objects.iter().filter(|o|o["active"]==true && matches!(o["kind"].as_str(),Some("mount"|"nfs_mount"))) {
        let Some(node)=nodes.iter().find(|n|n["node_id"]==object["node_id"]) else {continue;};
        let observed=filesystem_with_configuration(node,object,&objects,now);
        for (key,value,label) in [("accessibility",&observed["accessibility"],"NFS mount accessibility concern"),
            ("capacity_collection",&observed["capacity_collection"],"Filesystem capacity observation failed")] {
            let current_warning=if key=="accessibility" {value["state"]=="warning"} else {matches!(value["state"].as_str(),Some("failed"|"timeout"))};
            let clear=matches!(value["state"].as_str(),Some("ready"|"no_current_warning"));
            out.push(Condition {key:format!("filesystem:{key}:{}",object["object_id"].as_str().unwrap_or("")),kind:"filesystem".into(),
                node_id:node["node_id"].as_str().unwrap_or("").into(),object_id:object["object_id"].as_str().map(str::to_owned),
                status:if current_warning {"open"} else if clear {"resolved"} else {"interrupted"}.into(),severity:"warning".into(),
                observation_state:if current_warning||clear {"current"} else if value["state"]=="stale"||value["observation_state"]=="stale" {"stale"} else {"unavailable"}.into(),
                summary:label.into(),evidence:value.clone()});
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn context() -> Value { json!({"boot_id":"boot","agent_generation":"1","agent_session_id":"session"}) }
    fn node() -> Value { let mut n=context(); n["node_id"]=json!("node"); n["availability"]=json!("online"); n }
    fn host(enabled: bool) -> Value {json!({"node_id":"node","kind":"node","active":true,"properties":{"ciderd_acquisition":context(),"diagnostics_configuration":{"smart_enabled":enabled}}})}
    fn physical() -> Value {json!({"node_id":"node","object_id":"disk","active":true,"topology_state":"resolved","properties":{"ciderd_resource_type":"physical_device","ciderd_acquisition":context()}})}
    #[test]
    fn explicit_disabled_smart_is_distinct_from_missing_configuration() {
        let objects=vec![host(false),physical()];
        let out=disk(&node(),"disk",&objects,1000);
        assert_eq!(out["checks"][0]["state"],"disabled");
        assert_eq!(disk(&node(),"disk",&[physical()],1000)["checks"][0]["state"],"unknown");
    }
    #[test]
    fn configuration_from_an_old_session_cannot_claim_current_readiness() {
        let mut old=host(false); old["properties"]["ciderd_acquisition"]["agent_session_id"]=json!("old");
        assert_eq!(disk(&node(),"disk",&[old,physical()],1000)["checks"][0]["state"],"unknown");
    }
    #[test]
    fn no_smart_attempt_while_enabled_is_waiting_not_healthy() {
        assert_eq!(disk(&node(),"disk",&[host(true),physical()],1000)["checks"][0]["state"],"waiting");
    }
    #[test]
    fn current_nfs_failure_is_evidence_but_stale_failure_is_dated() {
        let mut object=json!({"node_id":"node","object_id":"mount","active":true,"kind":"nfs_mount","latest_metrics":{
            "storage.nfs.mount.dead":{"value":true,"state":"ok","age_seconds":1,"observed_at":"2026-09-13T12:00:00Z"},
            "storage.nfs.mount.not_responding":{"value":false,"state":"ok","age_seconds":1,"observed_at":"2026-09-13T12:00:00Z"}}});
        let current=filesystem(&node(),&object,1000);
        assert_eq!(current["accessibility"]["state"],"warning");
        object["latest_metrics"]["storage.nfs.mount.dead"]["state"]=json!("stale");
        let dated=filesystem(&node(),&object,1000);
        assert_eq!(dated["accessibility"]["state"],"unknown");
        assert_eq!(dated["accessibility"]["observation_state"],"stale");
        assert_eq!(dated["integrity"],"unknown");
    }
    #[test]
    fn read_only_mount_does_not_imply_corruption() {
        let o=json!({"active":true,"kind":"mount","latest_metrics":{"storage.mount.read_only":{"value":true,"state":"ok"}}});
        let out=filesystem(&node(),&o,1000);
        assert_eq!(out["integrity"],"unknown");
        assert_eq!(out["accessibility"]["state"],"unknown");
    }
    #[test]
    fn accessibility_date_belongs_to_the_triggering_observation() {
        let object=json!({"active":true,"latest_metrics":{
            "storage.nfs.mount.not_responding":{"value":true,"state":"ok","observed_at":"2026-09-13T12:00:00Z"}}});
        assert_eq!(filesystem(&node(),&object,1000)["accessibility"]["observed_at"],"2026-09-13T12:00:00Z");
    }
    #[test]
    fn failed_attempt_without_acquisition_age_cannot_open_a_current_concern() {
        let o=json!({"properties":{"ciderd_collector_states":[{"state":{"collector":"filesystem.capacity","stale_after_seconds":90},
            "last_attempt":{"status":"failed"},"acquisition":{"boot_id":"boot","agent_generation":"1","agent_session_id":"session"}}]}});
        let value=collector_check(&node(),Some(&o),"filesystem.capacity",Some(true),1000);
        assert_eq!(value["state"],"unknown");
    }
    #[test]
    fn collection_age_cannot_be_refreshed_by_delayed_delivery_or_clock_skew() {
        let o=json!({"properties":{"ciderd_collector_states":[{"state":{"collector":"filesystem.capacity","stale_after_seconds":90},
            "last_attempt":{"status":"ok","finished_at":"1970-01-01T00:01:00Z"},
            "acquisition":{"boot_id":"boot","agent_generation":"1","agent_session_id":"session","received_at":"1970-01-01T00:00:01Z","age_at_receipt_seconds":120}}]}});
        let value=collector_check(&node(),Some(&o),"filesystem.capacity",Some(true),1000);
        assert_eq!(value["state"],"stale");
    }
    #[test]
    fn nfs_capacity_disabled_configuration_and_legacy_unknown_are_distinct() {
        let object=json!({"active":true,"kind":"nfs_mount"});
        let mut config=host(false);config["properties"]["diagnostics_configuration"]["nfs_path_refresh_enabled"]=json!(false);
        assert_eq!(filesystem_with_configuration(&node(),&object,&[config],1000)["capacity_collection"]["state"],"disabled");
        assert_eq!(filesystem_with_configuration(&node(),&object,&[],1000)["capacity_collection"]["state"],"unknown");
    }
}
