use ciderd::{
    command::{CommandOutcome, CommandRunner, CommandSpec},
    model::WorkerPhase,
};
use std::{ffi::OsString, path::PathBuf, time::Duration};
use tempfile::TempDir;

fn spec(executable: &str, args: &[&str], scope: &str, timeout_ms: u64) -> CommandSpec {
    CommandSpec {
        executable: PathBuf::from(executable),
        args: args.iter().map(OsString::from).collect(),
        stdin: None,
        timeout: Duration::from_millis(timeout_ms),
        scope: scope.into(),
    }
}

async fn idle(runner: &CommandRunner) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while runner.pending_count() != 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("command reservations must eventually be released after exit");
}

#[tokio::test]
async fn absolute_executable_fixed_directory_and_minimal_environment() {
    let runner = CommandRunner::new(1, 4096, 4096, None, "test-boot".into())
        .await
        .unwrap();
    assert!(runner.launch(spec("env", &[], "env", 1000)).await.is_err());
    let job = runner
        .launch(spec("/usr/bin/env", &[], "env", 1000))
        .await
        .unwrap()
        .unwrap();
    let output = job.result.await.unwrap();
    assert_eq!(output.outcome, CommandOutcome::Completed);
    assert_eq!(output.exit_code, Some(0));
    let environment = String::from_utf8(output.stdout).unwrap();
    assert_eq!(environment.lines().count(), 3);
    assert!(environment.lines().any(|line| line == "LANG=C"));
    assert!(environment.lines().any(|line| line == "LC_ALL=C"));
    assert!(environment
        .lines()
        .any(|line| line == "PATH=/usr/bin:/bin:/usr/sbin:/sbin"));
    idle(&runner).await;
    let job = runner
        .launch(spec("/bin/pwd", &[], "cwd", 1000))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(job.result.await.unwrap().stdout, b"/\n");
}

#[tokio::test]
async fn stdin_is_closed_unless_explicitly_supplied() {
    let runner = CommandRunner::new(1, 4096, 4096, None, "test-boot".into())
        .await
        .unwrap();
    let job = runner
        .launch(spec("/bin/cat", &[], "cat", 1000))
        .await
        .unwrap()
        .unwrap();
    assert!(job.result.await.unwrap().stdout.is_empty());
    idle(&runner).await;
    let mut request = spec("/bin/cat", &[], "cat", 1000);
    request.stdin = Some(b"specific IPC request\n".to_vec());
    let job = runner.launch(request).await.unwrap().unwrap();
    assert_eq!(job.result.await.unwrap().stdout, b"specific IPC request\n");
}

#[tokio::test]
async fn scope_and_worker_limits_skip_without_queuing() {
    let runner = CommandRunner::new(2, 1024, 1024, None, "test-boot".into())
        .await
        .unwrap();
    let first = runner
        .launch(spec("/bin/sleep", &["10"], "first", 150))
        .await
        .unwrap()
        .unwrap();
    assert!(runner
        .launch(spec("/usr/bin/true", &[], "first", 1000))
        .await
        .unwrap()
        .is_none());
    let second = runner
        .launch(spec("/bin/sleep", &["10"], "second", 150))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(runner.pending_count(), 2);
    assert!(runner
        .launch(spec("/usr/bin/true", &[], "third", 1000))
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        first.result.await.unwrap().outcome,
        CommandOutcome::TimedOut
    );
    assert_eq!(
        second.result.await.unwrap().outcome,
        CommandOutcome::TimedOut
    );
    idle(&runner).await;
    let next = runner
        .launch(spec("/usr/bin/true", &[], "first", 1000))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        next.result.await.unwrap().outcome,
        CommandOutcome::Completed
    );
}

