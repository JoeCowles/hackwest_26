//! Pure physical-disk ownership projection from one committed read snapshot.
use serde_json::{Value, json};

pub struct DiskIndex {
    pub disks: Vec<Value>,
    pub objects: Vec<Value>,
    pub topology_revision: String,
    pub disk_inventory: Value,
}

use crate::{
    read_api::{capacity, rate, unknown},
    store,
};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

fn context(object: &Value) -> &Value {
    &object["properties"]["ciderd_acquisition"]
}
fn identity(context: &Value) -> Value {
    json!([
        context["boot_id"],
        context["agent_generation"],
        context["agent_session_id"]
    ])
}
fn known(context: &Value) -> bool {
    context["schema_version"] == 1
        && ["boot_id", "agent_generation", "agent_session_id"]
            .iter()
            .all(|k| context[*k].as_str().is_some_and(|v| !v.is_empty()))
}
fn current(context: &Value, node: &Value) -> bool {
    known(context)
        && context["boot_id"] == node["boot_id"]
        && context["agent_session_id"] == node["agent_session_id"]
        && context["agent_generation"] == node["agent_generation"]
}
fn source_identity_matches(object: &Value) -> bool {
    let p = &object["properties"];
    let c = context(object);
    p["observed_agent_session"]
        .as_str()
        .is_none_or(|s| c["agent_session_id"] == s)
        && p["observed_boot_id"]
            .as_str()
            .is_none_or(|s| c["boot_id"] == s)
}
fn physical(object: &Value) -> bool {
    object["properties"]["ciderd_resource_type"] == "physical_device"
        && object["properties"]["source"] == "diskutil.list.physical"
}
fn resource_type(object: &Value) -> &str {
    object["properties"]["ciderd_resource_type"]
        .as_str()
        .unwrap_or("")
}
fn local_mount(object: &Value) -> bool {
    let p = &object["properties"];
    resource_type(object) == "mount"
        && p["local"] != false
        && object["kind"] != "nfs_mount"
        && !matches!(
            p["filesystem_type"].as_str(),
            Some(
                "nfs"
                    | "nfs4"
                    | "smbfs"
                    | "cifs"
                    | "webdav"
                    | "autofs"
                    | "devfs"
                    | "tmpfs"
                    | "ramfs"
            )
        )
}
fn valid_kind(kind: &str, from: &Value, to: &Value, attributes: &Value) -> bool {
    let f = resource_type(from);
    let t = resource_type(to);
    match kind {
        "contains" => {
            (physical(from) && t == "media")
                || (matches!(f, "apfs_container" | "storage_pool") && t == "filesystem")
        }
        "backed_by" => {
            (matches!(
                f,
                "apfs_container" | "storage_pool" | "filesystem" | "media"
            ) || (local_mount(from)
                && from["properties"]["filesystem_type"] != "apfs"
                && attributes["source"] == "derived.storage"
                && to["properties"]["source"] == "diskutil.list.physical"
                && to["properties"]["bsd_name"]
                    .as_str()
                    .is_some_and(|bsd| from["properties"]["source"] == format!("/dev/{bsd}"))))
                && (t == "media" || physical(to))
        }
        "attached_to" => {
            f == "controller"
                && from["properties"]["scope"] == "driver"
                && physical(to)
                && attributes["source"] == "derived.storage"
        }
        "mounts" => local_mount(from) && t == "filesystem",
        "snapshot_of" => f == "snapshot" && matches!(t, "filesystem" | "snapshot"),
        _ => false,
    }
}
fn attempt_state(node: &Value, objects: &[Value], roots: &[String]) -> Value {
    let native = objects
        .iter()
        .any(|o| o["properties"]["ciderd_resource_type"].is_string());
    let attempt = objects
        .iter()
        .flat_map(|o| {
            o["properties"]["ciderd_collector_states"]
                .as_array()
                .into_iter()
                .flatten()
        })
        .find(|s| s["state"]["collector"] == "diskutil.inventory");
    let mut state = if native { "pending" } else { "unsupported" };
    let mut reason = if native {
        "physical_inventory_pending"
    } else {
        "physical_inventory_unsupported"
    };
    if let Some(attempt) = attempt {
        let status = attempt["last_attempt"]["status"].as_str();
        let at = attempt["last_attempt"]["finished_at"]
            .as_str()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.timestamp_millis());
        let expired = at
            .zip(node["_snapshot_time"].as_i64())
            .is_some_and(|(at, now)| {
                now - at
                    > attempt["state"]["stale_after_seconds"]
                        .as_i64()
                        .unwrap_or(90)
                        * 1000
            });
        if status == Some("ok") {
            state = "ok";
            reason = "";
        } else if status.is_some() {
            state = if roots.is_empty() {
                "unavailable"
            } else {
                "stale"
            };
            reason = "physical_inventory_failed";
        }
        if status.is_some() && (expired || !current(&attempt["acquisition"], node)) {
            state = "stale";
            reason = "physical_inventory_context_stale";
        }
    } else if !roots.is_empty() {
        state = "ok";
        reason = "";
    }
    if !roots.is_empty()
        && objects
            .iter()
            .filter(|o| physical(o))
            .any(|o| !current(context(o), node) || !source_identity_matches(o))
    {
        state = "stale";
        reason = "physical_inventory_context_stale";
    }
    if node["availability"] != "online" && !matches!(state, "pending" | "unsupported") {
        state = "stale";
        reason = "owner_unavailable";
    }
    json!({"state":state,"reason_codes":if reason.is_empty(){vec![]}else{vec![reason]},"physical_disk_count":roots.len(),"observed_at":attempt.map(|s|s["last_attempt"]["finished_at"].clone()).unwrap_or(Value::Null)})
}

