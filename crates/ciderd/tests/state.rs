use ciderd::{model::*, state::State};

fn fixture() -> Heartbeat {
    serde_json::from_str(include_str!("../contract/example-heartbeat.json")).unwrap()
}

#[test]
fn failures_and_partial_samples_retain_older_fields_without_restamping() {
    let hb = fixture();
    let old = hb.collections[0].clone();
    let mut state = State::new(hb, 256);
    state.apply_collection(old.clone(), "1").unwrap();
    let mut partial = old.clone();
    partial.collection_id = "new-partial".into();
    partial.finished_monotonic_ns = (old.finished_monotonic_ns.get() + 1).into();
    partial.metrics.truncate(1);
    partial.status = "partial".into();
    state.apply_collection(partial.clone(), "1").unwrap();
    let mut failed = partial.clone();
    failed.collection_id = "new-timeout".into();
    failed.finished_monotonic_ns = (partial.finished_monotonic_ns.get() + 1).into();
    failed.status = "timeout".into();
    failed.metrics.clear();
    state.apply_collection(failed, "1").unwrap();
    let snapshot = state.snapshot();
    assert!(snapshot.collections.contains(&old));
    assert!(snapshot.collections.contains(&partial));
    assert!(snapshot
        .collections
        .iter()
        .any(|c| c.collection_id == "new-timeout"));
    snapshot.validate().unwrap();
}

#[test]
fn wrong_ack_does_not_retire_inventory_or_events() {
    let hb = fixture();
    let mut state = State::new(hb.clone(), 256);
    let mut ack = Acknowledgement {
        schema_version: "2.0".into(),
        agent_session_id: "other-session".into(),
        accepted_sequence: hb.sequence,
        inventory_revision: Some(hb.inventory.revision),
        request_inventory: false,
        server_received_at: now(),
    };
    assert!(state.acknowledge(&hb, &ack).is_err());
    assert!(state.snapshot().inventory.included);
    ack.agent_session_id = hb.agent_session_id.clone();
    state.acknowledge(&hb, &ack).unwrap();
    assert!(!state.snapshot().inventory.included);
    assert!(state.snapshot().events.is_empty());
}

#[test]
fn stale_generation_cannot_overwrite_a_replacement_resource() {
    let hb = fixture();
    let old = hb.collections[0].clone();
    let mut state = State::new(hb, 256);
    state.set_generation(&old.resource_id, "2");
    assert!(!state.apply_collection(old, "1").unwrap());
}

#[test]
fn payload_summary_has_no_dangling_references_and_does_not_ack_omitted_inventory() {
    let mut state = State::new(fixture(), 256);
    let sent = state.prepare(2, 15, "test-clock", 4096).unwrap();
    assert!(sent.agent.payload_limited);
    assert!(!sent.inventory.included);
    assert!(serde_json::to_vec(&sent).unwrap().len() <= 4096);
    sent.validate().unwrap();
    let ack = Acknowledgement {
        schema_version: "2.0".into(),
        agent_session_id: sent.agent_session_id.clone(),
        accepted_sequence: sent.sequence,
        inventory_revision: Some(sent.inventory.revision),
        request_inventory: false,
        server_received_at: now(),
    };
    state.acknowledge(&sent, &ack).unwrap();
    assert!(state.snapshot().inventory.included);
}

fn empty_state() -> State {
    let mut heartbeat = fixture();
    heartbeat.collections.clear();
    heartbeat.collector_states.clear();
    heartbeat.events.clear();
    State::new(heartbeat, 256)
}

fn sample(index: usize, collector: &str, metrics: Vec<Metric>) -> Collection {
    let mut sample = fixture().collections[0].clone();
    sample.collection_id = format!("sample-{index}");
    sample.resource_id = "node-example-01/nfs-client".into();
    sample.collector = collector.into();
    sample.started_monotonic_ns = 0u64.into();
    sample.finished_monotonic_ns = (index as u64 + 1).into();
    sample.finished_at = sample.started_at.clone();
    sample.metrics = metrics;
    sample.validate().unwrap();
    sample
}

