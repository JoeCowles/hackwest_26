//! Explicit, one-time Cider enrollment. Never invoked by the heartbeat loop.
use crate::{config::{self, Config}, heartbeat::bearer_header, identity, model::parse_json};
use anyhow::{ensure, Context, Result};
use serde::Deserialize;
use serde_json::json;
use std::{fs::{self, File, OpenOptions}, io::Write, path::Path, time::Duration};

#[derive(Deserialize)]
struct Envelope { data: Enrollment }
#[derive(Deserialize)]
struct Enrollment { node_id: String, credential: String }

/// Exchanges a one-use token without printing or accepting a secret on argv.
pub async fn enroll(config: &Config, name: &str, token_file: &Path) -> Result<String> {
    config.validate()?;
    ensure!(!name.is_empty() && name.len() <= 128 && !name.chars().any(char::is_control), "name must contain 1..128 bytes without control characters");
    let identity_file = &config.node.identity_file;
    let credential_file = &config.heartbeat.bearer_token_file;
    ensure!(identity_file != credential_file && identity_file != token_file && credential_file != token_file, "identity, credential, and enrollment token paths must differ");
    for path in [identity_file, credential_file] {
        match fs::symlink_metadata(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
            _ => anyhow::bail!("identity or credential file already exists or cannot be inspected; enrollment will not overwrite it"),
        }
        let parent = path.parent().context("enrollment path has no parent")?;
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)] {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(parent)?;
    }
    // Startup validates this directory before Session::open can create it.
    // Prepare it before consuming the one-use enrollment token.
    let mut state_directory = fs::DirBuilder::new();
    state_directory.recursive(true);
    #[cfg(unix)] {
        use std::os::unix::fs::DirBuilderExt;
        state_directory.mode(0o700);
    }
    state_directory.create(&config.node.state_directory)
        .context("cannot create collector state directory before enrollment")?;
    let metadata = fs::symlink_metadata(token_file).context("cannot inspect enrollment token file")?;
    ensure!(metadata.is_file() && !metadata.file_type().is_symlink(), "enrollment token must be a regular file, not a symlink");
    #[cfg(unix)] {
        use std::os::unix::fs::PermissionsExt;
        ensure!(metadata.permissions().mode() & 0o077 == 0, "enrollment token file must be owner-only (0600)");
    }
    let token_bytes = config::read_bounded(token_file, 4096)?;
    let token = std::str::from_utf8(&token_bytes).context("invalid enrollment token file")?.trim();
    let _ = bearer_header(token).context("invalid enrollment token")?;
    let mut url = reqwest::Url::parse(&config.heartbeat.endpoint)?;
    ensure!(url.path() == "/api/v2/ciderd/heartbeat", "Cider configuration must use /api/v2/ciderd/heartbeat");
    url.set_path("/api/v1/nodes/enroll");
    let mut client = reqwest::Client::builder()
        .https_only(true).no_proxy().redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .connect_timeout(Duration::from_secs(config.heartbeat.connect_timeout_seconds))
        .timeout(Duration::from_secs(config.heartbeat.request_timeout_seconds))
        .user_agent(concat!("ciderd/", env!("CARGO_PKG_VERSION")));
    if let Some(path) = &config.heartbeat.ca_certificate_file {
        let metadata = fs::symlink_metadata(path).context("cannot inspect CA certificate file")?;
        ensure!(metadata.is_file() && !metadata.file_type().is_symlink(), "CA certificate must be a regular file");
        let certificates = reqwest::Certificate::from_pem_bundle(&config::read_bounded(path, 65536)?)?;
        ensure!(!certificates.is_empty() && certificates.len() <= 64, "CA file must contain 1..64 certificates");
        for certificate in certificates { client = client.add_root_certificate(certificate); }
    }
    // Enrollment is deliberately not retried: a lost response may have consumed the token.
    let mut response = client.build()?.post(url)
        .header("X-Request-ID", uuid::Uuid::new_v4().to_string())
        .header("X-Request-Timestamp", chrono::Utc::now().to_rfc3339())
        .json(&json!({"enrollment_token":token,"name":name,"agent":{"version":env!("CARGO_PKG_VERSION")}}))
        .send().await.context("enrollment request failed; it may have succeeded remotely, so do not retry blindly")?;
    ensure!(response.status() == reqwest::StatusCode::CREATED, "enrollment returned HTTP {}; response body omitted to protect credentials", response.status().as_u16());
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.context("enrollment response was lost; use a new token after administrator review")? {
        ensure!(chunk.len() <= 16384usize.saturating_sub(bytes.len()), "enrollment response exceeded 16 KiB");
        bytes.extend_from_slice(&chunk);
    }
    let enrollment: Envelope = parse_json(&bytes).context("invalid enrollment response; the token may already be consumed")?;
    uuid::Uuid::parse_str(&enrollment.data.node_id).context("server returned an invalid node ID")?;
    let _ = bearer_header(&enrollment.data.credential).context("server returned an invalid credential")?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)] {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options.open(credential_file)
        .context("server enrolled the node but local credential creation failed; administrator recovery is required")?;
    file.write_all(enrollment.data.credential.as_bytes())?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    File::open(credential_file.parent().context("credential parent missing")?)?.sync_all()?;
    identity::enroll(identity_file, Some(&enrollment.data.node_id))
        .with_context(|| format!("server enrolled node {}; credential is saved, but identity creation failed; recover explicitly without overwriting existing identity", enrollment.data.node_id))?;
    Ok(enrollment.data.node_id)
}
