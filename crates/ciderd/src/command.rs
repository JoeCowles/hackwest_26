//! Bounded command admission and supervisors that own children through reaping.
//!
//! A deadline ends the observation, not ownership of the process. Journal writes
//! run on blocking workers outside the heartbeat task. Each work class gets its
//! own runner and journal directory; scopes share that class's admission budget.

use std::{
    collections::HashMap,
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
    sync::{oneshot, watch, OwnedSemaphorePermit, Semaphore},
    task::JoinHandle,
    time::{Instant, MissedTickBehavior},
};

use crate::{config::read_bounded, model::WorkerPhase, platform::process_is_definitely_gone};

#[derive(Debug)]
pub struct CommandSpec {
    pub executable: PathBuf,
    pub args: Vec<OsString>,
    pub stdin: Option<Vec<u8>>,
    pub timeout: Duration,
    pub scope: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommandOutcome {
    Completed,
    TimedOut,
    OutputLimit,
    SpawnFailed,
    Cancelled,
}

#[derive(Debug)]
pub struct CommandOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_code: Option<i32>,
    pub outcome: CommandOutcome,
}

pub struct RunningCommand {
    pub result: oneshot::Receiver<CommandOutput>,
    pub phase: watch::Receiver<WorkerPhase>,
}

pub struct CommandRunner {
    workers: usize,
    stdout_limit: usize,
    stderr_limit: usize,
    slots: Arc<Semaphore>,
    scopes: Mutex<HashMap<String, usize>>,
    pending: AtomicUsize,
    recovered: AtomicUsize,
    quarantined: AtomicUsize,
    journal_directory: Option<PathBuf>,
    boot_id: String,
    persistence_failed: AtomicBool,
    shutdown: watch::Sender<bool>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ActiveJob {
    schema_version: u8,
    boot_id: String,
    scope: String,
    executable: PathBuf,
    pid: Option<u32>,
}

/// Dropped only after a confirmed exit and durable journal removal. Detached
/// supervisors own this reservation independently of the caller's result handle.
struct Reservation {
    runner: Arc<CommandRunner>,
    scope: String,
    _permit: Option<OwnedSemaphorePermit>,
    recovered: bool,
}

impl Drop for Reservation {
    fn drop(&mut self) {
        let mut scopes = self.runner.scopes.lock().unwrap();
        let count = scopes.get_mut(&self.scope).expect("reserved scope");
        *count -= 1;
        if *count == 0 {
            scopes.remove(&self.scope);
        }
        self.runner.pending.fetch_sub(1, Ordering::SeqCst);
        if self.recovered {
            self.runner.recovered.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

impl CommandRunner {
    pub async fn new(
        workers: usize,
        stdout_limit: usize,
        stderr_limit: usize,
        journal_directory: Option<PathBuf>,
        boot_id: String,
    ) -> Result<Arc<Self>> {
        if workers == 0 || workers > Semaphore::MAX_PERMITS {
            bail!("invalid command worker count");
        }
        if boot_id.is_empty() {
            bail!("command journal requires a boot identity");
        }
        let recovered = match &journal_directory {
            Some(directory) => {
                let directory = directory.clone();
                let boot = boot_id.clone();
                tokio::task::spawn_blocking(move || recover_records(&directory, &boot)).await??
            }
            None => Vec::new(),
        };
        let (shutdown, _) = watch::channel(false);
        let runner = Arc::new(Self {
            workers,
            stdout_limit,
            stderr_limit,
            slots: Arc::new(Semaphore::new(workers)),
            scopes: Mutex::new(HashMap::new()),
            pending: AtomicUsize::new(0),
            recovered: AtomicUsize::new(0),
            quarantined: AtomicUsize::new(0),
            journal_directory,
            boot_id,
            persistence_failed: AtomicBool::new(false),
            shutdown,
        });
        // More recovered workers than the current configured limit still count.
        // Admission checks the full pending count as well as semaphore permits.
        for (path, record) in recovered {
            let permit = runner.slots.clone().try_acquire_owned().ok();
            let reservation = runner.reserve_recovered(record.scope.clone(), permit);
            let runner = runner.clone();
            tokio::spawn(async move {
                match record.pid {
                    Some(pid) => {
                        while !process_is_definitely_gone(pid) {
                            tokio::time::sleep(Duration::from_millis(250)).await;
                        }
                        runner.remove_record_until_durable(Some(path)).await;
                        drop(reservation);
                    }
                    // A crash between durable intent and PID update cannot be
                    // distinguished from a worker still executing. Explicit
                    // operator recovery (or a different boot) is required.
                    None => {
                        std::future::pending::<()>().await;
                        drop(reservation);
                    }
                }
            });
        }
        Ok(runner)
    }

    fn reserve_recovered(
        self: &Arc<Self>,
        scope: String,
        permit: Option<OwnedSemaphorePermit>,
    ) -> Reservation {
        *self
            .scopes
            .lock()
            .unwrap()
            .entry(scope.clone())
            .or_default() += 1;
        self.pending.fetch_add(1, Ordering::SeqCst);
        self.recovered.fetch_add(1, Ordering::SeqCst);
        Reservation {
            runner: self.clone(),
            scope,
            _permit: permit,
            recovered: true,
        }
    }

    /// Try once; occupied scopes or work classes are skipped, never queued.
    pub async fn launch(self: &Arc<Self>, spec: CommandSpec) -> Result<Option<RunningCommand>> {
        if !spec.executable.is_absolute() {
            bail!("command executable must be absolute");
        }
        if spec.scope.is_empty() {
            bail!("command scope must not be empty");
        }
        let deadline = Instant::now()
            .checked_add(spec.timeout)
            .context("command timeout overflow")?;
        if self.persistence_failed.load(Ordering::SeqCst) {
            bail!("command journal failed; admission is disabled");
        }
        let mut scopes = self.scopes.lock().unwrap();
        if *self.shutdown.borrow()
            || self.pending_count() >= self.workers
            || scopes.contains_key(&spec.scope)
        {
            return Ok(None);
        }
        let Ok(permit) = self.slots.clone().try_acquire_owned() else {
            return Ok(None);
        };
        scopes.insert(spec.scope.clone(), 1);
        self.pending.fetch_add(1, Ordering::SeqCst);
        let reservation = Reservation {
            runner: self.clone(),
            scope: spec.scope.clone(),
            _permit: Some(permit),
            recovered: false,
        };
        drop(scopes);
        let (result, receiver) = oneshot::channel();
        let (phase, phase_receiver) = watch::channel(WorkerPhase::Running);
        let runner = self.clone();
        tokio::spawn(async move {
            runner
                .supervise(spec, deadline, result, phase, reservation)
                .await;
        });
        Ok(Some(RunningCommand {
            result: receiver,
            phase: phase_receiver,
        }))
    }

    pub fn shutdown(&self) {
        self.shutdown.send_replace(true);
    }

    pub fn pending_count(&self) -> usize {
        self.pending.load(Ordering::SeqCst)
    }

    /// Reservations recovered at startup, excluding this session's commands.
    pub fn recovered_count(&self) -> usize {
        self.recovered.load(Ordering::SeqCst)
    }

    /// This session's children with a published terminal observation whose exit
    /// has not been confirmed. Excludes recovered jobs and metadata-only waits.
    pub fn quarantined_count(&self) -> usize {
        self.quarantined.load(Ordering::SeqCst)
    }

    async fn supervise(
        self: Arc<Self>,
        spec: CommandSpec,
        deadline: Instant,
        result: oneshot::Sender<CommandOutput>,
        phase: watch::Sender<WorkerPhase>,
        reservation: Reservation,
    ) {
        let mut result = Some(result);
        let mut shutdown = self.shutdown.subscribe();
        let path = self
            .journal_directory
            .as_ref()
            .map(|dir| dir.join(format!("job-{}.json", uuid::Uuid::new_v4())));
        let mut record = ActiveJob {
            schema_version: 1,
            boot_id: self.boot_id.clone(),
            scope: spec.scope.clone(),
            executable: spec.executable.clone(),
            pid: None,
        };

        // The pre-spawn intent is durable before any subprocess is created.
        // Even a slow metadata disk cannot delay the caller's deadline result.
        if let Some(path) = &path {
            let mut write = write_record(path.clone(), record.clone());
            let mut write_observed = false;
            let early = if *shutdown.borrow() {
                Some(CommandOutcome::Cancelled)
            } else {
                tokio::select! {
                    done = &mut write => {
                        write_observed = true;
                        if !journal_ok(done) { self.persistence_failed.store(true, Ordering::SeqCst); Some(CommandOutcome::SpawnFailed) }
                        else { None }
                    }
                    _ = tokio::time::sleep_until(deadline) => Some(CommandOutcome::TimedOut),
                    _ = result.as_mut().unwrap().closed() => Some(CommandOutcome::Cancelled),
                    _ = shutdown.changed() => Some(CommandOutcome::Cancelled),
                }
            };
            if let Some(outcome) = early {
                publish(&mut result, &phase, outcome, &[], &[], None);
                if !write_observed && !journal_ok(write.await) {
                    self.persistence_failed.store(true, Ordering::SeqCst);
                }
                self.remove_record_until_durable(Some(path.clone())).await;
                drop(reservation);
                phase.send_replace(WorkerPhase::Idle);
                return;
            }
        }
        if *shutdown.borrow()
            || result.as_ref().is_some_and(oneshot::Sender::is_closed)
            || Instant::now() >= deadline
        {
            let outcome = if Instant::now() >= deadline {
                CommandOutcome::TimedOut
            } else {
                CommandOutcome::Cancelled
            };
            publish(&mut result, &phase, outcome, &[], &[], None);
            self.remove_record_until_durable(path).await;
            drop(reservation);
            phase.send_replace(WorkerPhase::Idle);
            return;
        }

        let mut command = Command::new(&spec.executable);
        command
            .args(spec.args)
            .current_dir("/")
            .env_clear()
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("LANG", "C")
            .env("LC_ALL", "C")
            .stdin(if spec.stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(_) => {
                publish(
                    &mut result,
                    &phase,
                    CommandOutcome::SpawnFailed,
                    &[],
                    &[],
                    None,
                );
                self.remove_record_until_durable(path).await;
                drop(reservation);
                phase.send_replace(WorkerPhase::Idle);
                return;
            }
        };
        record.pid = child.id();
        let mut pid_write = path.as_ref().map(|path| write_record(path.clone(), record));
        let mut stdout = child.stdout.take();
        let mut stderr = child.stderr.take();
        let stdin_write = spec.stdin.zip(child.stdin.take()).map(|(bytes, mut pipe)| {
            tokio::spawn(async move {
                let _ = pipe.write_all(&bytes).await;
                let _ = pipe.shutdown().await;
            })
        });
        let mut stdout_bytes = Vec::new();
        let mut stderr_bytes = Vec::new();
        let mut out_buffer = [0u8; 8192];
        let mut err_buffer = [0u8; 8192];
        let mut exited = None;
        let mut quarantined = false;
        let mut poll = tokio::time::interval(Duration::from_millis(20));
        poll.set_missed_tick_behavior(MissedTickBehavior::Skip);
        // No wait error is treated as proof of exit. A failed reaper retains the
        // Child and its reservation and retries at a bounded frequency.
        loop {
            if exited.is_some() && stdout.is_none() && stderr.is_none() {
                break;
            }
            let mut terminate = None;
            tokio::select! {
                _ = poll.tick(), if exited.is_none() => {
                    if let Ok(Some(status)) = child.try_wait() {
                        if quarantined {
                            self.quarantined.fetch_sub(1, Ordering::SeqCst);
                            quarantined = false;
                        }
                        exited = Some(status);
                    }
                }
                read = async { stdout.as_mut().unwrap().read(&mut out_buffer).await }, if stdout.is_some() => {
                    match read {
                        Ok(0) | Err(_) => { stdout = None; }
                        Ok(n) => {
                            if append_capped(&mut stdout_bytes, &out_buffer[..n], self.stdout_limit) {
                                terminate = Some(CommandOutcome::OutputLimit);
                            }
                        }
                    }
                }
                read = async { stderr.as_mut().unwrap().read(&mut err_buffer).await }, if stderr.is_some() => {
                    match read {
                        Ok(0) | Err(_) => { stderr = None; }
                        Ok(n) => {
                            if append_capped(&mut stderr_bytes, &err_buffer[..n], self.stderr_limit) {
                                terminate = Some(CommandOutcome::OutputLimit);
                            }
                        }
                    }
                }
                _ = tokio::time::sleep_until(deadline), if result.is_some() => { terminate = Some(CommandOutcome::TimedOut); }
                _ = async { result.as_mut().unwrap().closed().await }, if result.is_some() => { terminate = Some(CommandOutcome::Cancelled); }
                _ = shutdown.changed(), if result.is_some() => { terminate = Some(CommandOutcome::Cancelled); }
                done = async { pid_write.as_mut().unwrap().await }, if pid_write.is_some() => {
                    if !journal_ok(done) {
                        self.persistence_failed.store(true, Ordering::SeqCst);
                        terminate = Some(CommandOutcome::SpawnFailed);
                    }
                    pid_write = None;
                }
            }
            if *shutdown.borrow() && result.is_some() {
                terminate = Some(CommandOutcome::Cancelled);
            }
            if let Some(outcome) = terminate {
                if !quarantined
                    && exited.is_none()
                    && result.is_some()
                    && matches!(
                        outcome,
                        CommandOutcome::TimedOut
                            | CommandOutcome::OutputLimit
                            | CommandOutcome::Cancelled
                    )
                {
                    self.quarantined.fetch_add(1, Ordering::SeqCst);
                    quarantined = true;
                }
                publish(
                    &mut result,
                    &phase,
                    outcome,
                    &stdout_bytes,
                    &stderr_bytes,
                    exited.as_ref().and_then(|status| status.code()),
                );
                // Close the pipes rather than wait for untrusted output or a
                // descendant that inherited an FD. start_kill never waits.
                stdout = None;
                stderr = None;
                if let Some(write) = &stdin_write {
                    write.abort();
                }
                if exited.is_none() {
                    let _ = child.start_kill();
                }
            }
        }
        if let Some(write) = stdin_write {
            write.abort();
            let _ = write.await;
        }
        publish(
            &mut result,
            &phase,
            CommandOutcome::Completed,
            &stdout_bytes,
            &stderr_bytes,
            exited.and_then(|status| status.code()),
        );
        if let Some(write) = pid_write {
            if !journal_ok(write.await) {
                self.persistence_failed.store(true, Ordering::SeqCst);
            }
        }
        self.remove_record_until_durable(path).await;
        drop(reservation);
        phase.send_replace(WorkerPhase::Idle);
    }

    async fn remove_record_until_durable(&self, path: Option<PathBuf>) {
        let Some(path) = path else {
            return;
        };
        loop {
            let path = path.clone();
            let removed = tokio::task::spawn_blocking(move || remove_record(&path)).await;
            if journal_ok(removed) {
                return;
            }
            self.persistence_failed.store(true, Ordering::SeqCst);
            // A failed unlink/fsync leaves an uncertain record. Keep its budget
            // reserved while retrying, even though the child is already gone.
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
}

fn publish(
    sender: &mut Option<oneshot::Sender<CommandOutput>>,
    phase: &watch::Sender<WorkerPhase>,
    outcome: CommandOutcome,
    stdout: &[u8],
    stderr: &[u8],
    exit_code: Option<i32>,
) {
    let Some(sender) = sender.take() else {
        return;
    };
    if matches!(
        outcome,
        CommandOutcome::TimedOut | CommandOutcome::OutputLimit | CommandOutcome::Cancelled
    ) {
        phase.send_replace(WorkerPhase::TimedOutPendingExit);
    }
    let _ = sender.send(CommandOutput {
        stdout: stdout.to_vec(),
        stderr: stderr.to_vec(),
        exit_code,
        outcome,
    });
}

fn append_capped(bytes: &mut Vec<u8>, incoming: &[u8], cap: usize) -> bool {
    let available = cap.saturating_sub(bytes.len());
    bytes.extend_from_slice(&incoming[..incoming.len().min(available)]);
    incoming.len() > available
}

fn journal_ok(result: std::result::Result<Result<()>, tokio::task::JoinError>) -> bool {
    matches!(result, Ok(Ok(())))
}

fn write_record(path: PathBuf, record: ActiveJob) -> JoinHandle<Result<()>> {
    tokio::task::spawn_blocking(move || {
        let parent = path.parent().context("journal path has no parent")?;
        let temporary = parent.join(format!(".job-{}.tmp", uuid::Uuid::new_v4()));
        let result = (|| -> Result<()> {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temporary)?;
            file.write_all(&serde_json::to_vec(&record)?)?;
            file.sync_all()?;
            fs::rename(&temporary, &path)?;
            File::open(parent)?.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result
    })
}

fn remove_record(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    File::open(path.parent().context("journal path has no parent")?)?.sync_all()?;
    Ok(())
}

fn recover_records(directory: &Path, boot_id: &str) -> Result<Vec<(PathBuf, ActiveJob)>> {
    fs::create_dir_all(directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
    }
    let mut recovered = Vec::new();
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        let record: ActiveJob = serde_json::from_slice(&read_bounded(&path, 16 * 1024)?)
            .context("invalid active command journal")?;
        if record.schema_version != 1
            || record.boot_id.is_empty()
            || record.scope.is_empty()
            || !record.executable.is_absolute()
        {
            bail!("invalid active command journal identity");
        }
        if record.boot_id != boot_id || record.pid.is_some_and(process_is_definitely_gone) {
            remove_record(&path)?;
        } else {
            recovered.push((path, record));
        }
    }
    Ok(recovered)
}
