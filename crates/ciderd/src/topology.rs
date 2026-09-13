//! Pure exact-evidence joins. This group owns relationships, never source endpoints.
use crate::{
    collectors::scoped_id,
    model::{attrs, Relationship, Resource},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Evidence {
    pub node_id: String,
    pub boot_id: String,
    pub agent_session_id: String,
    pub source_generation: String,
    pub observed_at: String,
    pub state: String,
}
#[derive(Clone, Default)]
pub struct TopologyInput {
    pub node_id: String,
    pub boot_id: String,
    pub agent_session_id: String,
    pub resources: Vec<Resource>,
    pub evidence: BTreeMap<String, Evidence>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Association {
    pub state: String,
    pub reason_codes: Vec<String>,
}
#[derive(Default)]
pub struct TopologyResult {
    pub relationships: Vec<Relationship>,
    pub associations: BTreeMap<String, Association>,
}
fn text<'a>(r: &'a Resource, key: &str) -> Option<&'a str> {
    r.attributes.get(key)?.as_str()
}
fn current(input: &TopologyInput, r: &Resource) -> bool {
    input.evidence.get(&r.resource_id).is_some_and(|e| {
        e.node_id == input.node_id
            && e.boot_id == input.boot_id
            && e.agent_session_id == input.agent_session_id
            && matches!(e.state.as_str(), "ok" | "stale")
            && text(r, "source_generation").is_none_or(|g| g == e.source_generation)
    })
}
fn status(out: &mut TopologyResult, r: &Resource, state: &str, reason: &str) {
    out.associations.insert(
        r.resource_id.clone(),
        Association {
            state: state.into(),
            reason_codes: if reason.is_empty() {
                vec![]
            } else {
                vec![reason.into()]
            },
        },
    );
}
fn link(
    input: &TopologyInput,
    out: &mut TopologyResult,
    from: &Resource,
    to: &Resource,
    relation: &str,
    method: &str,
) {
    let a = &input.evidence[&from.resource_id];
    let b = &input.evidence[&to.resource_id];
    let stale = a.state == "stale" || b.state == "stale";
    let state = if stale { "stale" } else { "resolved" };
    let mut observed_at = a.observed_at.clone().min(b.observed_at.clone());
    if method.starts_with("diskarbitration.") || method == "iokit.snapshot_parent" {
        if let Some(identity_time) = from
            .attributes
            .get("mount_identity")
            .and_then(|v| v["observed_at"].as_str())
        {
            observed_at = observed_at.min(identity_time.to_owned());
        }
    }
    let attributes = attrs(json!({"source":"derived.storage","mapping_method":method,
        "evidence_resource_ids":[from.resource_id,to.resource_id],
        "evidence_generations":{from.resource_id.clone():a.source_generation,to.resource_id.clone():b.source_generation},
        "boot_id":input.boot_id,"agent_session_id":input.agent_session_id,
        "state":state,"observed_at":observed_at}));
    out.relationships.push(Relationship {
        relationship_id: scoped_id(
            &from.resource_id,
            &to.resource_id,
            "derived.storage",
            relation,
        ),
        revision: 1u64.into(),
        observed_at,
        from_resource_id: from.resource_id.clone(),
        to_resource_id: to.resource_id.clone(),
        relation: relation.into(),
        attributes: Some(attributes),
    });
    status(out, from, state, if stale { "source_stale" } else { "" });
}
fn exact_local_backing(input: &TopologyInput, out: &mut TopologyResult, mount: &Resource) {
    if !matches!(text(mount,"filesystem_type"),Some(kind) if kind!="apfs" && kind!="nfs" && kind!="smbfs")
    {
        return;
    }
    let Some(bsd) = text(mount, "source")
        .and_then(|s| s.strip_prefix("/dev/"))
        .filter(|s| crate::platform::valid_media_name(s))
    else {
        return;
    };
    let candidates: Vec<_> = input
        .resources
        .iter()
        .filter(|r| {
            matches!(r.resource_type.as_str(), "media" | "physical_device")
                && text(r, "source") == Some("diskutil.list.physical")
                && text(r, "bsd_name") == Some(bsd)
                && current(input, r)
        })
        .collect();
    match candidates.as_slice() {
        [media] => link(input, out, mount, media, "backed_by", "exact_media"),
        [] => {}
        _ => status(out, mount, "ambiguous", "multiple_physical_candidates"),
    }
}

