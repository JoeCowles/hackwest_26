use super::{MAX_WORKER_OUTPUT, SystemInfo, WorkerRequest};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    ffi::{CStr, CString},
    mem::{MaybeUninit, size_of},
    ptr,
};

unsafe extern "C" {
    fn storage_fsid_values(fsid: *const libc::fsid_t, output: *mut i32);
    fn storage_iokit(output: *mut *mut u8, length: *mut usize) -> libc::c_int;
    fn storage_native_free(buffer: *mut u8);
    fn storage_mount_identity(
        path: *const libc::c_char,
        output: *mut *mut u8,
        length: *mut usize,
    ) -> libc::c_int;
    fn storage_nstatus(
        fsid0: i32,
        fsid1: i32,
        output: *mut libc::c_char,
        capacity: usize,
    ) -> libc::c_int;
}
fn fsid_values(fsid: &libc::fsid_t) -> [i32; 2] {
    let mut output = [0; 2];
    // SAFETY: libc's SDK-compatible fsid_t is valid and output has two writable i32 elements.
    unsafe { storage_fsid_values(fsid, output.as_mut_ptr()) };
    output
}

pub fn effective_uid() -> u32 {
    // SAFETY: geteuid has no input or ownership preconditions.
    unsafe { libc::geteuid() }
}
pub fn pid_exists(pid: u32) -> bool {
    if pid == 0 || pid > i32::MAX as u32 {
        return false;
    }
    // SAFETY: a positive PID and signal zero only check existence/permission, never signal.
    unsafe {
        libc::kill(pid as i32, 0) == 0
            || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
}
pub fn process_is_definitely_gone(pid: u32) -> bool {
    if pid == 0 || pid > i32::MAX as u32 {
        return false;
    }
    // SAFETY: signal zero on a positive PID cannot send a signal or target a process group.
    unsafe {
        libc::kill(pid as i32, 0) == -1
            && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
    }
}

fn sysctl_string(name: &CStr) -> Result<String> {
    let mut buffer = [0u8; 512];
    let mut size = buffer.len();
    // SAFETY: pointers address initialized writable storage of `size` bytes; no new value is supplied.
    let result = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            buffer.as_mut_ptr().cast(),
            &mut size,
            ptr::null_mut(),
            0,
        )
    };
    ensure!(
        result == 0,
        "system information query failed: {}",
        std::io::Error::last_os_error()
    );
    ensure!(
        size > 0 && size <= buffer.len(),
        "invalid system information length"
    );
    let value =
        CStr::from_bytes_until_nul(&buffer[..size]).context("unterminated system information")?;
    Ok(value.to_str()?.to_owned())
}

pub fn system_info() -> Result<SystemInfo> {
    Ok(SystemInfo {
        boot_id: sysctl_string(c"kern.bootsessionuuid")?,
        os_version: sysctl_string(c"kern.osproductversion")?,
        os_build: sysctl_string(c"kern.osversion")?,
        model_identifier: sysctl_string(c"hw.model").map_err(|e| e.to_string()),
        target: format!("{}-apple-darwin", std::env::consts::ARCH),
    })
}

fn c_array(bytes: &[libc::c_char]) -> Result<(String, String)> {
    let length = bytes
        .iter()
        .position(|b| *b == 0)
        .context("unterminated mount field")?;
    let raw: Vec<u8> = bytes[..length].iter().map(|v| *v as u8).collect();
    let hex: String = raw.iter().map(|v| format!("{v:02x}")).collect();
    Ok((String::from_utf8_lossy(&raw).into_owned(), hex))
}

fn capacity_record(record: &libc::statfs) -> Value {
    json!({"block_size":record.f_bsize.to_string(), "blocks":record.f_blocks.to_string(),
        "blocks_free":record.f_bfree.to_string(), "blocks_available":record.f_bavail.to_string(),
        "files":record.f_files.to_string(), "files_free":record.f_ffree.to_string()})
}