fn worker_metric(index: usize) -> Metric {
    let operation = ["read", "write", "lookup"][index % 3];
    Metric::reading_with_attributes(
        "storage.nfs.client.operations_total",
        serde_json::json!("1"),
        "live",
        Some("test-counter-epoch"),
        attrs(serde_json::json!({"family":"v3_procedure", "operation":operation})),
    )
    .unwrap()
}

#[test]
fn replacing_the_implicit_generation_discards_old_usable_readings() {
    let heartbeat = fixture();
    let resource_id = heartbeat.collections[0].resource_id.clone();
    let mut state = State::new(heartbeat, 256);
    state.set_generation(&resource_id, "replacement");
    assert!(!state
        .snapshot()
        .collections
        .iter()
        .any(|result| result.resource_id == resource_id));
    assert!(!state
        .snapshot()
        .collector_states
        .iter()
        .any(|scope| scope.resource_id == resource_id));
}

#[test]
fn restored_inventory_revision_exceeds_every_saved_entity_revision() {
    let mut state = empty_state();
    let mut metadata = state.metadata.clone();
    metadata.resources.values_mut().next().unwrap().revision = 100u64.into();
    metadata.relationships.values_mut().next().unwrap().revision = 200u64.into();
    metadata.tombstones.push(Tombstone {
        entity_type: "resource".into(),
        entity_id: "node-example-01/removed".into(),
        revision: 300u64.into(),
        observed_at: now(),
        reason: "verified disappearance".into(),
    });
    state.restore(metadata).unwrap();
    assert_eq!(state.snapshot().inventory.revision, 301u64.into());
    let mut resource = state.metadata.resources.values().next().unwrap().clone();
    let resource_id = resource.resource_id.clone();
    resource
        .attributes
        .insert("changed".into(), serde_json::json!(true));
    state
        .reconcile("restored", vec![resource], vec![], false)
        .unwrap();
    assert_eq!(
        state.metadata.resources[&resource_id].revision,
        302u64.into()
    );
}

#[test]
fn exhausted_restored_revision_keeps_existing_metadata_unchanged() {
    let mut state = empty_state();
    let before = serde_json::to_value(&state.metadata).unwrap();
    let before_revision = state.snapshot().inventory.revision;
    let mut replacement = state.metadata.clone();
    replacement.resources.values_mut().next().unwrap().revision = u128::MAX.into();
    assert!(state.restore(replacement).is_err());
    assert_eq!(serde_json::to_value(&state.metadata).unwrap(), before);
    assert_eq!(state.snapshot().inventory.revision, before_revision);
}

#[test]
fn retained_result_limit_rejects_partial_updates_transactionally() {
    let mut state = empty_state();
    // Each scope retains two independently observed available fields.
    for scope in 0..2048 {
        let collector = format!("scope-{scope}");
        state
            .apply_collection(
                sample(scope * 2, &collector, vec![worker_metric(scope * 2)]),
                "1",
            )
            .unwrap();
        state
            .apply_collection(
                sample(
                    scope * 2 + 1,
                    &collector,
                    vec![worker_metric(scope * 2 + 1)],
                ),
                "1",
            )
            .unwrap();
    }
    assert_eq!(state.snapshot().collections.len(), 4096);
    let before = state.snapshot();
    // A third distinct field would require retaining three results in scope 0.
    assert!(state
        .apply_collection(sample(4096, "scope-0", vec![worker_metric(2)]), "1")
        .is_err());
    assert_eq!(state.snapshot().collections, before.collections);
    assert_eq!(state.snapshot().collector_states, before.collector_states);
    assert!(state
        .apply_collection(sample(4097, "scope-0", vec![worker_metric(0)]), "1")
        .unwrap());
    assert_eq!(state.snapshot().collections.len(), 4096);
}

#[test]
fn total_retained_bytes_are_bounded_without_evicting_usable_results() {
    let mut state = empty_state();
    let mut rejected = false;
    for index in 0..90 {
        let mut collection = sample(index, &format!("scope-{index}"), vec![worker_metric(index)]);
        collection.extensions = Some(attrs(
            serde_json::json!({"bounded_padding": "x".repeat(200_000)}),
        ));
        collection.validate().unwrap();
        let before = state.snapshot();
        if state.apply_collection(collection, "1").is_err() {
            assert!(
                index >= 80,
                "ordinary collections below the total limit should remain usable"
            );
            assert_eq!(state.snapshot().collections, before.collections);
            assert_eq!(state.snapshot().collector_states, before.collector_states);
            rejected = true;
            break;
        }
    }
    assert!(rejected, "retained results must stop growing at 16 MiB");
    assert!(state
        .apply_collection(sample(1000, "scope-0", vec![worker_metric(0)]), "1")
        .unwrap());
    let total: usize = state
        .snapshot()
        .collections
        .iter()
        .map(|result| serde_json::to_vec(result).unwrap().len())
        .sum();
    assert!(total <= 16 * 1024 * 1024);
}

