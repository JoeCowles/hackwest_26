//! Single-owner cache. These methods perform neither filesystem nor network I/O.
use crate::model::*;
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_METADATA_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_COLLECTION_BYTES: usize = 256 * 1024;
pub const MAX_RETAINED_COLLECTION_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_COLLECTOR_SCOPES: usize = 4096;
pub const MAX_METRIC_KEYS_PER_SCOPE: usize = 4096;
pub const MAX_RETAINED_COLLECTIONS: usize = 4096;

#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Metadata {
    pub resources: BTreeMap<String, Resource>,
    pub relationships: BTreeMap<String, Relationship>,
    pub tombstones: Vec<Tombstone>,
    pub groups: BTreeMap<String, BTreeSet<String>>,
    #[serde(default)]
    pub group_owners: BTreeMap<String, String>,
    pub generations: BTreeMap<String, String>,
}

#[derive(Clone, Default)]
struct Slot {
    last: Option<String>,
    fields: BTreeMap<String, String>,
}

pub struct State {
    envelope: Heartbeat,
    pub metadata: Metadata,
    slots: BTreeMap<(String, String), Slot>,
    results: BTreeMap<String, Collection>,
    result_sizes: BTreeMap<String, usize>,
    accepted: BTreeSet<String>,
    event_limit: usize,
    inventory_ack: Option<Decimal>,
}

impl State {
    pub fn new(mut envelope: Heartbeat, event_limit: usize) -> Self {
        let collections = std::mem::take(&mut envelope.collections);
        let metadata = Metadata {
            resources: envelope
                .resources
                .iter()
                .map(|r| (r.resource_id.clone(), r.clone()))
                .collect(),
            relationships: envelope
                .relationships
                .iter()
                .map(|r| (r.relationship_id.clone(), r.clone()))
                .collect(),
            tombstones: envelope.tombstones.clone(),
            ..Metadata::default()
        };
        let mut state = Self {
            envelope,
            metadata,
            slots: BTreeMap::new(),
            results: BTreeMap::new(),
            result_sizes: BTreeMap::new(),
            accepted: BTreeSet::new(),
            event_limit,
            inventory_ack: None,
        };
        for c in collections {
            let _ = state.apply_collection(c, "1");
        }
        state
    }

    pub fn restore(&mut self, metadata: Metadata) -> Result<()> {
        validate_metadata(&metadata)?;
        // Saved objects keep their observation dates and revisions. This new
        // graph version must be above every saved revision before any upsert.
        let maximum = metadata
            .resources
            .values()
            .map(|record| record.revision.get())
            .chain(
                metadata
                    .relationships
                    .values()
                    .map(|record| record.revision.get()),
            )
            .chain(
                metadata
                    .tombstones
                    .iter()
                    .map(|record| record.revision.get()),
            )
            .chain(std::iter::once(self.envelope.inventory.revision.get()))
            .max()
            .unwrap_or(0);
        let revision = maximum
            .checked_add(1)
            .context("inventory revision exhausted")?;
        let previous_generations = &self.metadata.generations;
        let still_current = |resource: &str| {
            metadata.resources.contains_key(resource)
                && previous_generations
                    .get(resource)
                    .map(String::as_str)
                    .unwrap_or("1")
                    == metadata
                        .generations
                        .get(resource)
                        .map(String::as_str)
                        .unwrap_or("1")
        };
        self.slots
            .retain(|(_, resource), _| still_current(resource));
        self.envelope
            .collector_states
            .retain(|scope| still_current(&scope.resource_id));
        self.metadata = metadata;
        self.envelope.inventory.revision = revision.into();
        self.inventory_ack = None;
        self.prune();
        Ok(())
    }

    pub fn set_generation(&mut self, resource: &str, generation: &str) {
        if !self.metadata.resources.contains_key(resource) || check_id(generation).is_err() {
            return;
        }
        let changed = self.generation(resource) != generation;
        self.metadata
            .generations
            .insert(resource.into(), generation.into());
        if changed {
            self.slots
                .retain(|(_, resource_id), _| resource_id != resource);
            self.envelope
                .collector_states
                .retain(|scope| scope.resource_id != resource);
            self.prune();
        }
    }

    pub fn generation(&self, resource: &str) -> &str {
        self.metadata
            .generations
            .get(resource)
            .map(String::as_str)
            .unwrap_or("1")
    }