pub fn build_disk_index(node: &Value, objects: &[Value]) -> DiskIndex {
    let mut objects = objects.to_vec();
    let owned: BTreeMap<String, usize> = objects
        .iter()
        .enumerate()
        .filter(|(_, o)| o["node_id"] == node["node_id"] && o["active"] == true)
        .filter_map(|(i, o)| Some((o["object_id"].as_str()?.to_owned(), i)))
        .collect();
    let resources: BTreeMap<String, String> = owned
        .iter()
        .filter_map(|(id, i)| {
            Some((
                objects[*i]["properties"]["ciderd_resource_id"]
                    .as_str()?
                    .to_owned(),
                id.clone(),
            ))
        })
        .collect();
    let roots: Vec<String> = owned
        .iter()
        .filter(|(_, i)| physical(&objects[**i]))
        .map(|(id, _)| id.clone())
        .collect();
    let inventory = attempt_state(node, &objects, &roots);
    let mut edges = BTreeMap::new();
    let mut reasons: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (id, i) in &owned {
        let object = &objects[*i];
        if !known(context(object)) {
            reasons
                .entry(id.clone())
                .or_default()
                .insert("acquisition_context_missing".into());
        }
        for raw in object["properties"]["ciderd_relationships"]
            .as_array()
            .into_iter()
            .flatten()
        {
            let rid = raw["relationship_id"].as_str().unwrap_or("");
            let from = raw["from_resource_id"]
                .as_str()
                .and_then(|r| resources.get(r));
            let to = raw["to_resource_id"]
                .as_str()
                .and_then(|r| resources.get(r));
            let (Some(from), Some(to)) = (from, to) else {
                reasons
                    .entry(id.clone())
                    .or_default()
                    .insert("relationship_endpoint_missing".into());
                continue;
            };
            let f = &objects[owned[from]];
            let t = &objects[owned[to]];
            let kind = raw["relation"].as_str().unwrap_or("");
            if !valid_kind(kind, f, t, &raw["attributes"]) {
                continue;
            }
            let c = &raw["acquisition"];
            let coherent = known(c)
                && known(context(f))
                && known(context(t))
                && identity(c) == identity(context(f))
                && identity(c) == identity(context(t))
                && source_identity_matches(f)
                && source_identity_matches(t)
                && raw["attributes"]["boot_id"]
                    .as_str()
                    .is_none_or(|v| c["boot_id"] == v)
                && raw["attributes"]["agent_session_id"]
                    .as_str()
                    .is_none_or(|v| c["agent_session_id"] == v);
            if !coherent {
                for id in [from, to] {
                    reasons
                        .entry(id.clone())
                        .or_default()
                        .insert("relationship_context_mismatch".into());
                }
                continue;
            }
            let stale = !current(c, node)
                || raw["attributes"]["state"] == "stale"
                || [f, t]
                    .iter()
                    .any(|o| o["properties"]["association_state"] == "stale");
            let edge = json!({"relationship_id":Uuid::new_v5(&Uuid::parse_str(node["node_id"].as_str().unwrap_or("")).unwrap_or(Uuid::NAMESPACE_OID),format!("ciderd:relationship:{rid}").as_bytes()).to_string(),
                "kind":kind,"from_object_id":from,"to_object_id":to,"attributes":raw["attributes"],"acquisition":c,"state":if stale{"stale"}else{"resolved"}});
            edges.insert(rid.to_owned(), edge);
        }
    }
    // Directed child -> backing parents. Host fallback parent_ids never participate.
    let mut parents: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for edge in edges.values() {
        let from = edge["from_object_id"].as_str().unwrap();
        let to = edge["to_object_id"].as_str().unwrap();
        let (child, parent) = if edge["kind"] == "contains" {
            (to, from)
        } else {
            (from, to)
        };
        parents
            .entry(child.into())
            .or_default()
            .insert(parent.into());
    }
    let root_set: BTreeSet<String> = roots.iter().cloned().collect();
    let mut membership: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for id in owned.keys() {
        let mut seen = BTreeSet::new();
        let mut todo = vec![id.clone()];
        let mut disks = BTreeSet::new();
        while let Some(at) = todo.pop() {
            if !seen.insert(at.clone()) {
                continue;
            }
            if root_set.contains(&at) {
                disks.insert(at);
                continue;
            }
            if seen.len() > 4096 {
                reasons
                    .entry(id.clone())
                    .or_default()
                    .insert("topology_traversal_limit".into());
                break;
            }
            if let Some(next) = parents.get(&at) {
                todo.extend(next.iter().cloned());
            }
        }
        membership.insert(id.clone(), disks);
    }
    // Fail closed for cycles: a reachable root does not make a cyclic ownership path valid.
    for id in owned.keys() {
        let mut todo = parents
            .get(id)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .collect::<Vec<_>>();
        let mut seen = BTreeSet::new();
        while let Some(at) = todo.pop() {
            if &at == id {
                reasons
                    .entry(id.clone())
                    .or_default()
                    .insert("topology_cycle".into());
                membership.insert(id.clone(), BTreeSet::new());
                break;
            }
            if !seen.insert(at.clone()) {
                continue;
            }
            if let Some(next) = parents.get(&at) {
                todo.extend(next.iter().cloned());
            }
        }
    }
    let cyclic: BTreeSet<String> = reasons
        .iter()
        .filter(|(_, codes)| codes.contains("topology_cycle"))
        .map(|(id, _)| id.clone())
        .collect();
    if !cyclic.is_empty() {
        for id in owned.keys() {
            let mut todo = vec![id.clone()];
            let mut seen = BTreeSet::new();
            while let Some(at) = todo.pop() {
                if cyclic.contains(&at) {
                    membership.insert(id.clone(), BTreeSet::new());
                    reasons
                        .entry(id.clone())
                        .or_default()
                        .insert("topology_cycle".into());
                    break;
                }
                if !seen.insert(at.clone()) {
                    continue;
                }
                if let Some(next) = parents.get(&at) {
                    todo.extend(next.iter().cloned());
                }
            }
        }
    }
    let structural_objects: Vec<_> = owned
        .iter()
        .map(|(id, i)| {
            let p = &objects[*i]["properties"];
            json!([
                id,
                resource_type(&objects[*i]),
                identity(context(&objects[*i])),
                p["source"],
                p["bsd_name"],
                p["reported_uuid"],
                p["mount_generation"],
                p["size_bytes"]
            ])
        })
        .collect();
    let structural_edges: Vec<_> = edges
        .values()
        .map(|e| {
            json!([
                e["relationship_id"],
                e["kind"],
                e["from_object_id"],
                e["to_object_id"],
                identity(&e["acquisition"]),
                e["attributes"]["mapping_method"],
                e["attributes"]["evidence_resource_ids"],
                e["attributes"]["evidence_generations"]
            ])
        })
        .collect();
    let topology_revision = store::fingerprint(
        json!([
            node["node_id"],
            node["boot_id"],
            node["agent_generation"],
            node["agent_session_id"],
            structural_objects,
            structural_edges
        ])
        .to_string()
        .as_bytes(),
    );
    for (id, i) in &owned {
        let reasons = reasons.get(id).cloned().unwrap_or_default();
        let stale = !current(context(&objects[*i]), node)
            || objects[*i]["properties"]["association_state"] == "stale";
        let state = if stale {
            "stale"
        } else if objects[*i]["properties"]["association_state"] == "ambiguous" {
            "ambiguous"
        } else if membership[id].is_empty() {
            "unresolved"
        } else if !reasons.is_empty() {
            "partial"
        } else {
            "resolved"
        };
        objects[*i]["relationships"] = json!(
            edges
                .values()
                .filter(|e| e["from_object_id"] == *id || e["to_object_id"] == *id)
                .collect::<Vec<_>>()
        );
        objects[*i]["physical_disk_ids"] = json!(membership[id]);
        objects[*i]["topology_state"] = json!(state);
        objects[*i]["topology_reason_codes"] = json!(reasons);
    }
    // Even retired/foreign/legacy rows keep an explicit normalized shape.
    for object in &mut objects {
        if object.get("relationships").is_none() {
            object["relationships"] = json!([]);
            object["physical_disk_ids"] = json!([]);
            object["topology_state"] = json!("unresolved");
            object["topology_reason_codes"] = json!(["inactive_or_foreign_object"]);
        }
    }
    let mut disks = Vec::new();
    for id in roots {
        let disk = &objects[owned[&id]];
        let members: Vec<&Value> = owned
            .iter()
            .filter(|(other, _)| *other != &id && membership[*other].contains(&id))
            .map(|(_, i)| &objects[*i])
            .collect();
        let pools: Vec<&Value> = members
            .iter()
            .copied()
            .filter(|o| matches!(resource_type(o), "apfs_container" | "storage_pool"))
            .collect();
        let shared: Vec<String> = pools
            .iter()
            .filter(|p| membership[p["object_id"].as_str().unwrap()].len() > 1)
            .filter_map(|p| p["object_id"].as_str().map(str::to_owned))
            .collect();
        let exclusive: Vec<&Value> = pools
            .iter()
            .copied()
            .filter(|p| {
                membership[p["object_id"].as_str().unwrap()].len() == 1
                    && p["topology_reason_codes"]
                        .as_array()
                        .is_some_and(|r| r.is_empty())
            })
            .collect();
        // One observer per confirmed filesystem, including independent partitions
        // on disks that also contain APFS pools. Never sum repeated mount aliases.
        let mut observers = BTreeMap::new();
        for member in &members {
            if !local_mount(member)
                || member["properties"]["filesystem_type"] == "apfs"
                || member["topology_state"] != "resolved"
            {
                continue;
            }
            let targets: BTreeSet<&str> = edges
                .values()
                .filter(|edge| {
                    edge["from_object_id"] == member["object_id"]
                        && matches!(edge["kind"].as_str(), Some("mounts" | "backed_by"))
                })
                .filter_map(|edge| edge["to_object_id"].as_str())
                .collect();
            if targets.len() != 1 {
                continue;
            }
            let key = targets
                .first()
                .expect("one confirmed filesystem")
                .to_string();
            crate::read_api::select_capacity_observer(&mut observers, key, member);
        }
        let mut capacity_objects = exclusive.clone();
        capacity_objects.extend(observers.into_values());
        let mut cap = capacity(&capacity_objects);
        cap["attribution"] = json!(if !shared.is_empty() {
            "shared"
        } else if !capacity_objects.is_empty() {
            "exclusive"
        } else {
            "unresolved"
        });
        let driver_edges: Vec<&Value> = edges
            .values()
            .filter(|e| e["kind"] == "attached_to" && e["to_object_id"] == id)
            .collect();
        let fanout = driver_edges.iter().any(|edge| {
            edges
                .values()
                .filter(|other| {
                    other["kind"] == "attached_to"
                        && other["from_object_id"] == edge["from_object_id"]
                })
                .map(|other| other["to_object_id"].as_str().unwrap_or(""))
                .collect::<BTreeSet<_>>()
                .len()
                > 1
        });
        let ambiguous = disk["properties"]["association_state"] == "ambiguous"
            || driver_edges.len() > 1
            || fanout;
        let edge = if driver_edges.len() == 1 {
            Some(driver_edges[0])
        } else {
            None
        };
        let driver = edge.map(|e| &objects[owned[e["from_object_id"].as_str().unwrap()]]);
        let state = if ambiguous {
            "ambiguous"
        } else if edge.is_none() {
            "unresolved"
        } else if inventory["state"] != "ok"
            || edge.unwrap()["state"] == "stale"
            || node["availability"] != "online"
        {
            "stale"
        } else {
            "resolved"
        };
        let reason = match state {
            "ambiguous" => "multiple_driver_candidates",
            "stale" => "association_evidence_stale",
            _ => "driver_association_unresolved",
        };
        let read = if state == "resolved" {
            rate(driver.unwrap(), "device_read_bytes_total")
        } else {
            let mut m = unknown("gauge", "bytes/second");
            m["reason"] = json!(reason);
            m
        };
        let write = if state == "resolved" {
            rate(driver.unwrap(), "device_write_bytes_total")
        } else {
            let mut m = unknown("gauge", "bytes/second");
            m["reason"] = json!(reason);
            m
        };
        let continuity = if state == "resolved" {
            Some(store::fingerprint(json!([node["node_id"],id,identity(context(disk)),driver.unwrap()["object_id"],edge.unwrap()["relationship_id"],edge.unwrap()["attributes"]["mapping_method"],edge.unwrap()["attributes"]["evidence_resource_ids"],driver.unwrap()["latest_metrics"]["device_read_bytes_total"]["ciderd"]["source_metric"]["counter_epoch"],driver.unwrap()["latest_metrics"]["device_write_bytes_total"]["ciderd"]["source_metric"]["counter_epoch"],driver.unwrap()["latest_metrics"]["device_read_bytes_total"]["ciderd"]["clock_id"]]).to_string().as_bytes()))
        } else {
            None
        };
        let mut size = unknown("gauge", "bytes");
        let c = context(disk);
        let raw = &disk["properties"]["size_bytes"];
        let exact = raw
            .as_str()
            .and_then(|s| s.parse::<u128>().ok())
            .or_else(|| {
                raw.is_number()
                    .then(|| raw.to_string())
                    .and_then(|text| text.parse::<u128>().ok())
            });
        size["value"] = json!(exact.map(|n| n.to_string()));
        size["state"] = json!(if exact.is_none() || !known(c) {
            "unknown"
        } else if inventory["state"] != "ok" || !current(c, node) {
            "stale"
        } else {
            "ok"
        });
        for key in ["observed_at", "received_at", "boot_id"] {
            size[key] = c[key].clone();
        }
        size["source"] = disk["properties"]["source"].clone();
        size["inventory_generation"] = disk["inventory_generation"].clone();
        let mut disk_reasons = disk["topology_reason_codes"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        if state != "resolved" {
            disk_reasons.push(json!(reason));
        }
        if members.iter().any(|m| m["topology_state"] != "resolved") {
            disk_reasons.push(json!("member_topology_incomplete"));
        }
        let topology_state = if inventory["state"] == "stale" || state == "stale" {
            "stale"
        } else if ambiguous {
            "ambiguous"
        } else if state == "resolved" && disk_reasons.is_empty() {
            "resolved"
        } else if members.is_empty() {
            "unresolved"
        } else {
            "partial"
        };
        disks.push(json!({"object_id":id,"node_id":node["node_id"],"boot_id":c["boot_id"],"inventory_generation":node["inventory_generation"],"active":disk["active"],"availability":node["availability"],
            "bsd_name":disk["properties"]["bsd_name"],"label":disk["properties"].get("model").or_else(||disk["properties"].get("bsd_name")),"hardware_size_bytes":size,
            "topology":{"state":topology_state,"reason_codes":disk_reasons,"member_count":members.len(),"shared_pool_ids":shared},
            "io":{"linkage_state":state,"driver_object_id":if state=="resolved"{driver.map(|d|d["object_id"].clone())}else{None},"read_bytes_per_second":read,"write_bytes_per_second":write,"source_object_ids":if state=="resolved"{json!([driver.unwrap()["object_id"]])}else{json!([])},"continuity_key":continuity},
            "capacity":cap,"inventory_url":format!("/api/v1/nodes/{}/inventory?disk_id={id}",node["node_id"].as_str().unwrap_or(""))}));
    }
    DiskIndex {
        disks,
        objects,
        topology_revision,
        disk_inventory: inventory,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;
    fn id(name: &str) -> String {
        Uuid::new_v5(&Uuid::NAMESPACE_OID, name.as_bytes()).to_string()
    }
    fn node() -> Value {
        json!({"node_id":id("node"),"boot_id":"boot","agent_generation":"1","agent_session_id":"session","inventory_generation":"4","availability":"online","_snapshot_time":0})
    }
    fn context() -> Value {
        json!({"schema_version":1,"boot_id":"boot","agent_generation":"1","agent_session_id":"session","observed_at":"2026-09-12T12:00:00Z","received_at":"2026-09-12T12:00:01Z"})
    }
    fn object(name: &str, kind: &str, resource: &str, source: &str) -> Value {
        json!({"object_id":id(name),"node_id":id("node"),"kind":kind,"active":true,"inventory_generation":"4","parent_ids":[],
            "properties":{"ciderd_resource_id":name,"ciderd_resource_type":resource,"source":source,"bsd_name":name,"size_bytes":"18446744073709552615","ciderd_acquisition":context(),"ciderd_relationships":[]},"latest_metrics":{}})
    }
    fn link(objects: &mut [Value], from: usize, to: usize, kind: &str) {
        let name = format!("{from}-{to}-{kind}");
        let edge = json!({"relationship_id":name,"relation":kind,"from_resource_id":objects[from]["properties"]["ciderd_resource_id"],"to_resource_id":objects[to]["properties"]["ciderd_resource_id"],"acquisition":context(),"attributes":{"source":"derived.storage","state":"resolved","mapping_method":"test"}});
        objects[from]["properties"]["ciderd_relationships"]
            .as_array_mut()
            .unwrap()
            .push(edge.clone());
        objects[to]["properties"]["ciderd_relationships"]
            .as_array_mut()
            .unwrap()
            .push(edge);
    }
    fn fixture() -> Vec<Value> {
        let mut objects = vec![
            object(
                "disk0",
                "device",
                "physical_device",
                "diskutil.list.physical",
            ),
            object("store0", "partition", "media", "diskutil.list.physical"),
            object(
                "pool",
                "apfs_container",
                "apfs_container",
                "diskutil.apfs.list",
            ),
            object("volume", "apfs_volume", "filesystem", "diskutil.apfs.list"),
            object("driver", "device", "controller", "IOBlockStorageDriver"),
        ];
        objects[4]["properties"]["scope"] = json!("driver");
        objects[4]["properties"]["throughput_scope"] = json!("iokit_driver");
        objects[4]["latest_metrics"]["device_read_bytes_total"] = json!({"value":"18446744073709552615","state":"ok","kind":"counter","unit":"bytes","derivation_state":"ok","derived_rate_per_second":0.001,"boot_id":"boot","ciderd":{"source_metric":{"counter_epoch":"epoch"},"clock_id":"clock"}});
        objects[2]["latest_metrics"]["capacity_bytes"] =
            json!({"value":"1000","state":"ok","unit":"bytes"});
        objects[2]["latest_metrics"]["used_bytes"] =
            json!({"value":"500","state":"ok","unit":"bytes"});
        link(&mut objects, 0, 1, "contains");
        link(&mut objects, 2, 1, "backed_by");
        link(&mut objects, 2, 3, "contains");
        link(&mut objects, 4, 0, "attached_to");
        objects
    }
    #[test]
    fn one_disk_projects_exact_size_shared_objects_and_source_rate() {
        let objects = fixture();
        let out = build_disk_index(&node(), &objects);
        assert_eq!(out.disks.len(), 1);
        let d = &out.disks[0];
        assert_eq!(d["hardware_size_bytes"]["value"], "18446744073709552615");
        assert_eq!(d["io"]["read_bytes_per_second"]["value"], 0.001);
        assert_eq!(d["io"]["driver_object_id"], id("driver"));
        assert_eq!(d["capacity"]["attribution"], "exclusive");
        assert_eq!(d["capacity"]["capacity_bytes"]["value"], "1000");
        assert_eq!(out.objects[3]["physical_disk_ids"], json!([id("disk0")]));
        assert_eq!(
            out.objects[3]["relationships"][0]["from_object_id"],
            id("pool")
        );
    }
    #[test]
    fn shared_pool_is_one_object_and_never_exclusive_capacity() {
        let mut objects = fixture();
        objects.push(object(
            "disk1",
            "device",
            "physical_device",
            "diskutil.list.physical",
        ));
        objects.push(object(
            "store1",
            "partition",
            "media",
            "diskutil.list.physical",
        ));
        link(&mut objects, 5, 6, "contains");
        link(&mut objects, 2, 6, "backed_by");
        let out = build_disk_index(&node(), &objects);
        assert_eq!(out.disks.len(), 2);
        assert_eq!(out.objects.len(), 7);
        assert_eq!(
            out.objects[2]["physical_disk_ids"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        for d in &out.disks {
            assert_eq!(d["capacity"]["attribution"], "shared");
            assert!(d["capacity"]["capacity_bytes"]["value"].is_null());
            assert_eq!(d["topology"]["shared_pool_ids"], json!([id("pool")]));
        }
    }
    #[test]
    fn ambiguous_stale_missing_context_and_cross_node_edges_never_authorize_rates() {
        let mut objects = fixture();
        objects.push(object(
            "driver2",
            "device",
            "controller",
            "IOBlockStorageDriver",
        ));
        objects[5]["properties"]["scope"] = json!("driver");
        link(&mut objects, 5, 0, "attached_to");
        let out = build_disk_index(&node(), &objects);
        assert_eq!(out.disks[0]["io"]["linkage_state"], "ambiguous");
        assert!(out.disks[0]["io"]["read_bytes_per_second"]["value"].is_null());
        assert_eq!(out.disks[0]["io"]["source_object_ids"], json!([]));
        for mode in ["missing", "boot", "node", "stale"] {
            let mut objects = fixture();
            match mode {
                "missing" => objects[4]["properties"]["ciderd_acquisition"] = Value::Null,
                "boot" => objects[4]["properties"]["ciderd_acquisition"]["boot_id"] = json!("old"),
                "node" => objects[4]["node_id"] = json!(id("elsewhere")),
                _ => objects[4]["properties"]["association_state"] = json!("stale"),
            }
            let out = build_disk_index(&node(), &objects);
            assert!(
                out.disks[0]["io"]["read_bytes_per_second"]["value"].is_null(),
                "{mode}"
            );
        }
    }
    #[test]
    fn topology_digest_ignores_values_and_time_but_changes_on_session_and_structure() {
        let mut objects = fixture();
        let first = build_disk_index(&node(), &objects);
        objects[4]["latest_metrics"]["device_read_bytes_total"]["value"] = json!("8");
        objects[0]["properties"]["ciderd_acquisition"]["received_at"] = json!("later");
        let next = build_disk_index(&node(), &objects);
        assert_eq!(first.topology_revision, next.topology_revision);
        assert!(!first.topology_revision.is_empty());
        let mut new_node = node();
        new_node["agent_session_id"] = json!("new");
        assert_ne!(
            first.topology_revision,
            build_disk_index(&new_node, &objects).topology_revision
        );
        objects[0]["active"] = json!(false);
        assert_ne!(
            first.topology_revision,
            build_disk_index(&node(), &objects).topology_revision
        );
    }
    #[test]
    fn legacy_and_successful_empty_inventory_are_distinct() {
        let legacy = build_disk_index(&node(), &[]);
        assert_eq!(legacy.disk_inventory["state"], "unsupported");
        let mut host = object("host", "node", "host", "system");
        host["properties"]["ciderd_collector_states"] = json!([{"state":{"collector":"diskutil.inventory","stale_after_seconds":60},"last_attempt":{"status":"ok","finished_at":"2026-09-12T12:00:00Z"},"acquisition":context()}]);
        let success = build_disk_index(&node(), &[host.clone()]);
        assert_eq!(success.disk_inventory["state"], "ok");
        assert!(success.disks.is_empty());
        host["properties"]["ciderd_collector_states"][0]["last_attempt"]["status"] =
            json!("failed");
        assert_eq!(
            build_disk_index(&node(), &[host]).disk_inventory["state"],
            "unavailable"
        );
    }
    #[test]
    fn cycles_and_incomplete_pool_backing_cannot_authorize_exclusive_capacity() {
        let mut objects = fixture();
        objects.push(object("store1", "partition", "media", "diskutil.apfs.list"));
        link(&mut objects, 1, 5, "backed_by");
        link(&mut objects, 5, 1, "backed_by");
        let out = build_disk_index(&node(), &objects);
        assert!(out.disks[0]["capacity"]["capacity_bytes"]["value"].is_null());
        assert!(
            out.objects[2]["physical_disk_ids"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        let mut objects = fixture();
        objects.push(object(
            "missing",
            "partition",
            "media",
            "diskutil.apfs.list",
        ));
        link(&mut objects, 2, 5, "backed_by");
        objects[5]["active"] = json!(false);
        let out = build_disk_index(&node(), &objects);
        assert_eq!(out.disks[0]["capacity"]["attribution"], "unresolved");
        assert!(out.disks[0]["capacity"]["capacity_bytes"]["value"].is_null());
    }
    #[test]
    fn local_mount_observers_deduplicate_while_network_and_virtual_edges_do_not_join() {
        let mut objects = vec![
            object(
                "disk0",
                "device",
                "physical_device",
                "diskutil.list.physical",
            ),
            object("media", "partition", "media", "diskutil.list.physical"),
            object("fs", "mount", "filesystem", "diskutil.list.physical"),
            object("m1", "mount", "mount", "/dev/media"),
            object("m2", "mount", "mount", "/dev/media"),
            object("nfs", "nfs_mount", "mount", "server:/share"),
            object("image", "provider", "virtual_device", "diskutil.list"),
        ];
        for i in [3, 4] {
            objects[i]["properties"]["filesystem_type"] = json!("hfs");
            objects[i]["properties"]["filesystem_id"] = json!("same-fsid");
            objects[i]["latest_metrics"]["capacity_bytes"] = json!({"state":"ok","value":"1000"});
        }
        link(&mut objects, 0, 1, "contains");
        link(&mut objects, 2, 1, "backed_by");
        link(&mut objects, 3, 2, "mounts");
        link(&mut objects, 4, 2, "mounts");
        link(&mut objects, 5, 2, "mounts");
        link(&mut objects, 6, 0, "attached_to");
        let out = build_disk_index(&node(), &objects);
        assert_eq!(out.disks[0]["capacity"]["capacity_bytes"]["value"], "1000");
        assert_eq!(out.objects[5]["physical_disk_ids"], json!([]));
        assert_eq!(out.objects[6]["physical_disk_ids"], json!([]));
    }

    #[test]
    fn a_driver_total_cannot_fan_out_to_two_physical_disks() {
        let mut objects = fixture();
        objects.push(object(
            "disk1",
            "device",
            "physical_device",
            "diskutil.list.physical",
        ));
        link(&mut objects, 4, 5, "attached_to");
        let out = build_disk_index(&node(), &objects);
        assert_eq!(out.disks.len(), 2);
        for d in out.disks {
            assert_eq!(d["io"]["linkage_state"], "ambiguous");
            assert!(d["io"]["read_bytes_per_second"]["value"].is_null());
        }
    }
    #[test]
    fn direct_local_mount_backing_is_exact_and_incomplete_members_make_disk_partial() {
        let mut objects = fixture();
        objects.push(object("hfs", "mount", "mount", "/dev/store0"));
        objects[5]["properties"]["filesystem_type"] = json!("hfs");
        link(&mut objects, 5, 1, "backed_by");
        let out = build_disk_index(&node(), &objects);
        assert_eq!(out.objects[5]["physical_disk_ids"], json!([id("disk0")]));
        objects[5]["properties"]["source"] = json!("/dev/store0s1");
        let out = build_disk_index(&node(), &objects);
        assert_eq!(out.objects[5]["physical_disk_ids"], json!([]));
        let mut objects = fixture();
        objects.push(object(
            "missing",
            "partition",
            "media",
            "diskutil.apfs.list",
        ));
        link(&mut objects, 2, 5, "backed_by");
        objects[5]["active"] = json!(false);
        let out = build_disk_index(&node(), &objects);
        assert_eq!(out.disks[0]["topology"]["state"], "partial");
        assert!(
            out.disks[0]["topology"]["reason_codes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r == "member_topology_incomplete")
        );
    }
    #[test]
    fn mixed_apfs_and_independent_mount_capacity_deduplicates_confirmed_filesystem() {
        let mut objects = fixture();
        objects.push(object(
            "hfs",
            "mount",
            "filesystem",
            "diskutil.list.physical",
        ));
        objects.push(object("m1", "mount", "mount", "/dev/store0"));
        objects.push(object("m2", "mount", "mount", "/dev/store0"));
        for i in [6, 7] {
            objects[i]["properties"]["filesystem_type"] = json!("hfs");
            objects[i]["latest_metrics"]["capacity_bytes"] = json!({"state":"ok","value":"100"});
        }
        link(&mut objects, 5, 1, "backed_by");
        link(&mut objects, 6, 5, "mounts");
        link(&mut objects, 7, 5, "mounts");
        let out = build_disk_index(&node(), &objects);
        assert_eq!(out.disks[0]["capacity"]["capacity_bytes"]["value"], "1100");
        objects[2]["active"] = json!(false);
        objects[3]["active"] = json!(false);
        let out = build_disk_index(&node(), &objects);
        assert_eq!(out.disks[0]["capacity"]["capacity_bytes"]["value"], "100");
    }
    #[test]
    fn exact_numeric_inventory_size_does_not_narrow_to_u64() {
        let mut objects = fixture();
        objects[0]["properties"]["size_bytes"] =
            serde_json::from_str("18446744073709552615").unwrap();
        let out = build_disk_index(&node(), &objects);
        assert_eq!(
            out.disks[0]["hardware_size_bytes"]["value"],
            "18446744073709552615"
        );
    }
}
