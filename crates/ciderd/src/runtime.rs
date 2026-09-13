//! Foreground daemon orchestration. State, collection, persistence, and HTTP
//! have independent tasks and bounded channels.
use crate::{
    clock::Clock,
    command::CommandRunner,
    config::{read_bounded, Config},
    heartbeat::{bearer_header, FailureKind, RetryPolicy, Sender},
    identity::{atomic_write, Session},
    model::*,
    platform, scheduler,
    state::{self, Metadata, State},
};
use anyhow::{ensure, Context, Result};
use reqwest::header::HeaderValue;
use std::{
    future::Future,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::{
    sync::{mpsc, watch},
    task::JoinHandle,
};

pub(crate) enum Message {
    Result(Box<scheduler::CompletedJob>),
    Phase {
        collector: String,
        resource: String,
        generation: String,
        phase: WorkerPhase,
        interval: u64,
    },
    Ack {
        sent: Arc<Heartbeat>,
        ack: Acknowledgement,
    },
    Event(&'static str, &'static str),
    ReconcileInventory,
}

#[allow(clippy::too_many_arguments)]
pub fn initial_heartbeat(
    node: &str,
    generation: u64,
    session: &str,
    boot: &str,
    os_version: &str,
    os_build: &str,
    target: &str,
    interval: u64,
) -> Heartbeat {
    Heartbeat {
        schema_version: "2.0".into(),
        message_type: "heartbeat".into(),
        node_id: node.into(),
        boot_id: boot.into(),
        agent_session_id: session.into(),
        agent_generation: generation.into(),
        sequence: 0u64.into(),
        created_at: now(),
        clock_id: format!("{session}/monotonic/0"),
        monotonic_ns: 0u64.into(),
        agent: AgentInfo {
            version: env!("CARGO_PKG_VERSION").into(),
            target: target.into(),
            os_version: os_version.into(),
            os_build: os_build.into(),
            delivery_mode: "latest".into(),
            heartbeat_interval_seconds: interval,
            quarantined_workers: 0,
            discarded_samples_total: 0u64.into(),
            dropped_events_total: 0u64.into(),
            payload_limited: false,
        },
        inventory: InventoryInfo {
            revision: 1u64.into(),
            included: true,
        },
        resources: vec![Resource::new(
            format!("{node}/host"),
            "host",
            attrs(
                serde_json::json!({"os_version":os_version,"os_build":os_build,"architecture":std::env::consts::ARCH}),
            ),
        )],
        relationships: Vec::new(),
        collections: Vec::new(),
        collector_states: Vec::new(),
        events: Vec::new(),
        tombstones: Vec::new(),
        extensions: None,
    }
}

struct Engine {
    snapshots: watch::Receiver<Arc<Heartbeat>>,
    messages: mpsc::Sender<Message>,
    clock: Arc<Clock>,
    stop: watch::Sender<bool>,
    tasks: Vec<JoinHandle<()>>,
    runners: Vec<Arc<CommandRunner>>,
}

impl Engine {
    async fn start(
        config: &Config,
        envelope: Heartbeat,
        worker: PathBuf,
        persistent: bool,
    ) -> Result<Self> {
        let clock = Arc::new(Clock::new(&envelope.agent_session_id));
        let (stop, stopping) = watch::channel(false);
        let mut state = State::new(envelope.clone(), config.limits.pending_events);
        let metadata_path = config.node.state_directory.join("inventory.json");
        if persistent {
            match read_bounded(&metadata_path,state::MAX_METADATA_BYTES) {
                Ok(bytes)=>{
                    let metadata:Metadata=parse_json(&bytes).context("invalid persisted inventory; explicit recovery required")?;
                    state.restore(metadata)?;
                    // Preserve restored graph as dated knowledge, update the host's OS context.
                    state.reconcile("host",envelope.resources.clone(),Vec::new(),true)?;
                },
                Err(error) if envelope.agent_generation.get()>1=>return Err(error.context("inventory metadata missing after enrollment; recover metadata or re-enroll with a new node ID")),
                Err(_)=>atomic_write(&metadata_path,&serde_json::to_vec(&state.metadata)?)?,
            }
        }
        let mut runners = Vec::new();
        for (class, count) in [
            ("command", config.limits.command_workers),
            ("native", config.limits.native_workers),
            ("remote", config.limits.remote_workers),
        ] {
            runners.push(
                CommandRunner::new(
                    count,
                    config.limits.stdout_bytes,
                    config.limits.stderr_bytes,
                    Some(config.node.state_directory.join("jobs").join(class)),
                    envelope.boot_id.clone(),
                )
                .await?,
            );
        }
        let (messages, receiver) = mpsc::channel(64);
        let (publication, snapshots) = watch::channel(Arc::new(state.snapshot()));
        let (metadata_tx, metadata_rx) = watch::channel(Arc::new(state.metadata.clone()));
        let actor = tokio::spawn(state_owner(state, receiver, publication, metadata_tx));
        let mut tasks = vec![actor];
        if persistent {
            tasks.push(tokio::spawn(persist_metadata(
                metadata_path,
                metadata_rx,
                messages.clone(),
                stopping.clone(),
            )));
        }
        tasks.extend(scheduler::start(
            config.clone(),
            worker,
            envelope.node_id,
            envelope.boot_id,
            envelope.agent_session_id,
            envelope.agent.os_build,
            clock.clone(),
            runners.clone(),
            snapshots.clone(),
            messages.clone(),
            stopping,
        ));
        Ok(Self {
            snapshots,
            messages,
            clock,
            stop,
            tasks,
            runners,
        })
    }
    fn prepared(&self, sequence: u64, limit: usize) -> Result<Heartbeat> {
        let published = self.snapshots.borrow().clone();
        let mut snapshot = published.as_ref().clone();
        snapshot.agent.quarantined_workers = self
            .runners
            .iter()
            .map(|r| r.recovered_count() + r.quarantined_count())
            .sum::<usize>();
        let (id, ns) = self.clock.read();
        state::prepare(snapshot, sequence, ns, &id, limit)
    }
    async fn shutdown(mut self) -> usize {
        self.stop.send_replace(true);
        for runner in &self.runners {
            runner.shutdown();
        }
        drop(self.messages);
        // Reapers retain durable active-job records if exit cannot be confirmed.
        let joined = async {
            for task in &mut self.tasks {
                let _ = task.await;
            }
            while self.runners.iter().any(|r| r.pending_count() > 0) {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        };
        let _ = tokio::time::timeout(Duration::from_secs(2), joined).await;
        for task in self.tasks {
            task.abort();
        }
        self.runners.iter().map(|r| r.pending_count()).sum()
    }
}

async fn state_owner(
    mut state: State,
    mut messages: mpsc::Receiver<Message>,
    publication: watch::Sender<Arc<Heartbeat>>,
    metadata: watch::Sender<Arc<Metadata>>,
) {
    while let Some(message) = messages.recv().await {
        let old_revision = state.snapshot().inventory.revision;
        let old_tombstones = state.metadata.tombstones.len();
        match message {
            Message::Result(job) => scheduler::apply(&mut state, *job),
            Message::Phase {
                collector,
                resource,
                generation,
                phase,
                interval,
            } => {
                if state.metadata.resources.contains_key(&resource)
                    && state.generation(&resource) == generation
                {
                    state.phase(&collector, &resource, phase, interval);
                }
            }
            Message::Ack { sent, ack } => {
                if state.acknowledge(&sent, &ack).is_err() {
                    state.event(
                        "heartbeat",
                        "Acknowledgement did not match the active session.",
                    );
                }
            }
            Message::Event(source, message) => state.event(source, message),
            Message::ReconcileInventory => state.request_inventory(),
        }
        let snapshot = state.snapshot();
        if snapshot.inventory.revision != old_revision
            || state.metadata.tombstones.len() != old_tombstones
        {
            metadata.send_replace(Arc::new(state.metadata.clone()));
        }
        publication.send_replace(Arc::new(snapshot));
    }
}

async fn persist_metadata(
    path: PathBuf,
    mut latest: watch::Receiver<Arc<Metadata>>,
    messages: mpsc::Sender<Message>,
    mut stopping: watch::Receiver<bool>,
) {
    let mut dirty = false;
    let mut retry_seconds = 1;
    loop {
        if dirty {
            tokio::select! {
                _=stopping.changed()=>break,
                result=latest.changed()=>if result.is_err(){break;},
                _=tokio::time::sleep(Duration::from_secs(retry_seconds))=>{},
            }
        } else {
            tokio::select! {
                _=stopping.changed()=>break,
                result=latest.changed()=>if result.is_err(){break;},
            }
        }
        let snapshot = latest.borrow_and_update().clone();
        let file = path.clone();
        let task = tokio::task::spawn_blocking(move || -> Result<()> {
            let bytes = serde_json::to_vec(&snapshot)?;
            ensure!(
                bytes.len() <= state::MAX_METADATA_BYTES,
                "metadata too large"
            );
            atomic_write(&file, &bytes)
        });
        // Only one persistence operation can exist, including one stuck in local I/O.
        if !matches!(task.await, Ok(Ok(()))) {
            let _ = messages.try_send(Message::Event(
                "persistence",
                "Inventory persistence failed; the latest metadata remains dirty and will retry.",
            ));
            if dirty {
                retry_seconds = (retry_seconds * 2).min(30);
            }
            dirty = true;
        } else {
            dirty = false;
            retry_seconds = 1;
        }
    }
}

/// Collect a short offline snapshot. It never reads credentials or sends HTTP.
pub async fn snapshot(config: Config, window: Duration) -> Result<Heartbeat> {
    snapshot_with_worker(config, window, std::env::current_exe()?).await
}

/// Embedded callers provide the deployed `ciderd` executable for workers.
pub async fn snapshot_with_worker(
    mut config: Config,
    window: Duration,
    worker: PathBuf,
) -> Result<Heartbeat> {
    config.validate()?;
    ensure!(
        (Duration::from_secs(1)..=Duration::from_secs(60)).contains(&window),
        "snapshot duration must be 1..60 seconds"
    );
    // Preview runs share a local admission journal, so repeated previews cannot
    // reset the budget for a worker which survived the preceding cleanup.
    let preview_directory =
        std::env::temp_dir().join(format!("ciderd-preview-{}", platform::effective_uid()));
    let preview_lock = preview_lock(&preview_directory)?;
    config.node.state_directory = preview_directory;
    let info = platform::system_info()?;
    let session = uuid::Uuid::new_v4().to_string();
    let node = format!("preview-{}", uuid::Uuid::new_v4());
    let mut envelope = initial_heartbeat(
        &node,
        0,
        &session,
        &info.boot_id,
        &info.os_version,
        &info.os_build,
        &info.target,
        config.heartbeat.interval_seconds,
    );
    envelope.resources[0]
        .attributes
        .extend(crate::hardware::hardware_attributes(
            info.model_identifier,
            &envelope.created_at,
        ));
    let engine = Engine::start(&config, envelope, worker, false).await?;
    tokio::time::sleep(window).await;
    let result = engine.prepared(1, config.heartbeat.maximum_request_bytes);
    let pending = engine.shutdown().await;
    let mut result = result?;
    result.agent.quarantined_workers = pending;
    if pending > 0 {
        result.events.push(Event{event_id:uuid::Uuid::new_v4().to_string(),resource_id:format!("{node}/host"),observed_at:now(),source:"preview.shutdown".into(),category:"collector".into(),severity:"warning".into(),message:"Workers survived the cleanup budget; their journals reserve slots in subsequent previews.".into(),details:None,raw_artifact_sha256:None});
    }
    drop(preview_lock);
    let clock_id = result.clock_id.clone();
    let ns = result.monotonic_ns.get();
    state::prepare(
        result,
        1,
        ns,
        &clock_id,
        config.heartbeat.maximum_request_bytes,
    )
}

fn preview_lock(directory: &Path) -> Result<std::fs::File> {
    use fs2::FileExt;
    #[cfg(unix)]
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    builder.mode(0o700);
    if let Err(error) = builder.create(directory) {
        if error.kind() != std::io::ErrorKind::AlreadyExists {
            return Err(error.into());
        }
    }
    let metadata = std::fs::symlink_metadata(directory)?;
    ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "invalid preview journal directory"
    );
    #[cfg(unix)]
    ensure!(
        metadata.uid() == platform::effective_uid() && metadata.mode() & 0o077 == 0,
        "preview journal directory must be private and owned by this user"
    );
    let mut options = std::fs::OpenOptions::new();
    options.create(true).write(true).truncate(false);
    #[cfg(unix)]
    options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    let lock = options.open(directory.join("preview.lock"))?;
    lock.try_lock_exclusive()
        .context("another offline snapshot is running")?;
    Ok(lock)
}

/// Run until the supplied shutdown future resolves. No launchd installation occurs.
pub async fn run(config: Config, shutdown: impl Future<Output = ()>) -> Result<()> {
    run_with_worker(config, std::env::current_exe()?, shutdown).await
}

pub async fn run_with_worker(
    config: Config,
    worker: PathBuf,
    shutdown: impl Future<Output = ()>,
) -> Result<()> {
    config.validate()?;
    let info = platform::system_info()?;
    let authorization = read_credential(&config.heartbeat.bearer_token_file)?;
    let sender = Sender::new(config.heartbeat.clone())?;
    let session = Session::open(&config.node.identity_file, &config.node.state_directory)?;
    let mut envelope = initial_heartbeat(
        &session.node_id,
        session.generation,
        &session.session_id,
        &info.boot_id,
        &info.os_version,
        &info.os_build,
        &info.target,
        config.heartbeat.interval_seconds,
    );
    envelope.resources[0]
        .attributes
        .extend(crate::hardware::hardware_attributes(
            info.model_identifier,
            &envelope.created_at,
        ));
    let engine = Engine::start(&config, envelope, worker, true).await?;
    let (credentials, credential_rx) = watch::channel(authorization);
    let rotation = tokio::spawn(rotate_credentials(
        config.heartbeat.bearer_token_file.clone(),
        credentials,
        engine.messages.clone(),
        engine.stop.subscribe(),
    ));
    let mut sequence = 0u64;
    let mut retry = RetryPolicy::new(
        Duration::from_secs(config.heartbeat.interval_seconds),
        Duration::from_secs(config.heartbeat.maximum_backoff_seconds),
    );
    let mut delay = Duration::ZERO;
    let mut budget = config.heartbeat.maximum_request_bytes;
    tokio::pin!(shutdown);
    let outcome = loop {
        tokio::select! { _=&mut shutdown=>break Ok(()),_=tokio::time::sleep(delay)=>{} }
        let attempt_started = tokio::time::Instant::now();
        sequence = match sequence.checked_add(1) {
            Some(n) => n,
            None => break Err(anyhow::anyhow!("heartbeat sequence exhausted")),
        };
        let heartbeat = match engine.prepared(sequence, budget) {
            Ok(h) => Arc::new(h),
            Err(_) => {
                let _ = engine.messages.try_send(Message::Event(
                    "heartbeat",
                    "Published snapshot failed contract or byte-limit validation.",
                ));
                delay = retry.failed(None);
                continue;
            }
        };
        let credential = credential_rx.borrow().clone();
        let result = tokio::select! {_=&mut shutdown=>break Ok(()),result=sender.send(&heartbeat,&credential)=>result};
        match result {
            Ok(ack) => {
                let _ = engine.messages.try_send(Message::Ack {
                    sent: heartbeat,
                    ack,
                });
                retry.succeeded();
                let period = Duration::from_secs(config.heartbeat.interval_seconds);
                delay = period
                    .checked_sub(attempt_started.elapsed())
                    .unwrap_or(period);
            }
            Err(error) => {
                let message = match error.kind {
                    FailureKind::Authentication => "Heartbeat credentials were rejected.",
                    FailureKind::Contract | FailureKind::InvalidAcknowledgement => {
                        "Heartbeat contract or acknowledgement was rejected."
                    }
                    FailureKind::PayloadTooLarge => {
                        budget = (budget / 2).max(4096);
                        "Heartbeat payload was too large; sending bounded summaries until configuration is corrected."
                    }
                    FailureKind::RateLimited => "Heartbeat peer requested a rate-limit pause.",
                    FailureKind::ResponseTooLarge => {
                        "Heartbeat acknowledgement exceeded its byte limit."
                    }
                    _ => "Heartbeat delivery failed; collectors continue independently.",
                };
                let _ = engine
                    .messages
                    .try_send(Message::Event("heartbeat", message));
                delay = retry.failed(error.retry_after);
            }
        }
    };
    // Stop admitting collection before the best-effort final bounded POST.
    engine.stop.send_replace(true);
    for runner in &engine.runners {
        runner.shutdown();
    }
    if let Some(next) = sequence.checked_add(1) {
        if let Ok(final_heartbeat) = engine.prepared(next, budget) {
            let credential = credential_rx.borrow().clone();
            let _ = tokio::time::timeout(
                Duration::from_secs(1),
                sender.send(&final_heartbeat, &credential),
            )
            .await;
        }
    }
    rotation.abort();
    engine.shutdown().await;
    drop(session);
    outcome
}

fn read_credential(path: &Path) -> Result<HeaderValue> {
    let metadata = std::fs::symlink_metadata(path).context("cannot inspect credential file")?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "credential must be a regular file"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ensure!(
            metadata.mode() & 0o077 == 0,
            "credential file must be owner-only (0600)"
        );
        ensure!(
            metadata.uid() == platform::effective_uid(),
            "credential must belong to the daemon user"
        );
    }
    let bytes = read_bounded(path, 4097)?;
    let token = std::str::from_utf8(&bytes)
        .context("credential is not ASCII")?
        .trim_end_matches(['\r', '\n']);
    bearer_header(token)
}