#[tokio::test]
async fn timeout_publishes_pending_exit_then_releases_after_reaping() {
    let runner = CommandRunner::new(1, 1024, 1024, None, "test-boot".into())
        .await
        .unwrap();
    let job = runner
        .launch(spec("/bin/sleep", &["10"], "sleep", 40))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(*job.phase.borrow(), WorkerPhase::Running);
    let mut phase = job.phase;
    assert_eq!(job.result.await.unwrap().outcome, CommandOutcome::TimedOut);
    // Watch channels coalesce updates: a promptly reaped process may already
    // be idle when the result receiver is scheduled.
    match *phase.borrow() {
        WorkerPhase::TimedOutPendingExit => {
            assert_eq!(runner.pending_count(), 1);
            assert_eq!(runner.quarantined_count(), 1);
        }
        WorkerPhase::Idle => {
            assert_eq!(runner.pending_count(), 0);
            assert_eq!(runner.quarantined_count(), 0);
        }
        ref unexpected => panic!("timeout published in phase {unexpected:?}"),
    }
    tokio::time::timeout(Duration::from_secs(2), async {
        while *phase.borrow_and_update() != WorkerPhase::Idle {
            phase.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    assert_eq!(runner.pending_count(), 0);
}

fn helper(name: &str, scope: &str) -> CommandSpec {
    let mut request = spec(
        "/unused",
        &["--ignored", "--exact", name, "--nocapture"],
        scope,
        3000,
    );
    request.executable = std::env::current_exe().unwrap();
    request
}

#[test]
#[ignore]
fn command_stdout_flood_helper() {
    use std::io::Write;
    loop {
        if std::io::stdout().write_all(&[b'x'; 8192]).is_err() {
            break;
        }
    }
}
#[test]
#[ignore]
fn command_stderr_flood_helper() {
    use std::io::Write;
    loop {
        if std::io::stderr().write_all(&[b'y'; 8192]).is_err() {
            break;
        }
    }
}
#[test]
#[ignore]
fn command_both_pipes_helper() {
    use std::io::Write;
    std::io::stdout().write_all(&[b'x'; 512]).unwrap();
    std::io::stderr().write_all(&[b'y'; 512]).unwrap();
}

#[tokio::test]
async fn stdout_and_stderr_floods_are_independently_capped_and_reaped() {
    let runner = CommandRunner::new(1, 257, 129, None, "test-boot".into())
        .await
        .unwrap();
    for name in ["command_stdout_flood_helper", "command_stderr_flood_helper"] {
        let job = runner.launch(helper(name, "flood")).await.unwrap().unwrap();
        let output = tokio::time::timeout(Duration::from_secs(2), job.result)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(output.outcome, CommandOutcome::OutputLimit);
        assert!(output.stdout.len() <= 257);
        assert!(output.stderr.len() <= 129);
        if name.contains("stdout") {
            assert_eq!(output.stdout.len(), 257);
        } else {
            assert_eq!(output.stderr.len(), 129);
        }
        idle(&runner).await;
    }
}

#[tokio::test]
async fn drains_both_pipes_before_publishing_a_normal_exit() {
    let runner = CommandRunner::new(1, 4096, 4096, None, "test-boot".into())
        .await
        .unwrap();
    let job = runner
        .launch(helper("command_both_pipes_helper", "both"))
        .await
        .unwrap()
        .unwrap();
    let output = job.result.await.unwrap();
    assert_eq!(output.outcome, CommandOutcome::Completed);
    assert_eq!(
        output.stdout.iter().filter(|&&byte| byte == b'x').count(),
        512
    );
    assert_eq!(output.stderr, vec![b'y'; 512]);
}

#[tokio::test]
async fn dropping_result_receiver_does_not_leak_the_child_or_scope() {
    let directory = tempfile::tempdir().unwrap();
    let runner = CommandRunner::new(
        1,
        1024,
        1024,
        Some(directory.path().into()),
        "test-boot".into(),
    )
    .await
    .unwrap();
    let job = runner
        .launch(spec("/bin/sleep", &["10"], "cancelled", 5000))
        .await
        .unwrap()
        .unwrap();
    let pid = wait_for_recorded_pid(&directory).await;
    drop(job.result);
    idle(&runner).await;
    assert!(ciderd::platform::process_is_definitely_gone(pid));
    assert_eq!(*job.phase.borrow(), WorkerPhase::Idle);
    let next = runner
        .launch(spec("/usr/bin/true", &[], "cancelled", 1000))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        next.result.await.unwrap().outcome,
        CommandOutcome::Completed
    );
}

#[tokio::test]
async fn shutdown_cancels_running_commands_and_stops_admission() {
    let directory = tempfile::tempdir().unwrap();
    let runner = CommandRunner::new(
        1,
        1024,
        1024,
        Some(directory.path().into()),
        "test-boot".into(),
    )
    .await
    .unwrap();
    let job = runner
        .launch(spec("/bin/sleep", &["10"], "shutdown", 5000))
        .await
        .unwrap()
        .unwrap();
    let pid = wait_for_recorded_pid(&directory).await;
    runner.shutdown();
    assert!(runner
        .launch(spec("/usr/bin/true", &[], "next", 1000))
        .await
        .unwrap()
        .is_none());
    assert_eq!(job.result.await.unwrap().outcome, CommandOutcome::Cancelled);
    idle(&runner).await;
    assert!(ciderd::platform::process_is_definitely_gone(pid));
}

async fn wait_for_recorded_pid(directory: &TempDir) -> u32 {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            for file in std::fs::read_dir(directory.path()).unwrap().flatten() {
                if !file
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "json")
                {
                    continue;
                }
                let record: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(file.path()).unwrap()).unwrap();
                if let Some(pid) = record["pid"].as_u64() {
                    return u32::try_from(pid).unwrap();
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("spawned child must be recorded")
}

fn write_record(directory: &TempDir, name: &str, boot: &str, scope: &str, pid: Option<u32>) {
    std::fs::write(directory.path().join(format!("job-{name}.json")), serde_json::to_vec(&serde_json::json!({
        "schema_version": 1, "boot_id": boot, "scope": scope, "executable": "/bin/sleep", "pid": pid
    })).unwrap()).unwrap();
}

#[tokio::test]
async fn journal_is_present_while_the_process_runs_and_removed_after_reap() {
    let directory = tempfile::tempdir().unwrap();
    let runner = CommandRunner::new(
        1,
        1024,
        1024,
        Some(directory.path().into()),
        "test-boot".into(),
    )
    .await
    .unwrap();
    let job = runner
        .launch(spec("/bin/sleep", &["10"], "journal", 500))
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_millis(400), async {
        loop {
            let files: Vec<_> = std::fs::read_dir(directory.path())
                .unwrap()
                .flatten()
                .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
                .collect();
            if let Some(file) = files.first() {
                let record: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(file.path()).unwrap()).unwrap();
                if record["pid"].is_u64() {
                    assert_eq!(record["scope"], "journal");
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(job.result.await.unwrap().outcome, CommandOutcome::TimedOut);
    idle(&runner).await;
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn same_boot_live_and_unknown_records_preserve_worker_accounting() {
    let directory = tempfile::tempdir().unwrap();
    write_record(
        &directory,
        "live",
        "test-boot",
        "live",
        Some(std::process::id()),
    );
    write_record(&directory, "uncertain", "test-boot", "unknown", None);
    let runner = CommandRunner::new(
        2,
        1024,
        1024,
        Some(directory.path().into()),
        "test-boot".into(),
    )
    .await
    .unwrap();
    assert_eq!(runner.pending_count(), 2);
    assert!(runner
        .launch(spec("/usr/bin/true", &[], "next", 1000))
        .await
        .unwrap()
        .is_none());
    runner.shutdown();
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(runner.pending_count(), 2);
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 2);
}

#[tokio::test]
async fn recovered_process_is_never_signalled_and_releases_only_when_gone() {
    let directory = tempfile::tempdir().unwrap();
    let mut child = std::process::Command::new("/bin/sleep")
        .arg("10")
        .spawn()
        .unwrap();
    write_record(
        &directory,
        "survivor",
        "test-boot",
        "survivor",
        Some(child.id()),
    );
    let runner = CommandRunner::new(
        1,
        1024,
        1024,
        Some(directory.path().into()),
        "test-boot".into(),
    )
    .await
    .unwrap();
    runner.shutdown();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        child.try_wait().unwrap().is_none(),
        "recovered PIDs must not be killed without identity proof"
    );
    assert_eq!(runner.pending_count(), 1);
    child.kill().unwrap();
    child.wait().unwrap();
    idle(&runner).await;
    assert_eq!(runner.recovered_count(), 0);
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn boot_change_discards_stale_records() {
    let directory = tempfile::tempdir().unwrap();
    write_record(
        &directory,
        "old",
        "previous-boot",
        "scope",
        Some(std::process::id()),
    );
    let runner = CommandRunner::new(
        1,
        1024,
        1024,
        Some(directory.path().into()),
        "new-boot".into(),
    )
    .await
    .unwrap();
    assert_eq!(runner.pending_count(), 0);
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn malformed_journal_fails_closed() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("job-broken.json"), b"{").unwrap();
    assert!(
        CommandRunner::new(1, 1024, 1024, Some(directory.path().into()), "boot".into())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn blocked_stdin_does_not_delay_deadline_or_reaping() {
    let runner = CommandRunner::new(1, 1024, 1024, None, "boot".into())
        .await
        .unwrap();
    let mut request = spec("/bin/sleep", &["10"], "blocked-stdin", 40);
    request.stdin = Some(vec![b'x'; 1024 * 1024]);
    let job = runner.launch(request).await.unwrap().unwrap();
    let output = tokio::time::timeout(Duration::from_secs(1), job.result)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(output.outcome, CommandOutcome::TimedOut);
    idle(&runner).await;
}

#[tokio::test]
async fn spawn_failure_releases_scope_and_removes_journal() {
    let directory = tempfile::tempdir().unwrap();
    let runner = CommandRunner::new(1, 1024, 1024, Some(directory.path().into()), "boot".into())
        .await
        .unwrap();
    let missing = directory.path().join("does-not-exist");
    let job = runner
        .launch(spec(missing.to_str().unwrap(), &[], "missing", 1000))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        job.result.await.unwrap().outcome,
        CommandOutcome::SpawnFailed
    );
    idle(&runner).await;
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    let next = runner
        .launch(spec("/usr/bin/true", &[], "missing", 1000))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        next.result.await.unwrap().outcome,
        CommandOutcome::Completed
    );
}

#[tokio::test]
async fn failed_journal_write_prevents_launch_and_disables_new_work() {
    let directory = tempfile::tempdir().unwrap();
    let journal = directory.path().join("journal");
    let runner = CommandRunner::new(1, 1024, 1024, Some(journal.clone()), "boot".into())
        .await
        .unwrap();
    std::fs::remove_dir(&journal).unwrap();
    // A regular file at the directory path makes persistence fail reliably even
    // for a privileged test process, without filesystem permission assumptions.
    std::fs::write(&journal, b"not a directory").unwrap();
    let job = runner
        .launch(spec("/usr/bin/true", &[], "failed", 1000))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        job.result.await.unwrap().outcome,
        CommandOutcome::SpawnFailed
    );
    assert_eq!(runner.pending_count(), 1);
    assert!(runner
        .launch(spec("/usr/bin/true", &[], "next", 1000))
        .await
        .is_err());
    std::fs::remove_file(&journal).unwrap();
    std::fs::create_dir(&journal).unwrap();
    idle(&runner).await;
    assert!(runner
        .launch(spec("/usr/bin/true", &[], "next", 1000))
        .await
        .is_err());
}

#[tokio::test]
async fn lowering_worker_limit_does_not_forget_recovered_work() {
    let directory = tempfile::tempdir().unwrap();
    for index in 0..3 {
        write_record(
            &directory,
            &index.to_string(),
            "boot",
            &format!("scope-{index}"),
            None,
        );
    }
    let runner = CommandRunner::new(1, 1024, 1024, Some(directory.path().into()), "boot".into())
        .await
        .unwrap();
    assert_eq!(runner.pending_count(), 3);
    assert!(runner
        .launch(spec("/usr/bin/true", &[], "next", 1000))
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn recovered_count_excludes_this_sessions_running_commands() {
    let directory = tempfile::tempdir().unwrap();
    write_record(
        &directory,
        "existing",
        "boot",
        "existing",
        Some(std::process::id()),
    );
    let runner = CommandRunner::new(2, 1024, 1024, Some(directory.path().into()), "boot".into())
        .await
        .unwrap();
    assert_eq!(runner.recovered_count(), 1);
    let job = runner
        .launch(spec("/bin/sleep", &["10"], "new", 40))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(runner.pending_count(), 2);
    assert_eq!(runner.recovered_count(), 1);
    assert_eq!(job.result.await.unwrap().outcome, CommandOutcome::TimedOut);
    tokio::time::timeout(Duration::from_secs(2), async {
        while runner.pending_count() != 1 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(runner.recovered_count(), 1);
}

#[tokio::test]
async fn oversized_active_job_record_fails_closed_before_recovery() {
    let directory = tempfile::tempdir().unwrap();
    write_record(
        &directory,
        "oversized",
        "boot",
        "scope",
        Some(std::process::id()),
    );
    let path = directory.path().join("job-oversized.json");
    let mut bytes = std::fs::read(&path).unwrap();
    bytes.extend_from_slice(&vec![b' '; 16 * 1024]);
    std::fs::write(path, bytes).unwrap();
    assert!(
        CommandRunner::new(1, 1024, 1024, Some(directory.path().into()), "boot".into())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn quarantine_count_clears_at_reap_even_if_journal_cleanup_is_pending() {
    let directory = tempfile::tempdir().unwrap();
    let journal = directory.path().join("journal");
    let backup = directory.path().join("paused-record.json");
    let runner = CommandRunner::new(1, 1024, 1024, Some(journal.clone()), "boot".into())
        .await
        .unwrap();
    let job = runner
        .launch(spec("/bin/sleep", &["10"], "quarantined", 1000))
        .await
        .unwrap()
        .unwrap();
    let (pid, record_path) = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            for file in std::fs::read_dir(&journal).unwrap().flatten() {
                if file
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "json")
                {
                    let record: serde_json::Value =
                        serde_json::from_slice(&std::fs::read(file.path()).unwrap()).unwrap();
                    if let Some(pid) = record["pid"].as_u64() {
                        return (u32::try_from(pid).unwrap(), file.path());
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(runner.quarantined_count(), 0);
    // A visible PID proves the record was renamed, but its parent directory
    // may still be syncing. Block only record unlinking, leaving that parent
    // intact so this test cannot turn a successful spawn into a journal error.
    std::fs::rename(&record_path, &backup).unwrap();
    std::fs::create_dir(&record_path).unwrap();
    assert_eq!(job.result.await.unwrap().outcome, CommandOutcome::TimedOut);
    tokio::time::timeout(Duration::from_secs(2), async {
        while !ciderd::platform::process_is_definitely_gone(pid) || runner.quarantined_count() != 0
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(runner.pending_count(), 1);
    assert_eq!(runner.quarantined_count(), 0);
    std::fs::remove_dir(&record_path).unwrap();
    std::fs::rename(&backup, &record_path).unwrap();
    idle(&runner).await;
}
