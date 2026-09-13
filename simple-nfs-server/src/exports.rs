//! `/etc/exports` management.
//!
//! Only the marked block is ever rewritten. Hand-written exports outside it are
//! preserved verbatim, the file is backed up before every change, the result is
//! validated with `nfsd checkexports`, and the backup is restored if validation
//! fails.

use crate::config::Config;
use crate::sys;
use anyhow::{Context, Result, ensure};
use std::path::{Path, PathBuf};
use tracing::info;

pub const EXPORTS: &str = "/etc/exports";
const BEGIN_MARK: &str = "# >>> orchard-nfs managed block >>>";
const END_MARK: &str = "# <<< orchard-nfs managed block <<<";

/// Return `content` with the managed block removed. Everything else is untouched.
pub fn strip_managed_block(content: &str) -> Result<String> {
    let mut out = String::with_capacity(content.len());
    let mut skipping = false;
    let mut seen = false;
    for original in content.split_inclusive('\n') {
        let line = original.trim_end_matches(['\r', '\n']);
        ensure!(
            !matches!(line.trim(), BEGIN_MARK | END_MARK) || line == line.trim(),
            "managed export markers must occupy an exact line; original exports retained"
        );
        if line == BEGIN_MARK {
            ensure!(
                !seen && !skipping,
                "duplicate or nested managed export block; original exports retained"
            );
            seen = true;
            skipping = true;
            continue;
        }
        if line == END_MARK {
            ensure!(
                skipping,
                "unmatched managed export block end; original exports retained"
            );
            skipping = false;
            continue;
        }
        if !skipping {
            out.push_str(original);
        }
    }
    ensure!(
        !skipping,
        "unclosed managed export block; original exports retained"
    );
    Ok(out)
}

/// Return `content` with the managed block replaced by one containing `export_line`.
pub fn with_managed_block(content: &str, export_line: &str) -> Result<String> {
    let mut out = strip_managed_block(content)?;
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(BEGIN_MARK);
    out.push('\n');
    out.push_str(&format!(
        "# generated {} by simple-nfs-server setup-server\n",
        chrono::Local::now().to_rfc3339()
    ));
    out.push_str(export_line);
    out.push('\n');
    out.push_str(END_MARK);
    out.push('\n');
    Ok(out)
}

pub fn has_managed_block(content: &str) -> bool {
    content.lines().any(|l| l == BEGIN_MARK)
}

/// Count export lines that are neither blank nor comments.
pub fn active_export_lines(content: &str) -> usize {
    content
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .count()
}

fn backup(path: &Path) -> Result<PathBuf> {
    let stamp = chrono::Local::now().format("%Y%m%d%H%M%S");
    let dest = PathBuf::from(format!("{}.orchard-backup.{stamp}", path.display()));
    std::fs::copy(path, &dest)
        .with_context(|| format!("backing up {} to {}", path.display(), dest.display()))?;
    info!("backed up {} -> {}", path.display(), dest.display());
    Ok(dest)
}

fn read_or_empty(path: &Path) -> Result<String> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

fn finish_validation(path: &Path, previous: Option<&str>, validation: Result<bool>) -> Result<()> {
    let error = match validation {
        Ok(true) => return Ok(()),
        Ok(false) => anyhow::anyhow!("nfsd checkexports rejected the configuration"),
        Err(error) => error.context("could not validate the new exports"),
    };
    let restored = if let Some(previous) = previous {
        std::fs::write(path, previous)
    } else {
        std::fs::remove_file(path)
    };
    if let Err(restore_error) = restored {
        return Err(error.context(format!("restoring previous exports also failed: {restore_error}; administrator recovery is required")));
    }
    Err(error.context("restored the previous exports file or its original absence"))
}

fn write_exports(path: &Path, next: &str, previous: Option<&str>) -> Result<()> {
    match std::fs::write(path, next) {
        Ok(()) => Ok(()),
        Err(error) => finish_validation(
            path,
            previous,
            Err(error).with_context(|| format!("writing {}", path.display())),
        ),
    }
}

pub async fn setup(cfg: &Config) -> Result<()> {
    sys::require_macos()?;
    sys::require_root("setup-server")?;

    let export_path = &cfg.server.export_path;
    if !export_path.is_dir() {
        anyhow::bail!(
            "export_path does not exist or is not a directory: {}",
            export_path.display()
        );
    }

    let exports = Path::new(EXPORTS);
    let current = read_or_empty(exports)?;
    let line = cfg.export_line();
    let next = with_managed_block(&current, &line)?;
    let backup_path = if exports.exists() {
        Some(backup(exports)?)
    } else {
        None
    };

    write_exports(
        exports,
        &next,
        backup_path.as_ref().map(|_| current.as_str()),
    )?;
    info!("wrote export: {line}");

    finish_validation(
        exports,
        backup_path.as_ref().map(|_| current.as_str()),
        sys::run_ok("nfsd", &["checkexports"]).await,
    )?;

    sys::run("nfsd", &["enable"]).await?;
    if sys::run_ok("nfsd", &["status"]).await? {
        sys::run("nfsd", &["update"]).await?;
        info!("nfsd running; exports reloaded");
    } else {
        sys::run("nfsd", &["start"]).await?;
        info!("nfsd started");
    }

    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    match sys::run("showmount", &["-e", "localhost"]).await {
        Ok(out) => println!("{out}"),
        Err(_) => info!("showmount returned nothing yet; give nfsd a moment and retry"),
    }
    Ok(())
}

