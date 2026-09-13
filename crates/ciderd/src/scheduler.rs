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
    [Kind::Inventory,Kind::Apfs,Kind::Iokit,Kind::Mounts,Kind::Nfs,Kind::NfsStatus,Kind::Capacity,Kind::Smart,Kind::Snapshots].into_iter().map(|kind| {
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
                            let (class,timeout)=match kind {Kind::Iokit|Kind::Mounts=>(1,2),Kind::Capacity|Kind::NfsStatus=>(2,3),Kind::Smart|Kind::Apfs|Kind::Snapshots=>(0,30),_=>(0,10)};
                            let executable=match kind {Kind::Iokit|Kind::Mounts|Kind::Capacity|Kind::NfsStatus=>worker.clone(),Kind::Nfs=>"/usr/bin/nfsstat".into(),Kind::Smart=>match config.tools.smartctl.clone(){Some(p)=>p,None=>continue},_=>"/usr/sbin/diskutil".into()};
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
        Kind::Capacity => c.collection.capacity_seconds,
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
                        Kind::Capacity | Kind::NfsStatus => {
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
                            if matches!(kind, Kind::NfsStatus) {
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
    if !state.metadata.resources.contains_key(&update.job.resource)
        || state.generation(&update.job.resource) != update.job.generation
    {
        return;
    }
    if let Some(mut collected) = update.collected.take() {
        for resource in &mut collected.resources {
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
                | Kind::Nfs
                | Kind::Smart
                | Kind::Snapshots
        );
        if has_inventory {
            let group = format!("{}:{}", update.job.collector, update.job.resource);
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
    }
}