    pub fn apply_collection(&mut self, collection: Collection, generation: &str) -> Result<bool> {
        if !self
            .metadata
            .resources
            .contains_key(&collection.resource_id)
            || self.generation(&collection.resource_id) != generation
        {
            return Ok(false);
        }
        collection.validate()?;
        let collection_bytes = serde_json::to_vec(&collection)?.len();
        ensure!(
            collection_bytes <= MAX_COLLECTION_BYTES,
            "collection byte limit exceeded"
        );
        if let Some(existing) = self.results.get(&collection.collection_id) {
            ensure!(
                existing == &collection,
                "collection ID reused with different content"
            );
            return Ok(false);
        }
        let key = (collection.collector.clone(), collection.resource_id.clone());
        if let Some(last) = self
            .slots
            .get(&key)
            .and_then(|slot| slot.last.as_ref())
            .and_then(|id| self.results.get(id))
        {
            if last.clock_id == collection.clock_id
                && last.finished_monotonic_ns > collection.finished_monotonic_ns
            {
                return Ok(false);
            }
        }
        ensure!(
            self.slots.contains_key(&key) || self.slots.len() < MAX_COLLECTOR_SCOPES,
            "collector scope limit exceeded"
        );
        let scope_position = self
            .envelope
            .collector_states
            .iter()
            .position(|scope| scope.collector == key.0 && scope.resource_id == key.1);
        ensure!(
            scope_position.is_some() || self.envelope.collector_states.len() < MAX_COLLECTOR_SCOPES,
            "collector state limit exceeded"
        );

        // Stage only the changed scope. All admission checks precede mutation
        // of the last attempt, usable fields, result cache, and live scope.
        let mut candidate = self.slots.get(&key).cloned().unwrap_or_default();
        candidate.last = Some(collection.collection_id.clone());
        for metric in &collection.metrics {
            if metric.availability == "available" {
                candidate
                    .fields
                    .insert(metric.key(), collection.collection_id.clone());
            }
        }
        ensure!(
            candidate.fields.len() <= MAX_METRIC_KEYS_PER_SCOPE,
            "metric key limit exceeded; prior readings retained"
        );
        let referenced: BTreeSet<&String> = self
            .slots
            .iter()
            .filter(|(scope, _)| *scope != &key)
            .flat_map(|(_, slot)| slot.last.iter().chain(slot.fields.values()))
            .chain(candidate.last.iter().chain(candidate.fields.values()))
            .collect();
        ensure!(
            referenced.len() <= MAX_RETAINED_COLLECTIONS,
            "retained collection count exceeded; prior readings retained"
        );
        let retained_bytes = referenced.iter().try_fold(0usize, |total, id| {
            let bytes = if id.as_str() == collection.collection_id {
                collection_bytes
            } else {
                *self
                    .result_sizes
                    .get(*id)
                    .context("missing retained collection size")?
            };
            total
                .checked_add(bytes)
                .context("retained collection size overflow")
        })?;
        ensure!(
            retained_bytes <= MAX_RETAINED_COLLECTION_BYTES,
            "retained collection byte limit exceeded; prior readings retained"
        );

        self.slots.insert(key.clone(), candidate);
        if let Some(position) = scope_position {
            self.envelope.collector_states[position].last_attempt_id =
                Some(collection.collection_id.clone());
        } else {
            self.envelope.collector_states.push(CollectorState {
                collector: key.0,
                resource_id: key.1,
                phase: WorkerPhase::Idle,
                poll_interval_seconds: 15,
                stale_after_seconds: 45,
                last_attempt_id: Some(collection.collection_id.clone()),
            });
        }
        self.result_sizes
            .insert(collection.collection_id.clone(), collection_bytes);
        self.results
            .insert(collection.collection_id.clone(), collection);
        self.prune();
        Ok(true)
    }

    fn prune(&mut self) {
        let referenced: BTreeSet<_> = self
            .slots
            .values()
            .flat_map(|s| s.last.iter().chain(s.fields.values()).cloned())
            .collect();
        let discarded = self
            .results
            .keys()
            .filter(|id| !referenced.contains(*id) && !self.accepted.contains(*id))
            .count();
        self.envelope.agent.discarded_samples_total = self
            .envelope
            .agent
            .discarded_samples_total
            .get()
            .saturating_add(discarded as u128)
            .into();
        self.results.retain(|id, _| referenced.contains(id));
        self.result_sizes.retain(|id, _| referenced.contains(id));
        self.accepted.retain(|id| referenced.contains(id));
    }

