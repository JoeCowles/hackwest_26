//! The only unsafe Rust boundary. All potentially blocking collection is worker-only.
use anyhow::Result;
use serde::{Deserialize, Serialize};

pub const MAX_WORKER_INPUT: usize = 64 * 1024;
pub const MAX_WORKER_OUTPUT: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "operation", rename_all = "kebab-case", deny_unknown_fields)]
pub enum WorkerRequest {
    Mounts,
    Capacity { path: String, fsid: [i32; 2] },
    Iokit,
    NfsStatus { fsid: [i32; 2] },
}

#[derive(Clone, Debug)]
pub struct SystemInfo {
    pub boot_id: String,
    pub os_version: String,
    pub os_build: String,
    pub target: String,
}

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::{effective_uid, pid_exists, process_is_definitely_gone, system_info, worker};

#[cfg(not(target_os = "macos"))]
pub fn worker(_: WorkerRequest) -> Result<Vec<u8>> {
    anyhow::bail!("native storage collection requires macOS")
}
#[cfg(not(target_os = "macos"))]
pub fn system_info() -> Result<SystemInfo> {
    anyhow::bail!("ciderd run requires macOS")
}
#[cfg(not(target_os = "macos"))]
pub fn effective_uid() -> u32 {
    u32::MAX
}
#[cfg(not(target_os = "macos"))]
pub fn pid_exists(_: u32) -> bool {
    false
}
#[cfg(all(unix, not(target_os = "macos")))]
pub fn process_is_definitely_gone(pid: u32) -> bool {
    if pid == 0 || pid > i32::MAX as u32 {
        return false;
    }
    // SAFETY: signal zero on a positive PID is an existence check, never a signal.
    unsafe {
        libc::kill(pid as i32, 0) == -1
            && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
    }
}
#[cfg(not(unix))]
pub fn process_is_definitely_gone(_: u32) -> bool {
    false
}

pub fn parse_worker_request(bytes: &[u8]) -> Result<WorkerRequest> {
    anyhow::ensure!(
        bytes.len() <= MAX_WORKER_INPUT,
        "worker input exceeds limit"
    );
    let value: serde_json::Value = crate::model::parse_json(bytes)?;
    let fields = value
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("worker input is not an object"))?;
    // Serde's internally tagged unit variants otherwise ignore extra fields even
    // with deny_unknown_fields. Check the operation-specific input envelope first.
    let allowed: &[&str] = match fields.get("operation").and_then(serde_json::Value::as_str) {
        Some("mounts" | "iokit") => &["operation"],
        Some("capacity") => &["operation", "path", "fsid"],
        Some("nfs-status") => &["operation", "fsid"],
        _ => anyhow::bail!("unknown worker operation"),
    };
    anyhow::ensure!(
        fields.len() == allowed.len() && fields.keys().all(|k| allowed.contains(&k.as_str())),
        "unexpected worker request fields"
    );
    Ok(serde_json::from_value(value)?)
}