async fn rotate_credentials(
    path: PathBuf,
    credentials: watch::Sender<HeaderValue>,
    messages: mpsc::Sender<Message>,
    mut stopping: watch::Receiver<bool>,
) {
    loop {
        tokio::select! {_=stopping.changed()=>break,_=tokio::time::sleep(Duration::from_secs(30))=>{}}
        let path = path.clone();
        match tokio::task::spawn_blocking(move || read_credential(&path)).await {
            Ok(Ok(value)) => {
                credentials.send_replace(value);
            }
            _ => {
                let _ = messages.try_send(Message::Event(
                    "credentials",
                    "Credential reload failed; the last validated credential remains active.",
                ));
            }
        }
    }
}

/// Check install-time path ownership without traversing any observed mount path.
pub fn validate_installation(
    config: &Config,
    worker: &Path,
    allow_unprivileged: bool,
) -> Result<()> {
    let uid = platform::effective_uid();
    ensure!(
        uid == 0 || allow_unprivileged,
        "run requires root; use --allow-unprivileged for local development"
    );
    let mut paths = vec![
        worker,
        &config.node.identity_file,
        &config.node.state_directory,
        &config.heartbeat.bearer_token_file,
    ];
    if let Some(tool) = &config.tools.smartctl {
        paths.push(tool);
    }
    if let Some(ca) = &config.heartbeat.ca_certificate_file {
        paths.push(ca);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        for path in paths {
            ensure!(path.is_absolute(), "deployment paths must be absolute");
            let metadata = std::fs::symlink_metadata(path).context("deployment path missing")?;
            ensure!(
                !metadata.file_type().is_symlink(),
                "deployment paths cannot be symlinks"
            );
            ensure!(
                metadata.uid() == uid || metadata.uid() == 0,
                "deployment path has a different owner"
            );
            ensure!(
                metadata.mode() & 0o022 == 0,
                "deployment path is group/world writable"
            );
            if uid == 0 {
                for parent in path.ancestors().skip(1) {
                    let meta = std::fs::symlink_metadata(parent)?;
                    ensure!(
                        meta.is_dir() && meta.uid() == 0 && meta.mode() & 0o022 == 0,
                        "root deployment requires root-owned, non-writable parent directories"
                    );
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn unchanged_metadata_retries_after_local_storage_recovers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing/inventory.json");
        let (updates, latest) = watch::channel(Arc::new(Metadata::default()));
        let (messages, mut events) = mpsc::channel(4);
        let (stop, stopping) = watch::channel(false);
        let task = tokio::spawn(persist_metadata(path.clone(), latest, messages, stopping));
        updates.send_replace(Arc::new(Metadata::default()));
        tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .unwrap()
            .unwrap();
        std::fs::create_dir(path.parent().unwrap()).unwrap();
        // No new watch update: recovery alone must allow the pending write.
        let result = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if let Ok(bytes) = tokio::fs::read(&path).await {
                    parse_json::<Metadata>(&bytes).unwrap();
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await;
        stop.send_replace(true);
        task.abort();
        assert!(
            result.is_ok(),
            "dirty metadata must retry without an inventory change"
        );
    }
}