pub fn reconcile(input: &TopologyInput) -> TopologyResult {
    let mut out = TopologyResult::default();
    let physical: Vec<_> = input
        .resources
        .iter()
        .filter(|r| {
            r.resource_type == "physical_device"
                && text(r, "source") == Some("diskutil.list.physical")
                && current(input, r)
        })
        .collect();
    let filesystems: Vec<_> = input
        .resources
        .iter()
        .filter(|r| r.resource_type == "filesystem" && current(input, r))
        .collect();
    for disk in &physical {
        status(&mut out, disk, "unresolved", "driver_mapping_missing");
    }
    let mut proposed = Vec::new();
    for driver in input
        .resources
        .iter()
        .filter(|r| text(r, "source") == Some("IOBlockStorageDriver"))
    {
        status(&mut out, driver, "unresolved", "media_mapping_missing");
        if !current(input, driver) {
            status(&mut out, driver, "unresolved", "source_context_mismatch");
            continue;
        }
        if !matches!(text(driver, "media_mapping_state"), Some("ok" | "stale")) {
            continue;
        }
        let candidates: Vec<_> = driver
            .attributes
            .get("whole_media_candidates")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|m| m["whole"] == true)
            .collect();
        if candidates.len() > 1 {
            status(&mut out, driver, "ambiguous", "multiple_whole_media");
            continue;
        }
        let Some(media) = candidates.first() else {
            continue;
        };
        let matches: Vec<_> = physical
            .iter()
            .filter(|p| text(p, "bsd_name") == media["bsd_name"].as_str())
            .copied()
            .collect();
        match matches.as_slice() {
            [disk] => proposed.push((driver, *disk)),
            [] => status(&mut out, driver, "unresolved", "physical_endpoint_missing"),
            _ => status(
                &mut out,
                driver,
                "ambiguous",
                "multiple_physical_candidates",
            ),
        }
    }
    for (driver, disk) in &proposed {
        if proposed
            .iter()
            .filter(|(_, p)| p.resource_id == disk.resource_id)
            .count()
            != 1
        {
            status(&mut out, driver, "ambiguous", "multiple_driver_candidates");
            status(&mut out, disk, "ambiguous", "multiple_driver_candidates");
        } else {
            link(
                input,
                &mut out,
                driver,
                disk,
                "attached_to",
                "iokit.direct_whole_media",
            );
            let association = out.associations[&driver.resource_id].clone();
            out.associations
                .insert(disk.resource_id.clone(), association);
        }
    }
    for mount in input
        .resources
        .iter()
        .filter(|r| r.resource_type == "mount")
    {
        status(&mut out, mount, "unresolved", "filesystem_identity_missing");
        if !current(input, mount) {
            status(&mut out, mount, "unresolved", "source_context_mismatch");
            continue;
        }
        if mount.attributes.get("local") != Some(&Value::Bool(true))
            || matches!(text(mount, "filesystem_type"), Some("nfs" | "smbfs"))
        {
            continue;
        }
        let identity = mount.attributes.get("mount_identity");
        let mut method = "exact_media";
        let matches: Vec<_> = if let Some(identity) = identity {
            if identity["state"] != "ok" && identity["state"] != "stale" {
                exact_local_backing(input, &mut out, mount);
                continue;
            }
            if identity
                .get("boot_id")
                .and_then(Value::as_str)
                .is_some_and(|v| v != input.boot_id)
                || identity
                    .get("agent_session_id")
                    .and_then(Value::as_str)
                    .is_some_and(|v| v != input.agent_session_id)
                || identity["mount_generation"].as_str() != text(mount, "mount_generation")
                || Some(&identity["fsid"]) != mount.attributes.get("fsid")
                || identity["source"].as_str() != text(mount, "source")
            {
                status(&mut out, mount, "unresolved", "mount_generation_mismatch");
                continue;
            }
            if let Some(uuid) = identity["parent_volume_uuid"]
                .as_str()
                .or(identity["volume_uuid"].as_str())
            {
                method = if identity["parent_volume_uuid"].is_string() {
                    "iokit.snapshot_parent"
                } else {
                    "diskarbitration.volume_uuid"
                };
                filesystems
                    .iter()
                    .filter(|r| {
                        text(r, "reported_uuid").is_some_and(|v| v.eq_ignore_ascii_case(uuid))
                    })
                    .copied()
                    .collect()
            } else {
                method = "diskarbitration.exact_media";
                filesystems
                    .iter()
                    .filter(|r| text(r, "bsd_name") == identity["media_bsd_name"].as_str())
                    .copied()
                    .collect()
            }
        } else {
            let Some(bsd) = text(mount, "source")
                .and_then(|s| s.strip_prefix("/dev/"))
                .filter(|s| crate::platform::valid_media_name(s))
            else {
                continue;
            };
            filesystems
                .iter()
                .filter(|r| text(r, "bsd_name") == Some(bsd))
                .copied()
                .collect()
        };
        match matches.as_slice() {
            [filesystem] => {
                if let Some(parent_bsd) = identity.and_then(|v| v["parent_media_bsd_name"].as_str())
                {
                    if text(filesystem, "bsd_name") != Some(parent_bsd) {
                        status(&mut out, mount, "unresolved", "conflicting_volume_identity");
                        continue;
                    }
                }
                link(input, &mut out, mount, filesystem, "mounts", method);
                if identity.is_some_and(|v| v["state"] == "stale") {
                    let edge = out.relationships.last_mut().unwrap();
                    edge.attributes
                        .as_mut()
                        .unwrap()
                        .insert("state".into(), "stale".into());
                    status(&mut out, mount, "stale", "source_stale");
                }
            }
            [] => exact_local_backing(input, &mut out, mount),
            _ => status(
                &mut out,
                mount,
                "ambiguous",
                "duplicate_filesystem_identity",
            ),
        }
    }
    out.relationships
        .sort_by(|a, b| a.relationship_id.cmp(&b.relationship_id));
    out
}
