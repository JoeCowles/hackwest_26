//! Thin wrappers around the system commands this tool depends on.

use anyhow::{Context, Result, bail};
use tokio::process::Command;

pub fn is_root() -> bool {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() == 0 }
}

pub fn require_root(what: &str) -> Result<()> {
    if !is_root() {
        bail!("{what} must run as root (sudo)");
    }
    Ok(())
}

pub fn require_macos() -> Result<()> {
    if !cfg!(target_os = "macos") {
        bail!("macOS only");
    }
    Ok(())
}

/// Run a command to completion and return stdout, failing on non-zero exit.
pub async fn run(program: &str, args: &[&str]) -> Result<String> {
    let out = Command::new(program)
        .args(args)
        .output()
        .await
        .with_context(|| format!("spawning {program}"))?;
    if !out.status.success() {
        bail!(
            "{program} {} failed ({}): {}",
            args.join(" "),
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Like `run`, but a non-zero exit is reported as `Ok(false)` instead of an error.
pub async fn run_ok(program: &str, args: &[&str]) -> Result<bool> {
    let status = Command::new(program)
        .args(args)
        .status()
        .await
        .with_context(|| format!("spawning {program}"))?;
    Ok(status.success())
}
