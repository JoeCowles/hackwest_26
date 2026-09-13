//! The only unsafe Rust boundary. All potentially blocking collection is worker-only.
use anyhow::Result;
use serde::{Deserialize, Serialize};

pub const MAX_WORKER_INPUT: usize = 64 * 1024;
pub const MAX_WORKER_OUTPUT: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "operation", rename_all = "kebab-case", deny_unknown_fields)]
pub enum WorkerRequest {
    Mounts,
    NfsQuota {
        target: crate::quota::QuotaTarget,
        timeout_seconds: u64,
    },
    Capacity {
        path: String,
        fsid: [i32; 2],
    },
    MountIdentity {
        path: String,
        fsid: [i32; 2],
        source: String,
    },
    Iokit,
    NfsStatus {
        fsid: [i32; 2],
    },
}

#[derive(Clone, Debug)]
pub struct SystemInfo {
    pub boot_id: String,
    pub os_version: String,
    pub os_build: String,
    pub target: String,
    pub model_identifier: Result<String, String>,
}

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::{effective_uid, pid_exists, process_is_definitely_gone, system_info, worker};

#[cfg(not(target_os = "macos"))]
pub fn worker(request: WorkerRequest) -> Result<Vec<u8>> {
    if let WorkerRequest::NfsQuota {
        target,
        timeout_seconds,
    } = request
    {
        return Ok(serde_json::to_vec(&crate::quota::query(
            &target,
            std::time::Duration::from_secs(timeout_seconds),
        ))?);
    }
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
        Some("mount-identity") => &["operation", "path", "fsid", "source"],
        Some("capacity") => &["operation", "path", "fsid"],
        Some("nfs-quota") => &["operation", "target", "timeout_seconds"],
        Some("nfs-status") => &["operation", "fsid"],
        _ => anyhow::bail!("unknown worker operation"),
    };
    anyhow::ensure!(
        fields.len() == allowed.len() && fields.keys().all(|k| allowed.contains(&k.as_str())),
        "unexpected worker request fields"
    );
    let request = serde_json::from_value(value)?;
    if let WorkerRequest::NfsQuota {
        target,
        timeout_seconds,
    } = &request
    {
        target.validate()?;
        anyhow::ensure!(
            (1..=10).contains(timeout_seconds),
            "invalid quota worker deadline"
        );
    }
    if let WorkerRequest::MountIdentity { path, source, .. } = &request {
        anyhow::ensure!(
            path.starts_with('/') && path.len() <= 4096 && !path.contains('\0'),
            "invalid mount identity path"
        );
        anyhow::ensure!(
            source.strip_prefix("/dev/").is_some_and(valid_media_name),
            "mount identity requires local device source"
        );
    }
    Ok(request)
}

pub fn valid_media_name(name: &str) -> bool {
    name.starts_with("disk")
        && name.len() <= 64
        && name.len() > 4
        && name[4..]
            .split('s')
            .all(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()))
}

#[derive(Clone, Debug)]
pub struct RpcIdentity {
    pub uid: u32,
    pub gid: u32,
    pub groups: Vec<u32>,
    pub hostname: String,
    pub groups_truncated: bool,
}
#[cfg(unix)]
pub fn rpc_identity() -> Result<RpcIdentity> {
    // SAFETY: getuid/getgid have no preconditions; hostname and group buffers are
    // correctly sized. AUTH_SYS uses the process's real identity, never query UID.
    unsafe {
        let uid = libc::getuid();
        let gid = libc::getgid();
        let count = libc::getgroups(0, std::ptr::null_mut());
        anyhow::ensure!(count >= 0 && count <= 65536, "cannot read process groups");
        let mut groups = vec![0 as libc::gid_t; count as usize];
        let got = libc::getgroups(count, groups.as_mut_ptr());
        anyhow::ensure!(got >= 0, "cannot read process groups");
        groups.truncate(got as usize);
        let groups_truncated = groups.len() > 16;
        groups.truncate(16);
        let mut host = [0u8; 256];
        anyhow::ensure!(
            libc::gethostname(host.as_mut_ptr().cast(), host.len()) == 0,
            "cannot read hostname"
        );
        let len = host.iter().position(|b| *b == 0).unwrap_or(255);
        let hostname = std::str::from_utf8(&host[..len])?.to_owned();
        Ok(RpcIdentity {
            uid,
            gid,
            groups,
            hostname,
            groups_truncated,
        })
    }
}
#[cfg(not(unix))]
pub fn rpc_identity() -> Result<RpcIdentity> {
    anyhow::bail!("AUTH_SYS identity unavailable")
}
