use ciderd::{
    model::{attrs, Resource},
    topology::{reconcile, Evidence, TopologyInput},
};
use serde_json::json;
fn resource(id: &str, kind: &str, value: serde_json::Value) -> Resource {
    Resource::new(id.into(), kind, attrs(value))
}
fn fixture() -> TopologyInput {
    let mut input = TopologyInput {
        node_id: "node".into(),
        boot_id: "boot".into(),
        agent_session_id: "session".into(),
        ..Default::default()
    };
    input.resources = vec![
        resource(
            "physical",
            "physical_device",
            json!({"bsd_name":"disk0","source":"diskutil.list.physical"}),
        ),
        resource(
            "driver",
            "controller",
            json!({"source":"IOBlockStorageDriver","media_mapping_state":"ok","whole_media_candidates":[{"bsd_name":"disk0","whole":true,"registry_entry_id":"42"}]}),
        ),
        resource(
            "volume",
            "filesystem",
            json!({"bsd_name":"disk3s1","reported_uuid":"volume-uuid","source":"diskutil.apfs.list"}),
        ),
        resource(
            "mount",
            "mount",
            json!({"local":true,"source":"/dev/disk3s1s1","fsid":[1,2],"mount_generation":"g1","mount_identity":{"fsid":[1,2],"source":"/dev/disk3s1s1","volume_uuid":"volume-uuid","media_bsd_name":"disk3s1s1","state":"ok","mount_generation":"g1"}}),
        ),
    ];
    for r in &input.resources {
        input.evidence.insert(
            r.resource_id.clone(),
            Evidence {
                node_id: "node".into(),
                boot_id: "boot".into(),
                agent_session_id: "session".into(),
                source_generation: "1".into(),
                observed_at: "2026-09-12T00:00:00Z".into(),
                state: "ok".into(),
            },
        );
    }
    input
}
#[test]
fn exact_links_are_order_independent_and_never_fan_out() {
    let mut input = fixture();
    for order in [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ] {
        let original = input.resources.clone();
        input.resources = order
            .into_iter()
            .map(|i| original[i].clone())
            .chain([original[3].clone()])
            .collect();
        let out = reconcile(&input);
        assert_eq!(out.relationships.len(), 2);
        assert!(out
            .relationships
            .iter()
            .any(|r| r.from_resource_id == "driver"
                && r.to_resource_id == "physical"
                && r.relation == "attached_to"));
        assert!(out
            .relationships
            .iter()
            .any(|r| r.from_resource_id == "mount"
                && r.to_resource_id == "volume"
                && r.relation == "mounts"));
    }
    let mut driver = input
        .resources
        .iter()
        .find(|r| r.resource_id == "driver")
        .unwrap()
        .clone();
    driver.resource_id = "second-driver".into();
    input
        .evidence
        .insert(driver.resource_id.clone(), input.evidence["driver"].clone());
    input.resources.push(driver);
    let out = reconcile(&input);
    assert!(!out
        .relationships
        .iter()
        .any(|r| r.relation == "attached_to"));
    assert_eq!(out.associations["driver"].state, "ambiguous");
}
#[test]
fn stale_evidence_retains_dated_links_but_boot_session_removal_invalidate_them() {
    let mut input = fixture();
    input.evidence.get_mut("physical").unwrap().state = "stale".into();
    let out = reconcile(&input);
    let edge = out
        .relationships
        .iter()
        .find(|r| r.relation == "attached_to")
        .unwrap();
    assert_eq!(edge.attributes.as_ref().unwrap()["state"], "stale");
    input.evidence.get_mut("physical").unwrap().boot_id = "old".into();
    assert!(!reconcile(&input)
        .relationships
        .iter()
        .any(|r| r.relation == "attached_to"));
    input = fixture();
    input.evidence.get_mut("driver").unwrap().agent_session_id = "old".into();
    assert!(!reconcile(&input)
        .relationships
        .iter()
        .any(|r| r.relation == "attached_to"));
    input = fixture();
    input.resources.retain(|r| r.resource_id != "physical");
    assert!(!reconcile(&input)
        .relationships
        .iter()
        .any(|r| r.relation == "attached_to"));
}
#[test]
fn no_suffix_guessing_duplicate_uuid_or_late_mount_generation() {
    let mut input = fixture();
    input.resources[3].attributes.remove("mount_identity");
    assert!(!reconcile(&input)
        .relationships
        .iter()
        .any(|r| r.relation == "mounts"));
    input = fixture();
    input.resources[3]
        .attributes
        .get_mut("mount_identity")
        .unwrap()["mount_generation"] = "old".into();
    assert!(!reconcile(&input)
        .relationships
        .iter()
        .any(|r| r.relation == "mounts"));
    input = fixture();
    let mut duplicate = input.resources[2].clone();
    duplicate.resource_id = "cloned-volume".into();
    input.evidence.insert(
        duplicate.resource_id.clone(),
        input.evidence["volume"].clone(),
    );
    input.resources.push(duplicate);
    let out = reconcile(&input);
    assert_eq!(out.associations["mount"].state, "ambiguous");
    assert!(!out.relationships.iter().any(|r| r.relation == "mounts"));
}
#[test]
fn virtual_whole_media_and_nested_partitions_do_not_authorize_physical_io() {
    let mut input = fixture();
    input.resources[1]
        .attributes
        .get_mut("whole_media_candidates")
        .unwrap()[0]["whole"] = false.into();
    assert!(!reconcile(&input)
        .relationships
        .iter()
        .any(|r| r.relation == "attached_to"));
    input = fixture();
    input.resources[0]
        .attributes
        .insert("source".into(), "disk-image".into());
    assert!(!reconcile(&input)
        .relationships
        .iter()
        .any(|r| r.relation == "attached_to"));
}

#[test]
fn independent_local_volume_can_use_exact_physical_media_without_invented_filesystem() {
    let mut input = fixture();
    input.resources.retain(|r| r.resource_type != "filesystem");
    let mount = input
        .resources
        .iter_mut()
        .find(|r| r.resource_type == "mount")
        .unwrap();
    mount.attributes.remove("mount_identity");
    mount
        .attributes
        .insert("source".into(), "/dev/disk0".into());
    mount
        .attributes
        .insert("filesystem_type".into(), "hfs".into());
    let out = reconcile(&input);
    assert!(out
        .relationships
        .iter()
        .any(|r| r.from_resource_id == "mount"
            && r.to_resource_id == "physical"
            && r.relation == "backed_by"));
    input
        .resources
        .iter_mut()
        .find(|r| r.resource_type == "mount")
        .unwrap()
        .attributes
        .insert("filesystem_type".into(), "apfs".into());
    assert!(!reconcile(&input)
        .relationships
        .iter()
        .any(|r| r.from_resource_id == "mount"));
}

#[test]
fn mount_identity_original_time_limits_association_time_after_later_source_refresh() {
    let mut input = fixture();
    input.resources[3]
        .attributes
        .get_mut("mount_identity")
        .unwrap()["observed_at"] = "2026-09-11T00:00:00Z".into();
    let out = reconcile(&input);
    assert_eq!(
        out.relationships
            .iter()
            .find(|r| r.relation == "mounts")
            .unwrap()
            .observed_at,
        "2026-09-11T00:00:00Z"
    );
}