fn extra_resource() -> Resource {
    Resource::new(
        "node-example-01/temporary-device".into(),
        "physical_device",
        attrs(serde_json::json!({})),
    )
}
fn extra_relationship() -> Relationship {
    let mut relation = fixture().relationships[0].clone();
    relation.relationship_id = "node-example-01/temporary-edge".into();
    relation.from_resource_id = "node-example-01/host".into();
    relation.to_resource_id = extra_resource().resource_id;
    relation
}

#[test]
fn authoritative_resource_removal_cleans_relationships_generations_and_late_results() {
    let mut state = empty_state();
    let resource = extra_resource();
    let relationship = extra_relationship();
    state
        .reconcile("device-source", vec![resource.clone()], vec![], true)
        .unwrap();
    state
        .reconcile("edge-source", vec![], vec![relationship.clone()], true)
        .unwrap();
    state.set_generation(&resource.resource_id, "replacement");
    let mut old = sample(0, "sample", vec![]);
    old.resource_id = resource.resource_id.clone();
    state.apply_collection(old.clone(), "replacement").unwrap();
    state
        .reconcile("device-source", vec![], vec![], true)
        .unwrap();
    assert!(!state.metadata.resources.contains_key(&resource.resource_id));
    assert!(!state
        .metadata
        .relationships
        .contains_key(&relationship.relationship_id));
    assert!(!state
        .metadata
        .generations
        .contains_key(&resource.resource_id));
    assert!(state
        .metadata
        .groups
        .values()
        .all(|ids| !ids.contains(&relationship.relationship_id)));
    assert!(state
        .metadata
        .tombstones
        .iter()
        .any(|tombstone| tombstone.entity_type == "resource"
            && tombstone.entity_id == resource.resource_id));
    assert!(state
        .metadata
        .tombstones
        .iter()
        .any(|tombstone| tombstone.entity_type == "relationship"
            && tombstone.entity_id == relationship.relationship_id));
    assert!(state
        .snapshot()
        .collector_states
        .iter()
        .all(|scope| scope.resource_id != resource.resource_id));
    old.collection_id = "late-after-tombstone".into();
    assert!(!state.apply_collection(old, "1").unwrap());
    state.phase("sample", &resource.resource_id, WorkerPhase::Idle, 15);
    assert!(state
        .snapshot()
        .collector_states
        .iter()
        .all(|scope| scope.resource_id != resource.resource_id));
}

#[test]
fn shared_resource_ownership_survives_one_sources_removal() {
    let mut state = empty_state();
    let resource = extra_resource();
    let relationship = extra_relationship();
    state
        .reconcile(
            "first",
            vec![resource.clone()],
            vec![relationship.clone()],
            true,
        )
        .unwrap();
    state
        .reconcile(
            "second",
            vec![resource.clone()],
            vec![relationship.clone()],
            true,
        )
        .unwrap();
    state.reconcile("first", vec![], vec![], true).unwrap();
    assert!(state.metadata.resources.contains_key(&resource.resource_id));
    assert!(state
        .metadata
        .relationships
        .contains_key(&relationship.relationship_id));
    assert!(!state
        .metadata
        .tombstones
        .iter()
        .any(|tombstone| tombstone.entity_id == resource.resource_id));
    state.reconcile("second", vec![], vec![], true).unwrap();
    assert!(!state.metadata.resources.contains_key(&resource.resource_id));
    assert!(!state
        .metadata
        .relationships
        .contains_key(&relationship.relationship_id));
    assert_eq!(
        state
            .metadata
            .tombstones
            .iter()
            .filter(|tombstone| tombstone.entity_id == relationship.relationship_id)
            .count(),
        1
    );
}