pub async fn teardown(stop_nfsd: bool) -> Result<()> {
    sys::require_macos()?;
    sys::require_root("teardown-server")?;

    let exports = Path::new(EXPORTS);
    if !exports.exists() {
        info!("{EXPORTS} does not exist; nothing to do");
        return Ok(());
    }

    let current = std::fs::read_to_string(exports)?;
    let stripped = strip_managed_block(&current)?;
    if !has_managed_block(&current) {
        info!("no simple-nfs-server block in {EXPORTS}; nothing to remove");
    } else {
        backup(exports)?;
        write_exports(exports, &stripped, Some(&current))?;
        info!("removed simple-nfs-server block from {EXPORTS}");
        if sys::run_ok("nfsd", &["status"]).await? {
            sys::run("nfsd", &["update"]).await?;
            info!("exports reloaded");
        }
    }

    let remaining = active_export_lines(&std::fs::read_to_string(exports)?);
    info!("remaining export lines in {EXPORTS}: {remaining}");

    if stop_nfsd {
        if remaining > 0 {
            info!("refusing --stop-nfsd: {remaining} other export line(s) still present");
        } else {
            sys::run("nfsd", &["stop"]).await?;
            sys::run("nfsd", &["disable"]).await?;
            info!("nfsd stopped and disabled");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const HAND_WRITTEN: &str = "/srv/other -ro 10.0.0.5\n";

    #[test]
    fn strip_removes_only_managed_block() {
        let content = format!("{HAND_WRITTEN}{BEGIN_MARK}\n/managed -ro 1.2.3.4\n{END_MARK}\n");
        assert_eq!(strip_managed_block(&content).unwrap(), HAND_WRITTEN);
    }

    #[test]
    fn strip_is_noop_without_block() {
        assert_eq!(strip_managed_block(HAND_WRITTEN).unwrap(), HAND_WRITTEN);
    }

    #[test]
    fn with_block_is_idempotent() {
        let once = with_managed_block(HAND_WRITTEN, "/a -ro 1.1.1.1").unwrap();
        let twice = with_managed_block(&once, "/b -ro 2.2.2.2").unwrap();
        assert!(twice.starts_with(HAND_WRITTEN));
        assert!(twice.contains("/b -ro 2.2.2.2"));
        assert!(!twice.contains("/a -ro 1.1.1.1"));
        assert_eq!(twice.matches(BEGIN_MARK).count(), 1);
    }

    #[test]
    fn with_block_on_empty_file() {
        let out = with_managed_block("", "/x -ro 1.1.1.1").unwrap();
        assert!(out.starts_with(BEGIN_MARK));
        assert!(out.ends_with(&format!("{END_MARK}\n")));
    }

    #[test]
    fn active_lines_ignore_comments_and_blanks() {
        let content = "# comment\n\n/a -ro 1.1.1.1\n   \n/b 2.2.2.2\n";
        assert_eq!(active_export_lines(content), 2);
    }

    #[test]
    fn malformed_managed_blocks_are_rejected() {
        for content in [
            format!("{BEGIN_MARK}\n/managed\n"),
            format!("{END_MARK}\n{HAND_WRITTEN}"),
            format!("{BEGIN_MARK}\n{BEGIN_MARK}\n{END_MARK}\n"),
            format!("{BEGIN_MARK}\n{END_MARK}\n{BEGIN_MARK}\n{END_MARK}\n"),
            format!(" {BEGIN_MARK}\n/managed\n {END_MARK}\n"),
        ] {
            assert!(
                strip_managed_block(&content).is_err(),
                "accepted malformed markers"
            );
            assert!(with_managed_block(&content, "/new -ro client").is_err());
        }
    }

    #[test]
    fn removing_a_block_preserves_other_bytes_and_missing_final_newline() {
        let before = "# existing comment\r\n/other -ro client\r\n";
        let after = "# final comment";
        let content = format!("{before}{BEGIN_MARK}\n/managed\n{END_MARK}\n{after}");
        assert_eq!(
            strip_managed_block(&content).unwrap(),
            format!("{before}{after}")
        );
        assert_eq!(strip_managed_block(after).unwrap(), after);
    }

    #[test]
    fn validator_launch_failure_restores_previous_file_or_absence() {
        let path = std::env::temp_dir().join(format!(
            "orchard-nfs-validation-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(&self.0);
            }
        }
        let _cleanup = Cleanup(path.clone());
        for previous in [Some(HAND_WRITTEN), None] {
            std::fs::write(&path, "/new -ro client\n").unwrap();
            assert!(
                finish_validation(
                    &path,
                    previous,
                    Err(anyhow::anyhow!("validator unavailable"))
                )
                .is_err()
            );
            match previous {
                Some(content) => assert_eq!(std::fs::read_to_string(&path).unwrap(), content),
                None => assert!(!path.exists()),
            }
        }
    }
}