    pub fn phase(&mut self, collector: &str, resource: &str, phase: WorkerPhase, interval: u64) {
        if !self.metadata.resources.contains_key(resource) {
            return;
        }
        if let Some(s) = self
            .envelope
            .collector_states
            .iter_mut()
            .find(|s| s.collector == collector && s.resource_id == resource)
        {
            s.phase = phase;
            s.poll_interval_seconds = interval;
            s.stale_after_seconds = interval.saturating_mul(3).min(604800);
        } else if self.envelope.collector_states.len() < 4096 {
            self.envelope.collector_states.push(CollectorState {
                collector: collector.into(),
                resource_id: resource.into(),
                phase,
                poll_interval_seconds: interval,
                stale_after_seconds: interval.saturating_mul(3).min(604800),
                last_attempt_id: None,
            });
        }
    }

    pub fn event(&mut self, source: &str, message: &str) {
        if self.envelope.events.len() >= self.event_limit
            || self
                .envelope
                .events
                .iter()
                .map(|e| e.message.len())
                .sum::<usize>()
                >= 256 * 1024
        {
            self.envelope.agent.dropped_events_total = self
                .envelope
                .agent
                .dropped_events_total
                .get()
                .saturating_add(1)
                .into();
            return;
        }
        // Messages are fixed operational descriptions; no subprocess output or credentials.
        let message: String = message.chars().take(1024).collect();
        self.envelope.events.push(Event {
            event_id: uuid::Uuid::new_v4().to_string(),
            resource_id: format!("{}/host", self.envelope.node_id),
            observed_at: now(),
            source: source.into(),
            category: "collector".into(),
            severity: "warning".into(),
            message,
            details: None,
            raw_artifact_sha256: None,
        });
    }

    pub fn reconcile(
        &mut self,
        group: &str,
        resources: Vec<Resource>,
        relationships: Vec<Relationship>,
        complete: bool,
    ) -> Result<()> {
        self.reconcile_group(group, None, resources, relationships, complete)
    }

    /// Reconcile enrichment collected for an existing resource. The group can
    /// enrich its owner but cannot independently keep that owner present.
    pub fn reconcile_owned(
        &mut self,
        group: &str,
        owner: &str,
        resources: Vec<Resource>,
        relationships: Vec<Relationship>,
        complete: bool,
    ) -> Result<()> {
        self.reconcile_group(group, Some(owner), resources, relationships, complete)
    }