fn prepared_inventory(state: &State, sequence: u64) -> Heartbeat {
    let sent = state
        .prepare(
            sequence,
            sequence as u128,
            "inventory-test-clock",
            1024 * 1024,
        )
        .unwrap();
    sent.validate().unwrap();
    sent
}

fn acknowledgement_for(sent: &Heartbeat) -> Acknowledgement {
    Acknowledgement {
        schema_version: "2.0".into(),
        agent_session_id: sent.agent_session_id.clone(),
        accepted_sequence: sent.sequence,
        inventory_revision: Some(sent.inventory.revision),
        request_inventory: false,
        server_received_at: now(),
    }
}

#[test]
fn repeated_removal_cycles_produce_valid_unique_pending_tombstones() {
    let mut state = empty_state();
    let resource = extra_resource();
    let relationship = extra_relationship();
    for cycle in 0..4 {
        state
            .reconcile(
                "removable",
                vec![resource.clone()],
                vec![relationship.clone()],
                true,
            )
            .unwrap();
        prepared_inventory(&state, cycle * 2 + 1);
        state.reconcile("removable", vec![], vec![], true).unwrap();
        let removed = prepared_inventory(&state, cycle * 2 + 2);
        assert_eq!(removed.tombstones.len(), 2);
        assert!(removed
            .tombstones
            .iter()
            .all(|record| record.revision == removed.inventory.revision));
    }
}

#[test]
fn reappearance_supersedes_old_deletion_but_delayed_ack_preserves_new_deletion() {
    let mut state = empty_state();
    let resource = extra_resource();
    let relationship = extra_relationship();
    state
        .reconcile(
            "removable",
            vec![resource.clone()],
            vec![relationship.clone()],
            true,
        )
        .unwrap();
    state.reconcile("removable", vec![], vec![], true).unwrap();
    let first_deletion = prepared_inventory(&state, 1);
    assert_eq!(first_deletion.tombstones.len(), 2);
    state
        .reconcile(
            "removable",
            vec![resource.clone()],
            vec![relationship.clone()],
            true,
        )
        .unwrap();
    let reappeared = prepared_inventory(&state, 2);
    assert!(reappeared.tombstones.is_empty());
    assert!(
        state.metadata.resources[&resource.resource_id].revision
            > first_deletion.inventory.revision
    );
    assert!(
        state.metadata.relationships[&relationship.relationship_id].revision
            > first_deletion.inventory.revision
    );
    state.reconcile("removable", vec![], vec![], true).unwrap();
    let second_deletion = prepared_inventory(&state, 3);
    assert_eq!(second_deletion.tombstones.len(), 2);
    assert!(second_deletion
        .tombstones
        .iter()
        .all(|record| record.revision > first_deletion.inventory.revision));
    state
        .acknowledge(&first_deletion, &acknowledgement_for(&first_deletion))
        .unwrap();
    let after_old_ack = prepared_inventory(&state, 4);
    assert_eq!(after_old_ack.tombstones, second_deletion.tombstones);
    assert!(after_old_ack.inventory.included);
    // A further confirmed absence neither restamps nor discards unacknowledged deletion intent.
    state.reconcile("removable", vec![], vec![], true).unwrap();
    assert_eq!(state.metadata.tombstones, second_deletion.tombstones);
    state
        .acknowledge(&second_deletion, &acknowledgement_for(&second_deletion))
        .unwrap();
    assert!(state.metadata.tombstones.is_empty());
    assert!(!prepared_inventory(&state, 5).inventory.included);
}

#[test]
fn rejected_reappearance_preserves_pending_deletion_intent() {
    let mut state = empty_state();
    let resource = extra_resource();
    state
        .reconcile("removable", vec![resource.clone()], vec![], true)
        .unwrap();
    state.reconcile("removable", vec![], vec![], true).unwrap();
    let pending = state.metadata.tombstones.clone();
    let revision = state.snapshot().inventory.revision;
    let mut invalid_edge = extra_relationship();
    invalid_edge.from_resource_id = "node-example-01/absent".into();
    assert!(state
        .reconcile(
            "removable",
            vec![resource.clone()],
            vec![invalid_edge],
            true
        )
        .is_err());
    assert_eq!(state.metadata.tombstones, pending);
    assert_eq!(state.snapshot().inventory.revision, revision);
    assert!(!state.metadata.resources.contains_key(&resource.resource_id));
}