fn mounts() -> Result<Value> {
    // SAFETY: a null buffer with zero length queries the record count only. MNT_NOWAIT avoids refresh.
    let count = unsafe { libc::getfsstat(ptr::null_mut(), 0, libc::MNT_NOWAIT) };
    ensure!(
        count >= 0,
        "getfsstat count failed: {}",
        std::io::Error::last_os_error()
    );
    ensure!(count <= 4096, "mount count exceeds limit");
    // Extra slots detect concurrent additions without returning a falsely complete inventory.
    let slots = (count as usize + 16).min(4097);
    let mut storage: Vec<MaybeUninit<libc::statfs>> = Vec::with_capacity(slots);
    storage.resize_with(slots, MaybeUninit::uninit);
    let byte_count = slots
        .checked_mul(size_of::<libc::statfs>())
        .context("mount storage overflow")?;
    ensure!(byte_count <= i32::MAX as usize, "mount storage overflow");
    // SAFETY: caller-owned storage is aligned for statfs and has byte_count writable bytes.
    let actual = unsafe {
        libc::getfsstat(
            storage.as_mut_ptr().cast(),
            byte_count as i32,
            libc::MNT_NOWAIT,
        )
    };
    ensure!(
        actual >= 0,
        "getfsstat failed: {}",
        std::io::Error::last_os_error()
    );
    ensure!(
        (actual as usize) < slots && actual <= 4096,
        "mount inventory changed beyond buffer capacity"
    );
    let mut rows = Vec::with_capacity(actual as usize);
    for slot in storage.iter().take(actual as usize) {
        // SAFETY: getfsstat initialized exactly the first `actual` records.
        let record = unsafe { slot.assume_init_ref() };
        let (mount_path, mount_path_hex) = c_array(&record.f_mntonname)?;
        let (source, source_hex) = c_array(&record.f_mntfromname)?;
        let (filesystem_type, _) = c_array(&record.f_fstypename)?;
        rows.push(
            json!({"fsid":fsid_values(&record.f_fsid), "mount_path":mount_path,
            "mount_path_hex":mount_path_hex, "source":source, "source_hex":source_hex,
            "filesystem_type":filesystem_type, "local":record.f_flags & libc::MNT_LOCAL as u32 != 0,
            "read_only":record.f_flags & libc::MNT_RDONLY as u32 != 0,
            "capacity":capacity_record(record)}),
        );
    }
    Ok(json!({"version":1,"mounts":rows}))
}

fn capacity(path: String, expected_fsid: [i32; 2]) -> Result<Value> {
    ensure!(
        path.len() <= 4096 && path.starts_with('/'),
        "invalid capacity path"
    );
    let path = CString::new(path).context("capacity path contains NUL")?;
    let mut record = MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: path is a valid terminated string and record points to a full statfs. This runs only in an isolated worker.
    let result = unsafe { libc::statfs(path.as_ptr(), record.as_mut_ptr()) };
    if result != 0 {
        let error = std::io::Error::last_os_error();
        return Ok(
            json!({"status": if matches!(error.raw_os_error(), Some(libc::ENOENT | libc::ESTALE)) {"gone"} else {"failed"}}),
        );
    }
    // SAFETY: successful statfs initialized the whole record.
    let record = unsafe { record.assume_init() };
    if fsid_values(&record.f_fsid) != expected_fsid {
        return Ok(json!({"status":"gone"}));
    }
    Ok(json!({"status":"ok", "capacity":capacity_record(&record)}))
}

fn iokit() -> Result<Vec<u8>> {
    let mut pointer = ptr::null_mut();
    let mut length = 0usize;
    // SAFETY: output arguments are valid; the C shim returns a uniquely owned malloc buffer on success.
    let error = unsafe { storage_iokit(&mut pointer, &mut length) };
    ensure!(error == 0, "IOKit query failed ({error})");
    struct Buffer(*mut u8);
    impl Drop for Buffer {
        fn drop(&mut self) {
            unsafe { storage_native_free(self.0) };
        }
    }
    let buffer = Buffer(pointer);
    ensure!(
        !buffer.0.is_null() && length <= MAX_WORKER_OUTPUT,
        "invalid native output size"
    );
    // SAFETY: shim's successful return guarantees this pointer addresses length initialized bytes; copy before Drop.
    Ok(unsafe { std::slice::from_raw_parts(buffer.0, length) }.to_vec())
}

