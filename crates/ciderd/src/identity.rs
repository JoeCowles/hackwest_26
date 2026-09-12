//! Startup-only identity persistence. Never called by the heartbeat sender.
use crate::{
    config::read_bounded,
    model::{check_id, parse_json},
};
use anyhow::{ensure, Context, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::Path,
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Identity {
    node_id: String,
    agent_generation: u64,
}

pub struct Session {
    pub node_id: String,
    pub generation: u64,
    pub session_id: String,
    _lock: File,
}

fn private_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    options
}

pub fn enroll(path: &Path, node_id: Option<&str>) -> Result<String> {
    let node_id = node_id
        .map(str::to_owned)
        .unwrap_or_else(|| format!("node-{}", uuid::Uuid::new_v4()));
    check_id(&node_id)?;
    ensure!(
        node_id.len() <= 128
            && node_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b)),
        "node ID must use 1..128 ASCII letters, digits, -, _, or ."
    );
    let parent = path.parent().context("identity path has no parent")?;
    std::fs::create_dir_all(parent)?;
    let mut file = private_options()
        .create_new(true)
        .open(path)
        .context("identity already exists or cannot be created")?;
    let data = serde_json::to_vec(&Identity {
        node_id: node_id.clone(),
        agent_generation: 0,
    })?;
    file.write_all(&data)?;
    file.sync_all()?;
    File::open(parent)?.sync_all()?;
    Ok(node_id)
}

impl Session {
    pub fn open(identity_file: &Path, state_directory: &Path) -> Result<Self> {
        std::fs::create_dir_all(state_directory)?;
        // The lock follows identity rather than state_directory: changing the
        // cache location cannot start a second writer of this node generation.
        let mut lock_name = identity_file.as_os_str().to_os_string();
        lock_name.push(".lock");
        let lock_path = std::path::PathBuf::from(lock_name);
        let lock = private_options()
            .create(true)
            .truncate(false)
            .open(lock_path)?;
        lock.try_lock_exclusive()
            .context("another ciderd owns this identity")?;
        let bytes = read_bounded(identity_file, 4096)
            .context("identity missing: enroll explicitly before run")?;
        let mut identity: Identity = parse_json(&bytes)
            .context("identity corrupt: explicit recovery/re-enrollment required")?;
        check_id(&identity.node_id)?;
        identity.agent_generation = identity
            .agent_generation
            .checked_add(1)
            .context("agent generation exhausted")?;
        atomic_write(identity_file, &serde_json::to_vec(&identity)?)?;
        Ok(Self {
            node_id: identity.node_id,
            generation: identity.agent_generation,
            session_id: uuid::Uuid::new_v4().to_string(),
            _lock: lock,
        })
    }
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("state path has no parent")?;
    let temporary = parent.join(format!(".ciderd-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| -> Result<()> {
        let mut file = private_options().create_new(true).open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)?;
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}
