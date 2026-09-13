use crate::{
    clock::{Clock, SampleTime},
    collectors::{self, Collected},
    command::{CommandOutcome, CommandOutput, CommandRunner, CommandSpec, RunningCommand},
    config::Config,
    model::*,
    platform::WorkerRequest,
    runtime::Message,
    state::State,
};
use anyhow::{Context, Result};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    path::PathBuf,
    sync::Arc,
    time::Duration,
};
use tokio::{
    sync::{mpsc, watch},
    task::{JoinHandle, JoinSet},
    time::{interval, MissedTickBehavior},
};

#[derive(Clone, Copy, Debug)]
enum Kind {
    Inventory,
    Apfs,
    Iokit,
    Mounts,
    MountIdentity,
    Nfs,
    NfsStatus,
    Capacity,
    Smart,
    Snapshots,
}
#[derive(Clone)]
struct Job {
    kind: Kind,
    resource: String,
    generation: String,
    collector: String,
    interval: u64,
    request: Option<WorkerRequest>,
    args: Vec<OsString>,
    mount: Option<Resource>,
}
pub(crate) struct CompletedJob {
    job: Job,
    pub collected: Option<Collected>,
    stamp: SampleTime,
    finished_at: String,
    finished_ns: u128,
    run_id: String,
    status: String,
    code: &'static str,
    exit_code: Option<u8>,
    pending: bool,
    source_version: String,
}
struct ContextInfo {
    node: String,
    boot: String,
    session: String,
    os_build: String,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn start(
    config: Config,
    worker: PathBuf,
    node: String,
    boot: String,
    session: String,
    os_build: String,
    clock: Arc<Clock>,
    runners: Vec<Arc<CommandRunner>>,
    snapshots: watch::Receiver<Arc<Heartbeat>>,
    messages: mpsc::Sender<Message>,
    stopping: watch::Receiver<bool>,
) -> Vec<JoinHandle<()>> {
    let context = Arc::new(ContextInfo {
        node,
        boot,
        session,
        os_build,
    });
    [Kind::Inventory,Kind::Apfs,Kind::Iokit,Kind::Mounts,Kind::MountIdentity,Kind::Nfs,Kind::NfsStatus,Kind::Capacity,Kind::Smart,Kind::Snapshots].into_iter().map(|kind| {
        let config=config.clone();let worker=worker.clone();let context=context.clone();let clock=clock.clone();let runners=runners.clone();let snapshots=snapshots.clone();let messages=messages.clone();let mut stopping=stopping.clone();
        tokio::spawn(async move {
            let seconds=period(kind,&config);let mut ticks=interval(Duration::from_secs(1));ticks.set_missed_tick_behavior(MissedTickBehavior::Skip);
            let mut running=JoinSet::new();
            let mut next_due=BTreeMap::new();
            let mut inflight=BTreeSet::new();
            loop {
                tokio::select! {
                    _=stopping.changed()=>break,
                    result=running.join_next(),if !running.is_empty()=>{match result {Some(Ok(scope))=>{inflight.remove(&scope);},Some(Err(_))=>{let _=messages.try_send(Message::Event("scheduler","Collector task failed; its scope remains reserved."));},None=>{}}},
                    _=ticks.tick()=>{
                        let snapshot=snapshots.borrow().clone();
                        let mut jobs=jobs(kind,&config,&snapshot,&context.node);
                        next_due.retain(|resource,_|jobs.iter().any(|j|&j.resource==resource));
                        // Never-admitted and longest-waiting scopes go first,
                        // including when the configured fleet overloads a pool.
                        jobs.sort_by_key(|job|next_due.get(&job.resource).copied());
                        for job in jobs {
                            if *stopping.borrow(){break;}
                            let scope=(job.resource.clone(),job.generation.clone());
                            if inflight.contains(&scope){continue;}
                            if next_due.get(&job.resource).is_some_and(|due|*due>tokio::time::Instant::now()){continue;}
                            let (class,timeout)=match kind {Kind::Iokit|Kind::Mounts=>(1,2),Kind::Capacity|Kind::NfsStatus|Kind::MountIdentity=>(2,3),Kind::Smart|Kind::Apfs|Kind::Snapshots=>(0,30),_=>(0,10)};
                            let executable=match kind {Kind::Iokit|Kind::Mounts|Kind::Capacity|Kind::NfsStatus|Kind::MountIdentity=>worker.clone(),Kind::Nfs=>"/usr/bin/nfsstat".into(),Kind::Smart=>match config.tools.smartctl.clone(){Some(p)=>p,None=>continue},_=>"/usr/sbin/diskutil".into()};
                            let stdin=job.request.as_ref().and_then(|r|serde_json::to_vec(r).ok());
                            let args=if job.request.is_some(){vec!["worker".into()]}else{job.args.clone()};
                            let spec=CommandSpec{executable,args,stdin,timeout:Duration::from_secs(timeout),scope:format!("{}:{}:{}",job.collector,job.resource,job.generation)};
                            let stamp=clock.start();
                            let launch=tokio::select! {_=stopping.changed()=>break,result=runners[class].launch(spec)=>result};
                            match launch {
                                Ok(Some(command))=>{
                                    let jitter=seconds*config.collection.jitter_percent as u64*uuid::Uuid::new_v4().as_bytes()[0] as u64/255/100;
                                    next_due.insert(job.resource.clone(),tokio::time::Instant::now()+Duration::from_secs(seconds+jitter));
                                    let _=messages.send(Message::Phase{collector:job.collector.clone(),resource:job.resource.clone(),generation:job.generation.clone(),phase:WorkerPhase::Running,interval:job.interval}).await;
                                    inflight.insert(scope.clone());
                                    let work=collect(job,command,stamp,context.clone(),messages.clone());
                                    running.spawn(async move {work.await;scope});
                                    if matches!(kind,Kind::Inventory){let _=messages.try_send(Message::ReconcileInventory);}
                                },
                                Ok(None)=>{},
                                Err(_)=>{let _=messages.try_send(Message::Event("scheduler","Worker admission failed; scope remains bounded and will retry on its schedule."));}
                            }
                        }
                    }
                }
            }
            running.abort_all();
        })
    }).collect()
}

fn period(kind: Kind, c: &Config) -> u64 {
    match kind {
        Kind::Inventory => c.collection.inventory_reconcile_seconds,
        Kind::Apfs | Kind::Snapshots => c.collection.apfs_accounting_seconds,
        Kind::Iokit => c.collection.io_seconds,
        Kind::Mounts => c.collection.mount_inventory_seconds,
        Kind::Nfs => c.collection.nfs_counters_seconds,
        Kind::NfsStatus => c.collection.nfs_status_seconds,
        Kind::Capacity | Kind::MountIdentity => c.collection.capacity_seconds,
        Kind::Smart => c.collection.smart_seconds,
    }
}

fn jobs(kind: Kind, c: &Config, snapshot: &Heartbeat, node: &str) -> Vec<Job> {
    let interval = period(kind, c);
    let collector = match kind {
        Kind::Inventory => "diskutil.inventory",
        Kind::Apfs => "apfs.accounting",
        Kind::Iokit => "iokit.block",
        Kind::Mounts => "mount.inventory",
        Kind::Nfs => "nfs.client",
        Kind::NfsStatus => "nfs.status",
        Kind::Capacity => "filesystem.capacity",
        Kind::MountIdentity => "mount.identity",
        Kind::Smart => "smartctl",
        Kind::Snapshots => "apfs.snapshots",
    };
    let mut base = Job {
        kind,
        resource: format!("{node}/host"),
        generation: "1".into(),
        collector: collector.into(),
        interval,
        request: None,
        args: Vec::new(),
        mount: None,
    };
    match kind {
        Kind::Inventory => {
            base.args = ["list", "-plist", "physical"]
                .into_iter()
                .map(Into::into)
                .collect()
        }
        Kind::Apfs => {
            base.args = ["apfs", "list", "-plist"]
                .into_iter()
                .map(Into::into)
                .collect()
        }
        Kind::Iokit => base.request = Some(WorkerRequest::Iokit),
        Kind::Mounts => base.request = Some(WorkerRequest::Mounts),
        Kind::Nfs => base.args = ["-f", "JSON", "-c"].into_iter().map(Into::into).collect(),
        _ => {
            return snapshot
                .resources
                .iter()
                .filter_map(|r| {
                    if r.attributes
                        .get("observed_agent_session")
                        .and_then(serde_json::Value::as_str)
                        != Some(snapshot.agent_session_id.as_str())
                    {
                        return None;
                    }
                    let mut job = base.clone();
                    job.resource = r.resource_id.clone();
                    job.generation = r
                        .attributes
                        .get("mount_generation")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("1")
                        .into();
                    match kind {
                        Kind::Capacity | Kind::NfsStatus | Kind::MountIdentity => {
                            if r.resource_type != "mount" {
                                return None;
                            }
                            let nfs = r
                                .attributes
                                .get("filesystem_type")
                                .and_then(serde_json::Value::as_str)
                                == Some("nfs");
                            let fsid: [i32; 2] =
                                serde_json::from_value(r.attributes.get("fsid")?.clone()).ok()?;
                            if matches!(kind, Kind::MountIdentity) {
                                if nfs
                                    || r.attributes
                                        .get("local")
                                        .and_then(serde_json::Value::as_bool)
                                        != Some(true)
                                {
                                    return None;
                                }
                                let source = r.attributes.get("source")?.as_str()?;
                                if !source
                                    .strip_prefix("/dev/")
                                    .is_some_and(crate::platform::valid_media_name)
                                {
                                    return None;
                                }
                                job.mount = Some(r.clone());
                                job.request = Some(WorkerRequest::MountIdentity {
                                    path: r.attributes.get("mount_path")?.as_str()?.into(),
                                    fsid,
                                    source: source.into(),
                                });
                            } else if matches!(kind, Kind::NfsStatus) {
                                if !nfs {
                                    return None;
                                }
                                job.request = Some(WorkerRequest::NfsStatus { fsid });
                            } else {
                                let local = r
                                    .attributes
                                    .get("local")
                                    .and_then(serde_json::Value::as_bool)
                                    .unwrap_or(false);
                                if !local && !c.collection.nfs_path_refresh_enabled {
                                    return None;
                                }
                                job.request = Some(WorkerRequest::Capacity {
                                    path: r.attributes.get("mount_path")?.as_str()?.into(),
                                    fsid,
                                });
                            }
                        }
                        Kind::Smart => {
                            if c.tools.smartctl.is_none()
                                || r.attributes
                                    .get("smart_eligible")
                                    .and_then(serde_json::Value::as_bool)
                                    != Some(true)
                            {
                                return None;
                            }
                            let bsd = r.attributes.get("bsd_name")?.as_str()?;
                            if !valid_disk(bsd) {
                                return None;
                            }
                            // Skipping standby disks makes wake behavior explicit. Status is
                            // still interpreted as SMART's bitmask, not a generic exit code.
                            job.args = vec![
                                "-x".into(),
                                "--json=v".into(),
                                "-n".into(),
                                "standby,2".into(),
                                format!("/dev/{bsd}").into(),
                            ];
                        }
                        Kind::Snapshots => {
                            if r.resource_type != "filesystem"
                                || r.attributes
                                    .get("apfs")
                                    .and_then(serde_json::Value::as_bool)
                                    != Some(true)
                            {
                                return None;
                            }
                            let bsd = r.attributes.get("bsd_name")?.as_str()?;
                            if !valid_disk(bsd) {
                                return None;
                            }
                            job.args = vec![
                                "apfs".into(),
                                "listSnapshots".into(),
                                "-plist".into(),
                                bsd.into(),
                            ];
                        }
                        _ => return None,
                    }
                    Some(job)
                })
                .collect();
        }
    }
    vec![base]
}
fn valid_disk(name: &str) -> bool {
    name.starts_with("disk")
        && name.len() <= 32
        && name[4..].bytes().all(|b| b.is_ascii_digit() || b == b's')
        && !name[4..].is_empty()
}

async fn collect(
    job: Job,
    command: RunningCommand,
    stamp: SampleTime,
    context: Arc<ContextInfo>,
    messages: mpsc::Sender<Message>,
) {
    let mut phase = command.phase;
    let output = match command.result.await {
        Ok(output) => output,
        Err(_) => return,
    };
    let finished_at = now();
    let finished_ns = stamp.finished_ns();
    let pending = *phase.borrow() == WorkerPhase::TimedOutPendingExit;
    let (mut status, code) = match output.outcome {
        CommandOutcome::Completed => ("ok", "completed"),
        CommandOutcome::TimedOut => ("timeout", "deadline"),
        CommandOutcome::OutputLimit => ("timeout", "output_limit"),
        CommandOutcome::Cancelled => ("timeout", "cancelled"),
        CommandOutcome::SpawnFailed => ("failed", "spawn_failed"),
    };
    let exit_code = output.exit_code.and_then(|n| u8::try_from(n).ok());
    let mut collected = None;
    if output.outcome == CommandOutcome::Completed {
        if exit_code != Some(0) && !matches!(job.kind, Kind::Smart) {
            status = "failed";
        } else {
            let parse_job = job.clone();
            let parse_context = context.clone();
            match tokio::task::spawn_blocking(move || parse(&parse_job, &output, &parse_context))
                .await
            {
                Ok(Ok(result)) => collected = Some(result),
                _ => status = "parse_error",
            }
        }
    }
    let update = CompletedJob {
        job: job.clone(),
        collected,
        stamp,
        finished_at,
        finished_ns,
        run_id: format!("{}/{}", context.session, uuid::Uuid::new_v4()),
        status: status.into(),
        code,
        exit_code,
        pending,
        source_version: format!("macOS/{}", context.os_build),
    };
    if messages
        .send(Message::Result(Box::new(update)))
        .await
        .is_err()
    {
        return;
    }
    if pending {
        while *phase.borrow() != WorkerPhase::Idle {
            if phase.changed().await.is_err() {
                return;
            }
        }
    }
    let _ = messages
        .send(Message::Phase {
            collector: job.collector,
            resource: job.resource,
            generation: job.generation,
            phase: WorkerPhase::Idle,
            interval: job.interval,
        })
        .await;
}

fn parse(job: &Job, output: &CommandOutput, context: &ContextInfo) -> Result<Collected> {
    match job.kind {
        Kind::Inventory => {
            collectors::parse_inventory(&output.stdout, &context.node, &context.boot)
        }
        Kind::Apfs => collectors::parse_apfs(&output.stdout, &context.node, &context.boot),
        Kind::Iokit => collectors::parse_iokit(&output.stdout, &context.node, &context.boot),
        Kind::Mounts => collectors::parse_mounts(&output.stdout, &context.node, &context.boot),
        Kind::Nfs => collectors::parse_nfs(&output.stdout, &context.node, &context.boot),
        Kind::NfsStatus => collectors::parse_nfs_status(&output.stdout, &job.resource),
        Kind::Capacity => collectors::parse_capacity(&output.stdout, &job.resource),
        Kind::MountIdentity => collectors::parse_mount_identity(
            &output.stdout,
            job.mount
                .as_ref()
                .context("missing mount identity context")?,
        ),
        Kind::Smart => collectors::parse_smart(
            &output.stdout,
            output
                .exit_code
                .and_then(|n| u8::try_from(n).ok())
                .context("missing SMART exit status")?,
            &job.resource,
            &format!("{}/{}/{}", context.session, job.resource, job.generation),
        ),
        Kind::Snapshots => collectors::parse_snapshots(&output.stdout, &job.resource),
    }
}

pub(crate) fn apply(state: &mut State, mut update: CompletedJob) {
    let envelope = state.snapshot();
    if update.run_id.split('/').next() != Some(envelope.agent_session_id.as_str()) {
        return;
    }
    let group = format!("{}:{}", update.job.collector, update.job.resource);
    let mut accepted_inventory = false;
    if !state.metadata.resources.contains_key(&update.job.resource)
        || state.generation(&update.job.resource) != update.job.generation
    {
        return;
    }
    if let Some(mut collected) = update.collected.take() {
        for resource in &mut collected.resources {
            if matches!(update.job.kind, Kind::MountIdentity) {
                let current = &state.metadata.resources[&update.job.resource];
                let Some(mut identity) = resource.attributes.get("mount_identity").cloned() else {
                    return;
                };
                let same_mount = |value: &serde_json::Value| {
                    value.get("fsid") == current.attributes.get("fsid")
                        && value.get("source") == current.attributes.get("source")
                        && value.get("mount_generation")
                            == current.attributes.get("mount_generation")
                };
                if resource.resource_id != current.resource_id
                    || current.resource_type != "mount"
                    || current.attributes.get("local") != Some(&serde_json::Value::Bool(true))
                    || !same_mount(&identity)
                {
                    return;
                }
                identity["observed_at"] = update.finished_at.clone().into();
                identity["boot_id"] = envelope.boot_id.clone().into();
                identity["agent_session_id"] = envelope.agent_session_id.clone().into();
                // A query failure is not evidence that the established identity
                // disappeared. Keep its acquisition date; a confirmed replacement
                // must instead invalidate it, even before mount inventory catches up.
                if identity["state"] == "unavailable" && identity["reason"] != "mount_replaced" {
                    if let Some(previous) = current.attributes.get("mount_identity").filter(|v| {
                        matches!(v["state"].as_str(), Some("ok" | "stale"))
                            && same_mount(v)
                            && v["boot_id"] == envelope.boot_id
                            && v["agent_session_id"] == envelope.agent_session_id
                    }) {
                        identity = previous.clone();
                        identity["state"] = "stale".into();
                        identity["reason"] = "identity_refresh_failed".into();
                    }
                }
                // The worker carried an admission-time copy for validation. Its
                // enrichment cannot roll back newer accepted source attributes.
                *resource = current.clone();
                resource
                    .attributes
                    .insert("mount_identity".into(), identity);
                continue;
            }
            resource
                .attributes
                .insert("observed_boot_id".into(), envelope.boot_id.clone().into());
            resource.attributes.insert(
                "observed_agent_session".into(),
                update.run_id.split('/').next().unwrap_or("").into(),
            );
            if resource.resource_type == "mount" {
                let generation = state
                    .metadata
                    .resources
                    .get(&resource.resource_id)
                    .and_then(|old| old.attributes.get("mount_generation"))
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
                    .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                resource
                    .attributes
                    .insert("mount_generation".into(), generation.into());
            }
        }
        for resource in &mut collected.resources {
            if matches!(update.job.kind, Kind::MountIdentity) {
                continue;
            }
            if matches!(update.job.kind, Kind::Iokit)
                && resource
                    .attributes
                    .get("media_mapping_state")
                    .and_then(serde_json::Value::as_str)
                    == Some("unavailable")
            {
                if let Some(old) =
                    state
                        .metadata
                        .resources
                        .get(&resource.resource_id)
                        .filter(|old| {
                            old.attributes.get("observed_agent_session")
                                == resource.attributes.get("observed_agent_session")
                                && matches!(
                                    old.attributes
                                        .get("media_mapping_state")
                                        .and_then(serde_json::Value::as_str),
                                    Some("ok" | "stale")
                                )
                        })
                {
                    if let Some(candidates) = old.attributes.get("whole_media_candidates") {
                        resource
                            .attributes
                            .insert("whole_media_candidates".into(), candidates.clone());
                        resource
                            .attributes
                            .insert("media_mapping_state".into(), "stale".into());
                    }
                }
            }
            let semantic = |r: &Resource| {
                let mut a = r.attributes.clone();
                for key in [
                    "association_state",
                    "association_reason_codes",
                    "mount_identity",
                    "source_generation",
                    "media_mapping_state",
                ] {
                    a.remove(key);
                }
                (r.resource_type.clone(), a)
            };
            let generation = state
                .metadata
                .resources
                .get(&resource.resource_id)
                .filter(|old| semantic(old) == semantic(resource))
                .and_then(|old| old.attributes.get("source_generation"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            resource
                .attributes
                .insert("source_generation".into(), generation.into());
        }
        let source_ids: Vec<_> = collected
            .resources
            .iter()
            .map(|r| r.resource_id.clone())
            .collect();
        for edge in &mut collected.relationships {
            let mut a = edge.attributes.take().unwrap_or_default();
            a.insert("source".into(), update.job.collector.clone().into());
            a.insert("boot_id".into(), envelope.boot_id.clone().into());
            a.insert(
                "agent_session_id".into(),
                envelope.agent_session_id.clone().into(),
            );
            edge.attributes = Some(a);
        }
        let generations: BTreeMap<_, _> = collected
            .resources
            .iter()
            .filter_map(|r| {
                r.attributes
                    .get("mount_generation")
                    .and_then(serde_json::Value::as_str)
                    .map(|g| (r.resource_id.clone(), g.to_owned()))
            })
            .collect();
        let has_inventory = matches!(
            update.job.kind,
            Kind::Inventory
                | Kind::Apfs
                | Kind::Iokit
                | Kind::Mounts
                | Kind::MountIdentity
                | Kind::Nfs
                | Kind::Smart
                | Kind::Snapshots
        );
        if has_inventory {
            if state
                .reconcile_owned(
                    &group,
                    &update.job.resource,
                    collected.resources,
                    collected.relationships,
                    collected.complete,
                )
                .is_err()
            {
                state.event("inventory","Inventory update exceeded limits or failed validation; previous inventory is retained.");
                update.status = "parse_error".into();
                collected.samples.clear();
            } else {
                accepted_inventory = true;
                if !matches!(update.job.kind, Kind::MountIdentity) {
                    state.source_evidence(&group, &source_ids, &update.finished_at, true);
                }
                for (id, generation) in generations {
                    state.set_generation(&id, &generation);
                }
            }
        }
        for sample in collected.samples {
            let mut collection = collection(&update, &sample.resource_id, &sample.collector);
            collection.metrics = sample.metrics;
            collection.status = sample.status;
            collection.source_version =
                format!("{}; {}", update.source_version, sample.source_version)
                    .chars()
                    .take(256)
                    .collect();
            collection.exit_code = sample.exit_code.or(update.exit_code);
            let generation = if sample.resource_id == update.job.resource {
                update.job.generation.clone()
            } else {
                state.generation(&sample.resource_id).to_owned()
            };
            if state.apply_collection(collection, &generation).is_err() {
                state.event(
                    "collector",
                    "Collector observation failed contract or cache-limit validation.",
                );
            }
            if state.metadata.resources.contains_key(&sample.resource_id) {
                state.phase(
                    &sample.collector,
                    &sample.resource_id,
                    WorkerPhase::Idle,
                    update.job.interval,
                );
            }
        }
    }
    if !accepted_inventory && update.status != "ok" {
        state.source_evidence(&group, &[], &update.finished_at, false);
        if matches!(update.job.kind, Kind::MountIdentity) {
            if let Some(mut resource) = state.metadata.resources.get(&update.job.resource).cloned()
            {
                if let Some(identity) = resource.attributes.get_mut("mount_identity") {
                    identity["state"] = "stale".into();
                    identity["reason"] = "identity_refresh_failed".into();
                    let _ = state.reconcile_owned(
                        &group,
                        &update.job.resource,
                        vec![resource],
                        vec![],
                        true,
                    );
                }
            }
        }
    }
    if let Err(_) = state.refresh_topology() {
        state.event(
            "topology",
            "Derived storage reconciliation failed; previous dated associations retained.",
        );
    }
    let mut attempt = collection(&update, &update.job.resource, &update.job.collector);
    // A per-resource parser may already have emitted this exact scope. Preserve
    // its measured attempt rather than replacing it with an empty success.
    let already_measured = state
        .snapshot()
        .collections
        .iter()
        .any(|c| c.collection_id == attempt.collection_id);
    if !already_measured {
        if update.status != "ok" {
            attempt.error=Some(CollectionError{domain:"collector".into(),code:update.code.into(),message:"Collector did not produce a usable complete result; previous observations retain their original ages.".into(),exit_code:update.exit_code,signal:None,reasons:None});
        }
        if state
            .apply_collection(attempt, &update.job.generation)
            .is_err()
        {
            state.event("collector", "Collector attempt failed state validation.");
        }
    }
    let last_is_timeout = state
        .snapshot()
        .collector_states
        .iter()
        .find(|s| s.collector == update.job.collector && s.resource_id == update.job.resource)
        .and_then(|s| s.last_attempt_id.as_ref())
        .is_some_and(|id| {
            state
                .snapshot()
                .collections
                .iter()
                .any(|c| &c.collection_id == id && c.status == "timeout")
        });
    let phase = if update.pending {
        if last_is_timeout {
            WorkerPhase::TimedOutPendingExit
        } else {
            WorkerPhase::Running
        }
    } else {
        WorkerPhase::Idle
    };
    state.phase(
        &update.job.collector,
        &update.job.resource,
        phase,
        update.job.interval,
    );
}

fn collection(update: &CompletedJob, resource: &str, collector: &str) -> Collection {
    Collection {
        collection_id: format!(
            "{}/{}",
            update.run_id,
            uuid::Uuid::new_v5(
                &uuid::Uuid::NAMESPACE_OID,
                format!("{resource}/{collector}").as_bytes()
            )
        ),
        resource_id: resource.into(),
        collector: collector.into(),
        adapter_version: env!("CARGO_PKG_VERSION").into(),
        source_version: update.source_version.clone(),
        started_at: update.stamp.started_at.clone(),
        finished_at: update.finished_at.clone(),
        clock_id: update.stamp.clock_id.clone(),
        started_monotonic_ns: update.stamp.started_monotonic_ns.into(),
        finished_monotonic_ns: update.finished_ns.into(),
        status: update.status.clone(),
        metrics: Vec::new(),
        worker_state: Some(
            if update.pending {
                "timed_out_pending_exit"
            } else {
                "exited"
            }
            .into(),
        ),
        error: None,
        exit_code: update.exit_code,
        raw_artifact_sha256: None,
        extensions: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mount_update(bytes: &[u8], clock: &Clock) -> CompletedJob {
        let stamp = clock.start();
        let finished_ns = stamp.finished_ns();
        CompletedJob {
            job: Job {
                kind: Kind::Mounts,
                resource: "node/host".into(),
                generation: "1".into(),
                collector: "mount.inventory".into(),
                interval: 15,
                request: None,
                args: Vec::new(),
                mount: None,
            },
            collected: Some(collectors::parse_mounts(bytes, "node", "boot").unwrap()),
            stamp,
            finished_at: now(),
            finished_ns,
            run_id: format!("session/{}", uuid::Uuid::new_v4()),
            status: "ok".into(),
            code: "completed",
            exit_code: Some(0),
            pending: false,
            source_version: "test".into(),
        }
    }

    const BOOT_MOUNTS: &[u8] = br#"{"version":1,"mounts":[{"fsid":[12,34],"mount_path":"/","mount_path_hex":"2f","source":"/dev/disk3s1s1","source_hex":"2f6465762f6469736b3373317331","filesystem_type":"apfs","local":true,"read_only":true,"capacity":{}}]}"#;

    fn boot_mount_state(clock: &Clock) -> (State, Resource) {
        let mut state = State::new(
            crate::runtime::initial_heartbeat(
                "node", 1, "session", "boot", "test", "test", "test", 5,
            ),
            256,
        );
        apply(&mut state, mount_update(BOOT_MOUNTS, clock));
        let mut volume = mount_update(BOOT_MOUNTS, clock);
        volume.job.kind = Kind::Apfs;
        volume.job.collector = "apfs.accounting".into();
        volume.collected = Some(Collected {
            resources: vec![Resource::new(
                "system-volume".into(),
                "filesystem",
                attrs(
                    serde_json::json!({"source":"diskutil.apfs.list","bsd_name":"disk3s1","reported_uuid":"SYSTEM-VOLUME-UUID"}),
                ),
            )],
            complete: true,
            ..Default::default()
        });
        apply(&mut state, volume);
        let mount = state
            .metadata
            .resources
            .values()
            .find(|r| r.resource_type == "mount")
            .unwrap()
            .clone();
        (state, mount)
    }

    fn identity_update(
        mount: &Resource,
        status: &str,
        reason: Option<&str>,
        clock: &Clock,
    ) -> CompletedJob {
        let mut update = mount_update(BOOT_MOUNTS, clock);
        update.job.kind = Kind::MountIdentity;
        update.job.collector = "mount.identity".into();
        update.job.resource = mount.resource_id.clone();
        update.job.generation = mount.attributes["mount_generation"]
            .as_str()
            .unwrap()
            .into();
        update.job.mount = Some(mount.clone());
        let mut value = serde_json::json!({"state":status,"reason":reason,"source":"/dev/disk3s1s1","fsid":[12,34]});
        if status == "ok" {
            value["volume_uuid"] = "SNAPSHOT-UUID".into();
            value["media_bsd_name"] = "disk3s1s1".into();
            value["parent_volume_uuid"] = "SYSTEM-VOLUME-UUID".into();
            value["parent_media_bsd_name"] = "disk3s1".into();
            value["parent_media_registry_id"] = "42".into();
        }
        update.collected = Some(
            collectors::parse_mount_identity(&serde_json::to_vec(&value).unwrap(), mount).unwrap(),
        );
        update
    }

    #[test]
    fn unavailable_mount_identity_retains_dated_boot_association() {
        let clock = Clock::new("session");
        let (mut state, mount) = boot_mount_state(&clock);
        apply(&mut state, identity_update(&mount, "ok", None, &clock));
        let old = state.metadata.resources[&mount.resource_id].attributes["mount_identity"].clone();
        let edge = state
            .metadata
            .relationships
            .values()
            .find(|r| r.relation == "mounts")
            .unwrap()
            .clone();
        for _ in 0..2 {
            apply(
                &mut state,
                identity_update(&mount, "unavailable", Some("identity_query_failed"), &clock),
            );
            let retained =
                &state.metadata.resources[&mount.resource_id].attributes["mount_identity"];
            assert_eq!(retained["state"], "stale");
            assert_eq!(retained["parent_volume_uuid"], "SYSTEM-VOLUME-UUID");
            assert_eq!(retained["observed_at"], old["observed_at"]);
            let retained_edge = &state.metadata.relationships[&edge.relationship_id];
            assert_eq!(retained_edge.observed_at, edge.observed_at);
            assert_eq!(retained_edge.attributes.as_ref().unwrap()["state"], "stale");
        }
        apply(&mut state, identity_update(&mount, "ok", None, &clock));
        assert_eq!(
            state.metadata.relationships[&edge.relationship_id]
                .attributes
                .as_ref()
                .unwrap()["state"],
            "resolved"
        );
    }

    #[test]
    fn mount_identity_completion_preserves_newer_source_attributes() {
        let clock = Clock::new("session");
        let (mut state, mount) = boot_mount_state(&clock);
        let in_flight = identity_update(&mount, "ok", None, &clock);
        let mut inventory = mount_update(BOOT_MOUNTS, &clock);
        inventory.collected.as_mut().unwrap().resources[0]
            .attributes
            .insert(
                "inventory_annotation".into(),
                "newer accepted source data".into(),
            );
        apply(&mut state, inventory);
        let source_generation =
            state.metadata.resources[&mount.resource_id].attributes["source_generation"].clone();
        assert_ne!(source_generation, mount.attributes["source_generation"]);
        apply(&mut state, in_flight);
        let current = &state.metadata.resources[&mount.resource_id];
        assert_eq!(
            current
                .attributes
                .get("inventory_annotation")
                .and_then(serde_json::Value::as_str),
            Some("newer accepted source data")
        );
        assert_eq!(current.attributes["source_generation"], source_generation);
        assert_eq!(current.attributes["association_state"], "resolved");
        assert!(state
            .metadata
            .relationships
            .values()
            .any(|r| r.relation == "mounts"));
    }

    #[test]
    fn confirmed_mount_replacement_does_not_retain_identity() {
        let clock = Clock::new("session");
        let (mut state, mount) = boot_mount_state(&clock);
        apply(&mut state, identity_update(&mount, "ok", None, &clock));
        apply(
            &mut state,
            identity_update(&mount, "unavailable", Some("mount_replaced"), &clock),
        );
        assert!(!state
            .metadata
            .relationships
            .values()
            .any(|r| r.relation == "mounts"));
        let late = identity_update(&mount, "ok", None, &clock);
        let mut empty = mount_update(BOOT_MOUNTS, &clock);
        empty.collected = Some(Collected {
            complete: true,
            ..Default::default()
        });
        apply(&mut state, empty);
        assert!(!state.metadata.resources.contains_key(&mount.resource_id));
        apply(&mut state, mount_update(BOOT_MOUNTS, &clock));
        assert_ne!(
            state.generation(&mount.resource_id),
            mount.attributes["mount_generation"].as_str().unwrap()
        );
        apply(&mut state, late);
        assert!(!state.metadata.resources[&mount.resource_id]
            .attributes
            .contains_key("mount_identity"));
        assert!(!state
            .metadata
            .relationships
            .values()
            .any(|r| r.relation == "mounts"));
    }

    #[test]
    fn accepted_sources_reconcile_in_both_orders_and_failure_keeps_stale_edge() {
        let clock = Clock::new("session");
        for reverse in [false, true] {
            let mut state = State::new(
                crate::runtime::initial_heartbeat(
                    "node", 1, "session", "boot", "test", "test", "test", 5,
                ),
                256,
            );
            let mut disk = mount_update(include_bytes!("../tests/fixtures/mounts.json"), &clock);
            disk.job.kind = Kind::Inventory;
            disk.job.collector = "diskutil.inventory".into();
            disk.collected = Some(Collected {
                resources: vec![Resource::new(
                    "physical".into(),
                    "physical_device",
                    attrs(
                        serde_json::json!({"bsd_name":"disk0","source":"diskutil.list.physical"}),
                    ),
                )],
                complete: true,
                ..Default::default()
            });
            let mut driver = mount_update(include_bytes!("../tests/fixtures/mounts.json"), &clock);
            driver.job.kind = Kind::Iokit;
            driver.job.collector = "iokit.block".into();
            driver.collected = Some(Collected {
                resources: vec![Resource::new(
                    "driver".into(),
                    "controller",
                    attrs(
                        serde_json::json!({"source":"IOBlockStorageDriver","media_mapping_state":"ok","whole_media_candidates":[{"bsd_name":"disk0","registry_entry_id":"42","whole":true}]}),
                    ),
                )],
                complete: true,
                ..Default::default()
            });
            if reverse {
                apply(&mut state, driver);
                apply(&mut state, disk);
            } else {
                apply(&mut state, disk);
                apply(&mut state, driver);
            }
            let edge = state
                .metadata
                .relationships
                .values()
                .find(|e| e.relation == "attached_to")
                .expect("source arrival must establish driver mapping")
                .clone();
            let mut mapping_failed =
                mount_update(include_bytes!("../tests/fixtures/mounts.json"), &clock);
            mapping_failed.job.kind = Kind::Iokit;
            mapping_failed.job.collector = "iokit.block".into();
            mapping_failed.collected = Some(Collected {
                resources: vec![Resource::new(
                    "driver".into(),
                    "controller",
                    attrs(
                        serde_json::json!({"source":"IOBlockStorageDriver","media_mapping_state":"unavailable","whole_media_candidates":[]}),
                    ),
                )],
                complete: true,
                ..Default::default()
            });
            apply(&mut state, mapping_failed);
            assert_eq!(
                state
                    .metadata
                    .relationships
                    .get(&edge.relationship_id)
                    .expect("optional mapping failure retains dated association")
                    .attributes
                    .as_ref()
                    .unwrap()["state"],
                "stale"
            );
            let mut failed = mount_update(include_bytes!("../tests/fixtures/mounts.json"), &clock);
            failed.job.kind = Kind::Inventory;
            failed.job.collector = "diskutil.inventory".into();
            failed.collected = None;
            failed.status = "failed".into();
            apply(&mut state, failed);
            let stale = &state.metadata.relationships[&edge.relationship_id];
            assert_eq!(stale.attributes.as_ref().unwrap()["state"], "stale");
            assert_eq!(stale.observed_at, edge.observed_at);
            assert!(state.metadata.groups["derived.storage"]
                .iter()
                .all(|id| state.metadata.relationships.contains_key(id)));
        }
    }

    #[test]
    fn physical_apfs_driver_sources_reconcile_all_six_arrival_orders() {
        let clock = Clock::new("session");
        for order in [
            [0, 1, 2],
            [0, 2, 1],
            [1, 0, 2],
            [1, 2, 0],
            [2, 0, 1],
            [2, 1, 0],
        ] {
            let mut state = State::new(
                crate::runtime::initial_heartbeat(
                    "node", 1, "session", "boot", "test", "test", "test", 5,
                ),
                256,
            );
            for source in order {
                let mut update =
                    mount_update(include_bytes!("../tests/fixtures/mounts.json"), &clock);
                let (kind, collector, mut collected) = match source {
                    0 => (
                        Kind::Inventory,
                        "diskutil.inventory",
                        collectors::parse_inventory(
                            include_bytes!("../tests/fixtures/diskutil-physical.plist"),
                            "node",
                            "boot",
                        )
                        .unwrap(),
                    ),
                    1 => (
                        Kind::Apfs,
                        "apfs.accounting",
                        collectors::parse_apfs(
                            include_bytes!("../tests/fixtures/diskutil-apfs.plist"),
                            "node",
                            "boot",
                        )
                        .unwrap(),
                    ),
                    _ => (
                        Kind::Iokit,
                        "iokit.block",
                        collectors::parse_iokit(
                            include_bytes!("../tests/fixtures/iokit-whole-media.plist"),
                            "node",
                            "boot",
                        )
                        .unwrap(),
                    ),
                };
                // Add a second partition/store, backed by the other physical disk.
                if source < 2 {
                    let store_id =
                        collectors::scoped_id("node", "boot", "diskutil-media", "disk1s2");
                    let parent = collected
                        .resources
                        .iter()
                        .find(|r| {
                            if source == 0 {
                                r.resource_type == "physical_device"
                                    && r.attributes
                                        .get("bsd_name")
                                        .and_then(serde_json::Value::as_str)
                                        == Some("disk1")
                            } else {
                                r.resource_type == "apfs_container"
                            }
                        })
                        .unwrap()
                        .resource_id
                        .clone();
                    collected.resources.push(Resource::new(store_id.clone(),"media",attrs(serde_json::json!({"bsd_name":"disk1s2","source":if source==0 {"diskutil.list.physical"}else{"diskutil.apfs.list"}}))));
                    collected.relationships.push(Relationship {
                        relationship_id: format!("second-store-{source}"),
                        revision: 1u64.into(),
                        observed_at: now(),
                        from_resource_id: parent,
                        to_resource_id: store_id,
                        relation: if source == 0 { "contains" } else { "backed_by" }.into(),
                        attributes: None,
                    });
                }
                update.job.kind = kind;
                update.job.collector = collector.into();
                update.collected = Some(collected);
                apply(&mut state, update);
            }
            assert_eq!(
                state
                    .metadata
                    .relationships
                    .values()
                    .filter(|r| r.relation == "attached_to")
                    .count(),
                1
            );
            let container = state
                .metadata
                .resources
                .values()
                .find(|r| r.resource_type == "apfs_container")
                .unwrap();
            assert_eq!(
                state
                    .metadata
                    .relationships
                    .values()
                    .filter(|r| r.from_resource_id == container.resource_id
                        && r.relation == "backed_by")
                    .count(),
                2
            );
            let mut removed = mount_update(include_bytes!("../tests/fixtures/mounts.json"), &clock);
            removed.job.kind = Kind::Inventory;
            removed.job.collector = "diskutil.inventory".into();
            removed.collected = Some(Collected {
                complete: true,
                ..Default::default()
            });
            apply(&mut state, removed);
            assert!(!state
                .metadata
                .relationships
                .values()
                .any(|r| r.relation == "attached_to"));
            assert!(state
                .metadata
                .resources
                .values()
                .any(|r| r.resource_type == "controller"));
        }
    }

    #[test]
    fn observed_mount_reappearance_starts_a_new_incarnation() {
        let clock = Clock::new("session");
        let mut state = State::new(
            crate::runtime::initial_heartbeat(
                "node", 1, "session", "boot", "test", "test", "test", 15,
            ),
            256,
        );
        let fixture = include_bytes!("../tests/fixtures/mounts.json");
        let first = mount_update(fixture, &clock);
        let id = first
            .collected
            .as_ref()
            .unwrap()
            .resources
            .iter()
            .find(|r| r.resource_type == "mount")
            .unwrap()
            .resource_id
            .clone();
        apply(&mut state, first);
        let generation = state.generation(&id).to_owned();
        let mut empty = mount_update(fixture, &clock);
        let collected = empty.collected.as_mut().unwrap();
        collected.resources.clear();
        collected.relationships.clear();
        collected.samples.clear();
        apply(&mut state, empty);
        assert!(!state.metadata.resources.contains_key(&id));
        apply(&mut state, mount_update(fixture, &clock));
        assert_ne!(state.generation(&id), generation);
        let snapshot = state.snapshot();
        let mut late = snapshot
            .collections
            .iter()
            .find(|c| c.resource_id == id)
            .unwrap()
            .clone();
        late.collection_id = "old-worker-reply".into();
        assert!(!state.apply_collection(late, &generation).unwrap());
        let mut late_identity = mount_update(fixture, &clock);
        late_identity.job.kind = Kind::MountIdentity;
        late_identity.job.collector = "mount.identity".into();
        late_identity.job.resource = id.clone();
        late_identity.job.generation = generation;
        let mut old = state.metadata.resources[&id].clone();
        old.attributes
            .insert("mount_identity".into(), serde_json::json!({"state":"ok"}));
        late_identity.collected = Some(Collected {
            resources: vec![old],
            complete: true,
            ..Default::default()
        });
        apply(&mut state, late_identity);
        assert!(!state.metadata.resources[&id]
            .attributes
            .contains_key("mount_identity"));
    }
}