fn mount_identity(path: String, fsid: [i32; 2], source: String) -> Result<Value> {
    ensure!(
        path.starts_with('/')
            && path.len() <= 4096
            && source
                .strip_prefix("/dev/")
                .is_some_and(super::valid_media_name),
        "invalid local mount identity input"
    );
    let request_path = CString::new(path.clone())?;
    // MNT_NOWAIT reads the kernel mount table, avoiding network-path probes.
    let matches = |table: &Value| {
        table["mounts"].as_array().is_some_and(|rows| {
            rows.iter()
                .filter(|r| {
                    r["mount_path"] == path
                        && r["source"] == source
                        && r["fsid"] == json!(fsid)
                        && r["local"] == true
                        && r["filesystem_type"] != "nfs"
                        && r["filesystem_type"] != "smbfs"
                })
                .count()
                == 1
        })
    };
    let failed = |reason: &str| {
        json!({"fsid":fsid,"source":source,"state":"unavailable","reason":reason,
        "volume_uuid":null,"media_bsd_name":null,"media_registry_id":null})
    };
    if !matches(&mounts()?) {
        return Ok(failed("mount_replaced"));
    }
    let mut pointer = ptr::null_mut();
    let mut length = 0;
    // SAFETY: terminated local path and writable out parameters. Buffer is C-owned until freed below.
    let error = unsafe { storage_mount_identity(request_path.as_ptr(), &mut pointer, &mut length) };
    if error != 0 {
        return Ok(failed("identity_query_failed"));
    }
    struct Buffer(*mut u8);
    impl Drop for Buffer {
        fn drop(&mut self) {
            unsafe { storage_native_free(self.0) };
        }
    }
    let buffer = Buffer(pointer);
    ensure!(
        !buffer.0.is_null() && length <= MAX_WORKER_OUTPUT,
        "invalid identity output"
    );
    // SAFETY: successful shim result owns length initialized bytes, copied/parsed before Drop.
    let identity: Value =
        plist::from_bytes(unsafe { std::slice::from_raw_parts(buffer.0, length) })?;
    if !matches(&mounts()?) {
        return Ok(failed("mount_replaced"));
    }
    if identity["volume_uuid"].as_str().is_none() && identity["media_bsd_name"].as_str().is_none() {
        return Ok(failed("identity_not_reported"));
    }
    Ok(
        json!({"fsid":fsid,"source":source,"state":"ok","reason":null,
        "volume_uuid":identity["volume_uuid"],"media_bsd_name":identity["media_bsd_name"],"media_registry_id":identity["media_registry_id"],
        "parent_volume_uuid":identity["parent_volume_uuid"],"parent_media_bsd_name":identity["parent_media_bsd_name"],"parent_media_registry_id":identity["parent_media_registry_id"]}),
    )
}

fn nfs_status(fsid: [i32; 2]) -> Result<Vec<u8>> {
    let mut output = [0u8; 512];
    // SAFETY: the shim takes two numeric fsid values and a bounded writable output buffer; all SDK structs stay in C.
    let error =
        unsafe { storage_nstatus(fsid[0], fsid[1], output.as_mut_ptr().cast(), output.len()) };
    if error == 0 {
        return Ok(CStr::from_bytes_until_nul(&output)?.to_bytes().to_vec());
    }
    let status = match error {
        libc::ENOENT | libc::ESTALE => "gone",
        libc::ENOTSUP | libc::ENOSYS | libc::EINVAL => "unsupported",
        libc::EACCES | libc::EPERM => "permission_denied",
        _ => "failed",
    };
    Ok(serde_json::to_vec(&json!({"status":status}))?)
}

pub fn worker(request: WorkerRequest) -> Result<Vec<u8>> {
    let bytes = match request {
        WorkerRequest::NfsQuota {
            target,
            timeout_seconds,
        } => serde_json::to_vec(&crate::quota::query(
            &target,
            std::time::Duration::from_secs(timeout_seconds),
        ))?,
        WorkerRequest::Mounts => serde_json::to_vec(&mounts()?)?,
        WorkerRequest::Capacity { path, fsid } => serde_json::to_vec(&capacity(path, fsid)?)?,
        WorkerRequest::Iokit => iokit()?,
        WorkerRequest::MountIdentity { path, fsid, source } => {
            serde_json::to_vec(&mount_identity(path, fsid, source)?)?
        }
        WorkerRequest::NfsStatus { fsid } => nfs_status(fsid)?,
    };
    ensure!(
        bytes.len() <= MAX_WORKER_OUTPUT,
        "worker output exceeds limit"
    );
    Ok(bytes)
}