#[test]
fn restore_rejects_duplicate_tombstone_entities_transactionally() {
    let mut state = empty_state();
    let before = state.metadata.clone();
    let revision = state.snapshot().inventory.revision;
    let mut metadata = before.clone();
    let old = Tombstone {
        entity_type: "resource".into(),
        entity_id: extra_resource().resource_id,
        revision: 100u64.into(),
        observed_at: now(),
        reason: "confirmed absence".into(),
    };
    let mut newer = old.clone();
    newer.revision = 101u64.into();
    metadata.tombstones.extend([old, newer]);
    assert!(state.restore(metadata).is_err());
    assert!(state.metadata == before);
    assert_eq!(state.snapshot().inventory.revision, revision);
}

#[test]
fn restore_rejects_tombstones_conflicting_with_current_entities() {
    for kind in ["resource", "relationship"] {
        let mut state = empty_state();
        let before = state.metadata.clone();
        let mut metadata = before.clone();
        let entity_id = if kind == "resource" {
            metadata.resources.keys().next().unwrap().clone()
        } else {
            metadata.relationships.keys().next().unwrap().clone()
        };
        metadata.tombstones.push(Tombstone {
            entity_type: kind.into(),
            entity_id,
            revision: 100u64.into(),
            observed_at: now(),
            reason: "contradictory deletion".into(),
        });
        assert!(state.restore(metadata).is_err());
        assert!(state.metadata == before);
    }
}

fn owned_resource(name: &str, kind: &str) -> Resource {
    Resource::new(
        format!("node-example-01/{name}"),
        kind,
        attrs(serde_json::json!({})),
    )
}

#[test]
fn owner_removal_retires_enrichment_groups_and_exclusive_descendants() {
    let mut state = empty_state();
    let volume = owned_resource("owned-volume", "filesystem");
    let snapshot = owned_resource("owned-snapshot", "snapshot");
    let detail = owned_resource("snapshot-detail", "provider_resource");
    let mut edge = extra_relationship();
    edge.from_resource_id = volume.resource_id.clone();
    edge.to_resource_id = snapshot.resource_id.clone();
    state
        .reconcile("apfs", vec![volume.clone()], vec![], true)
        .unwrap();
    state
        .reconcile_owned(
            "apfs.snapshots:volume",
            &volume.resource_id,
            vec![volume.clone(), snapshot.clone()],
            vec![edge.clone()],
            true,
        )
        .unwrap();
    state
        .reconcile_owned(
            "provider.details:snapshot",
            &snapshot.resource_id,
            vec![detail.clone()],
            vec![],
            true,
        )
        .unwrap();
    assert!(!state.metadata.groups["apfs.snapshots:volume"].contains(&volume.resource_id));
    state.reconcile("apfs", vec![], vec![], true).unwrap();
    for removed in [
        &volume.resource_id,
        &snapshot.resource_id,
        &detail.resource_id,
    ] {
        assert!(!state.metadata.resources.contains_key(removed));
        assert!(state
            .metadata
            .tombstones
            .iter()
            .any(|pending| pending.entity_type == "resource" && &pending.entity_id == removed));
    }
    assert!(!state
        .metadata
        .relationships
        .contains_key(&edge.relationship_id));
    assert!(!state.metadata.groups.contains_key("apfs.snapshots:volume"));
    assert!(!state
        .metadata
        .groups
        .contains_key("provider.details:snapshot"));
    assert!(state.metadata.group_owners.is_empty());
    assert_eq!(prepared_inventory(&state, 1).tombstones.len(), 4);
    let before = state.metadata.clone();
    assert!(state
        .reconcile_owned(
            "apfs.snapshots:volume",
            &volume.resource_id,
            vec![snapshot],
            vec![],
            true
        )
        .is_err());
    assert!(state.metadata == before);
}