    fn reconcile_group(
        &mut self,
        group: &str,
        owner: Option<&str>,
        resources: Vec<Resource>,
        relationships: Vec<Relationship>,
        complete: bool,
    ) -> Result<()> {
        check_id(group)?;
        let recorded_owner = self.metadata.group_owners.get(group).map(String::as_str);
        if let (Some(previous), Some(requested)) = (recorded_owner, owner) {
            ensure!(
                previous == requested,
                "enrichment group owner cannot change"
            );
        }
        let owner = owner.or(recorded_owner).map(str::to_owned);
        if let Some(owner) = &owner {
            check_id(owner)?;
            ensure!(
                self.metadata.resources.contains_key(owner),
                "enrichment owner is no longer present"
            );
        }
        let mut metadata = self.metadata.clone();
        if let Some(owner) = &owner {
            metadata.group_owners.insert(group.into(), owner.clone());
        }
        let mut ids = BTreeSet::new();
        let revision = self
            .envelope
            .inventory
            .revision
            .get()
            .checked_add(1)
            .context("inventory revision exhausted")?;
        for mut resource in resources {
            resource.validate()?;
            ensure!(
                ids.insert(resource.resource_id.clone()),
                "duplicate inventory resource"
            );
            ensure!(
                !metadata.relationships.contains_key(&resource.resource_id),
                "resource ID collides with a relationship"
            );
            let reappeared =
                supersede_tombstone(&mut metadata, "resource", &resource.resource_id, revision)?;
            // A successful enumeration preserves the original observation when
            // all semantic attributes remain unchanged and no deletion is superseded.
            if let Some(old) = metadata.resources.get(&resource.resource_id) {
                if !reappeared
                    && old.attributes == resource.attributes
                    && old.capabilities == resource.capabilities
                    && old.resource_type == resource.resource_type
                    && old.identity_confidence == resource.identity_confidence
                {
                    continue;
                }
            }
            resource.revision = revision.into();
            metadata
                .resources
                .insert(resource.resource_id.clone(), resource);
        }
        for mut relationship in relationships {
            relationship.validate()?;
            ensure!(
                ids.insert(relationship.relationship_id.clone()),
                "duplicate inventory relationship"
            );
            ensure!(
                !metadata
                    .resources
                    .contains_key(&relationship.relationship_id),
                "relationship ID collides with a resource"
            );
            ensure!(
                metadata
                    .resources
                    .contains_key(&relationship.from_resource_id)
                    && metadata
                        .resources
                        .contains_key(&relationship.to_resource_id),
                "unresolved inventory relationship"
            );
            let reappeared = supersede_tombstone(
                &mut metadata,
                "relationship",
                &relationship.relationship_id,
                revision,
            )?;
            if let Some(old) = metadata.relationships.get(&relationship.relationship_id) {
                if !reappeared
                    && old.from_resource_id == relationship.from_resource_id
                    && old.to_resource_id == relationship.to_resource_id
                    && old.relation == relationship.relation
                    && old.attributes == relationship.attributes
                {
                    continue;
                }
            }
            relationship.revision = revision.into();
            metadata
                .relationships
                .insert(relationship.relationship_id.clone(), relationship);
        }
        if let Some(owner) = &owner {
            ids.remove(owner);
        }
        let mut removed = Vec::new();
        if complete {
            let previous = metadata
                .groups
                .insert(group.into(), ids.clone())
                .unwrap_or_default();
            removed.extend(
                previous
                    .difference(&ids)
                    .filter(|id| owner.as_ref() != Some(*id))
                    .cloned(),
            );
        } else {
            metadata.groups.entry(group.into()).or_default().extend(ids);
            if let Some(owner) = &owner {
                metadata.groups.get_mut(group).unwrap().remove(owner);
            }
        }
        retire_unowned_entities(&mut metadata, removed, revision);

        // Even an edge owned by another collector cannot remain attached to a
        // removed endpoint. Tombstone each dependent edge once and clean every
        // group's membership, so later reconciliations do not repeat deletions.
        let dangling: Vec<_> = metadata
            .relationships
            .iter()
            .filter(|(_, edge)| {
                !metadata.resources.contains_key(&edge.from_resource_id)
                    || !metadata.resources.contains_key(&edge.to_resource_id)
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in dangling {
            metadata.relationships.remove(&id);
            append_tombstone(&mut metadata, "relationship", &id, revision);
        }
        for members in metadata.groups.values_mut() {
            members.retain(|id| {
                metadata.resources.contains_key(id) || metadata.relationships.contains_key(id)
            });
        }
        metadata
            .generations
            .retain(|id, _| metadata.resources.contains_key(id));
        validate_metadata(&metadata)?;
        if metadata != self.metadata {
            self.metadata = metadata;
            self.envelope.inventory.revision = revision.into();
            self.slots
                .retain(|(_, id), _| self.metadata.resources.contains_key(id));
            self.envelope
                .collector_states
                .retain(|scope| self.metadata.resources.contains_key(&scope.resource_id));
            self.prune();
        }
        Ok(())
    }

    pub fn request_inventory(&mut self) {
        self.inventory_ack = None;
    }

    pub fn acknowledge(&mut self, sent: &Heartbeat, ack: &Acknowledgement) -> Result<()> {
        ack.validate_for(sent)?;
        ensure!(
            sent.agent_session_id == self.envelope.agent_session_id
                && sent.agent_generation == self.envelope.agent_generation,
            "retired session acknowledgement"
        );
        if ack.request_inventory {
            self.inventory_ack = None;
        } else if sent.inventory.included && ack.inventory_revision == Some(sent.inventory.revision)
        {
            self.inventory_ack = ack.inventory_revision;
        }
        let events: BTreeSet<_> = sent.events.iter().map(|e| &e.event_id).collect();
        self.envelope
            .events
            .retain(|e| !events.contains(&e.event_id));
        if sent.inventory.included && ack.inventory_revision == Some(sent.inventory.revision) {
            self.metadata
                .tombstones
                .retain(|t| !sent.tombstones.contains(t));
        }
        self.accepted.extend(
            sent.collections
                .iter()
                .filter(|c| self.results.contains_key(&c.collection_id))
                .map(|c| c.collection_id.clone()),
        );
        Ok(())
    }

    pub fn snapshot(&self) -> Heartbeat {
        let mut snapshot = self.envelope.clone();
        snapshot.collections = self.results.values().cloned().collect();
        snapshot.inventory.included = self.inventory_ack != Some(snapshot.inventory.revision);
        snapshot.resources = self.metadata.resources.values().cloned().collect();
        snapshot.relationships = self.metadata.relationships.values().cloned().collect();
        snapshot.tombstones = self.metadata.tombstones.clone();
        snapshot.agent.quarantined_workers = snapshot
            .collector_states
            .iter()
            .filter(|s| s.phase == WorkerPhase::TimedOutPendingExit)
            .count();
        snapshot
    }

    pub fn prepare(
        &self,
        sequence: u64,
        monotonic_ns: u128,
        clock_id: &str,
        limit: usize,
    ) -> Result<Heartbeat> {
        prepare(self.snapshot(), sequence, monotonic_ns, clock_id, limit)
    }
}

fn retire_unowned_entities(metadata: &mut Metadata, mut candidates: Vec<String>, revision: u128) {
    loop {
        for id in candidates.drain(..) {
            // A remaining independent source can keep the entity present.
            if metadata
                .groups
                .values()
                .any(|members| members.contains(&id))
            {
                continue;
            }
            let kind = if metadata.resources.remove(&id).is_some() {
                Some("resource")
            } else if metadata.relationships.remove(&id).is_some() {
                Some("relationship")
            } else {
                None
            };
            if let Some(kind) = kind {
                append_tombstone(metadata, kind, &id, revision);
            }
        }
        let retired: Vec<_> = metadata
            .group_owners
            .iter()
            .filter(|(_, owner)| !metadata.resources.contains_key(*owner))
            .map(|(group, _)| group.clone())
            .collect();
        if retired.is_empty() {
            break;
        }
        for group in retired {
            metadata.group_owners.remove(&group);
            if let Some(members) = metadata.groups.remove(&group) {
                candidates.extend(members);
            }
        }
        // Removing an exclusively owned child can in turn retire its own
        // enrichment groups, so repeat until no dependent ownership remains.
    }
}

fn append_tombstone(metadata: &mut Metadata, kind: &str, id: &str, revision: u128) {
    let deletion = Tombstone {
        entity_type: kind.into(),
        entity_id: id.into(),
        revision: revision.into(),
        observed_at: now(),
        reason: "absent from successful authoritative enumeration".into(),
    };
    if let Some(pending) = metadata
        .tombstones
        .iter_mut()
        .find(|pending| pending.entity_type == kind && pending.entity_id == id)
    {
        // Equal revisions describe the same deletion; retain its original date.
        if pending.revision < deletion.revision {
            *pending = deletion;
        }
    } else {
        metadata.tombstones.push(deletion);
    }
}

fn supersede_tombstone(
    metadata: &mut Metadata,
    kind: &str,
    id: &str,
    revision: u128,
) -> Result<bool> {
    ensure!(
        metadata
            .tombstones
            .iter()
            .filter(|pending| pending.entity_type == kind && pending.entity_id == id)
            .all(|pending| pending.revision.get() < revision),
        "upsert does not supersede pending deletion revision"
    );
    let previous = metadata.tombstones.len();
    metadata
        .tombstones
        .retain(|pending| pending.entity_type != kind || pending.entity_id != id);
    Ok(previous != metadata.tombstones.len())
}

fn validate_metadata(metadata: &Metadata) -> Result<()> {
    ensure!(
        metadata.resources.len() <= 4096
            && metadata.relationships.len() <= 8192
            && metadata.tombstones.len() <= 8192,
        "inventory record limit exceeded; previous inventory retained"
    );
    ensure!(
        serde_json::to_vec(metadata)?.len() <= MAX_METADATA_BYTES,
        "metadata byte limit exceeded; previous inventory retained"
    );
    for (id, resource) in &metadata.resources {
        resource.validate()?;
        ensure!(
            id == &resource.resource_id,
            "resource index does not match identity"
        );
        ensure!(
            !metadata.relationships.contains_key(id),
            "resource ID collides with a relationship"
        );
    }
    for (id, edge) in &metadata.relationships {
        edge.validate()?;
        ensure!(
            id == &edge.relationship_id,
            "relationship index does not match identity"
        );
        ensure!(
            metadata.resources.contains_key(&edge.from_resource_id)
                && metadata.resources.contains_key(&edge.to_resource_id),
            "unresolved inventory relationship"
        );
    }
    let mut deleted = BTreeSet::new();
    for tombstone in &metadata.tombstones {
        tombstone.validate()?;
        ensure!(
            deleted.insert((&tombstone.entity_type, &tombstone.entity_id)),
            "duplicate pending tombstone entity"
        );
        let present = if tombstone.entity_type == "resource" {
            metadata.resources.contains_key(&tombstone.entity_id)
        } else {
            metadata.relationships.contains_key(&tombstone.entity_id)
        };
        ensure!(
            !present,
            "pending tombstone conflicts with a present entity"
        );
    }
    for (group, members) in &metadata.groups {
        check_id(group)?;
        ensure!(
            members
                .iter()
                .all(|id| metadata.resources.contains_key(id)
                    || metadata.relationships.contains_key(id)),
            "unresolved inventory group member"
        );
    }
    for (group, owner) in &metadata.group_owners {
        let members = metadata
            .groups
            .get(group)
            .context("enrichment owner refers to a missing group")?;
        ensure!(
            metadata.resources.contains_key(owner),
            "enrichment group has a missing owner"
        );
        ensure!(
            !members.contains(owner),
            "enrichment group cannot own its own owner"
        );
    }
    validate_ownership_dependencies(metadata)?;
    for (resource, generation) in &metadata.generations {
        ensure!(
            metadata.resources.contains_key(resource),
            "unresolved resource generation"
        );
        check_id(generation)?;
    }
    Ok(())
}

fn validate_ownership_dependencies(metadata: &Metadata) -> Result<()> {
    if metadata.group_owners.is_empty() {
        return Ok(());
    }
    let mut children: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    let mut incoming: BTreeMap<&str, usize> = metadata
        .resources
        .keys()
        .map(|id| (id.as_str(), 0))
        .collect();
    for (group, owner) in &metadata.group_owners {
        for member in &metadata.groups[group] {
            if metadata.resources.contains_key(member)
                && children
                    .entry(owner.as_str())
                    .or_default()
                    .insert(member.as_str())
            {
                *incoming
                    .get_mut(member.as_str())
                    .expect("validated group resource") += 1;
            }
        }
    }
    let mut ready: Vec<&str> = incoming
        .iter()
        .filter(|(_, count)| **count == 0)
        .map(|(id, _)| *id)
        .collect();
    let mut visited = 0usize;
    while let Some(owner) = ready.pop() {
        visited += 1;
        if let Some(dependents) = children.get(owner) {
            for child in dependents {
                let count = incoming
                    .get_mut(*child)
                    .expect("validated dependent resource");
                *count -= 1;
                if *count == 0 {
                    ready.push(child);
                }
            }
        }
    }
    ensure!(
        visited == metadata.resources.len(),
        "cyclic enrichment ownership would prevent resource retirement"
    );
    Ok(())
}

pub fn prepare(
    mut heartbeat: Heartbeat,
    sequence: u64,
    monotonic_ns: u128,
    clock_id: &str,
    limit: usize,
) -> Result<Heartbeat> {
    heartbeat.sequence = sequence.into();
    heartbeat.created_at = now();
    heartbeat.monotonic_ns = monotonic_ns.into();
    heartbeat.clock_id = clock_id.into();
    if !heartbeat.inventory.included {
        heartbeat.resources.clear();
        heartbeat.relationships.clear();
        heartbeat.tombstones.clear();
    }
    if serde_json::to_vec(&heartbeat)?.len() > limit {
        heartbeat.agent.payload_limited = true;
        heartbeat.inventory.included = false;
        heartbeat.resources.clear();
        heartbeat.relationships.clear();
        heartbeat.tombstones.clear();
        heartbeat.collections.clear();
        heartbeat.collector_states.clear();
        heartbeat.events.clear();
        heartbeat.extensions = None;
    }
    ensure!(
        serde_json::to_vec(&heartbeat)?.len() <= limit,
        "mandatory heartbeat exceeds byte limit"
    );
    heartbeat.validate()?;
    Ok(heartbeat)
}