#[test]
fn shared_enrichment_children_survive_until_their_last_owner_disappears() {
    let mut state = empty_state();
    let first = owned_resource("first-volume", "filesystem");
    let second = owned_resource("second-volume", "filesystem");
    let snapshot = owned_resource("shared-snapshot", "snapshot");
    let detail = owned_resource("shared-snapshot-detail", "provider_resource");
    state
        .reconcile("apfs", vec![first.clone(), second.clone()], vec![], true)
        .unwrap();
    state
        .reconcile_owned(
            "snapshots:first",
            &first.resource_id,
            vec![snapshot.clone()],
            vec![],
            true,
        )
        .unwrap();
    state
        .reconcile_owned(
            "snapshots:second",
            &second.resource_id,
            vec![snapshot.clone()],
            vec![],
            true,
        )
        .unwrap();
    state
        .reconcile_owned(
            "details:snapshot",
            &snapshot.resource_id,
            vec![detail.clone()],
            vec![],
            true,
        )
        .unwrap();
    state
        .reconcile("apfs", vec![second.clone()], vec![], true)
        .unwrap();
    assert!(!state.metadata.groups.contains_key("snapshots:first"));
    assert!(state.metadata.resources.contains_key(&snapshot.resource_id));
    assert!(state.metadata.resources.contains_key(&detail.resource_id));
    assert!(state.metadata.groups.contains_key("details:snapshot"));
    assert!(!state
        .metadata
        .tombstones
        .iter()
        .any(|pending| pending.entity_id == snapshot.resource_id));
    prepared_inventory(&state, 1);
    state.reconcile("apfs", vec![], vec![], true).unwrap();
    assert!(!state.metadata.resources.contains_key(&snapshot.resource_id));
    assert!(!state.metadata.resources.contains_key(&detail.resource_id));
    assert!(state.metadata.group_owners.is_empty());
    assert_eq!(prepared_inventory(&state, 2).tombstones.len(), 4);
}

#[test]
fn legacy_metadata_defaults_to_no_recorded_enrichment_owners() {
    let mut state = empty_state();
    let mut serialized = serde_json::to_value(&state.metadata).unwrap();
    serialized.as_object_mut().unwrap().remove("group_owners");
    let legacy: ciderd::state::Metadata = serde_json::from_value(serialized).unwrap();
    assert!(legacy.group_owners.is_empty());
    state.restore(legacy).unwrap();
    prepared_inventory(&state, 1);
}

#[test]
fn restore_rejects_enrichment_ownership_with_missing_group_or_owner() {
    let mut state = empty_state();
    let before = state.metadata.clone();
    let owner = "node-example-01/host";
    let mut missing_group = before.clone();
    missing_group
        .group_owners
        .insert("missing-group".into(), owner.into());
    assert!(state.restore(missing_group).is_err());
    assert!(state.metadata == before);
    let mut missing_owner = before.clone();
    missing_owner.groups.insert(
        "enrichment".into(),
        [owner.to_string()].into_iter().collect(),
    );
    missing_owner
        .group_owners
        .insert("enrichment".into(), "node-example-01/absent-owner".into());
    assert!(state.restore(missing_owner).is_err());
    assert!(state.metadata == before);
}

#[test]
fn cyclic_enrichment_ownership_is_rejected_without_mutating_inventory() {
    let mut state = empty_state();
    let first = owned_resource("cycle-first", "provider_resource");
    let second = owned_resource("cycle-second", "provider_resource");
    state
        .reconcile("roots", vec![first.clone(), second.clone()], vec![], true)
        .unwrap();
    state
        .reconcile_owned(
            "first-enrichment",
            &first.resource_id,
            vec![second.clone()],
            vec![],
            true,
        )
        .unwrap();
    let before = state.metadata.clone();
    assert!(state
        .reconcile_owned(
            "second-enrichment",
            &second.resource_id,
            vec![first],
            vec![],
            true
        )
        .is_err());
    assert!(state.metadata == before);
    // Rejecting the cycle lets removal of the root retire the dependent group.
    state.reconcile("roots", vec![], vec![], true).unwrap();
    assert!(state.metadata.group_owners.is_empty());
}

#[test]
fn old_metadata_has_unknown_topology_context_until_fresh_source_evidence() {
    let mut value = serde_json::to_value(ciderd::state::Metadata::default()).unwrap();
    value.as_object_mut().unwrap().remove("topology_evidence");
    let metadata: ciderd::state::Metadata = serde_json::from_value(value).unwrap();
    assert!(metadata.topology_evidence.is_empty());
}
