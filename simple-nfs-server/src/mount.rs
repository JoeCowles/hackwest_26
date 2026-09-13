//! Client-side mount of the cluster export.

use crate::config::Config;
use crate::sys;
use anyhow::Result;
use tracing::info;

/// One line of `mount -t nfs` output, parsed.
#[derive(Debug, PartialEq, Eq)]
pub struct NfsMount {
    pub source: String,
    pub mount_point: String,
    pub options: String,
}

/// Parse a line such as
/// `192.168.1.10:/export on /Users/Shared/nfs/cluster (nfs, nodev, nosuid, mounted by g)`.
pub fn parse_mount_line(line: &str) -> Option<NfsMount> {
    let (source, rest) = line.split_once(" on ")?;
    let (mount_point, rest) = rest.split_once(" (")?;
    let options = rest.strip_suffix(')').unwrap_or(rest);
    Some(NfsMount {
        source: source.to_string(),
        mount_point: mount_point.to_string(),
        options: options.to_string(),
    })
}

pub async fn list_nfs_mounts() -> Result<Vec<NfsMount>> {
    let out = sys::run("mount", &["-t", "nfs"]).await?;
    Ok(out.lines().filter_map(parse_mount_line).collect())
}

pub async fn mount(cfg: &Config) -> Result<()> {
    sys::require_macos()?;
    sys::require_root("mount")?;

    let mp = cfg.client.mount_point.display().to_string();
    if let Some(existing) = list_nfs_mounts()
        .await?
        .into_iter()
        .find(|m| m.mount_point == mp)
    {
        info!(
            "already mounted at {mp}: {} ({})",
            existing.source, existing.options
        );
        return Ok(());
    }

    std::fs::create_dir_all(&cfg.client.mount_point)?;

    let target = format!(
        "{}:{}",
        cfg.client.nfs_server,
        cfg.server.export_path.display()
    );
    let opts = cfg.mount_options();
    info!("mounting {target} -> {mp} (-o {opts})");
    sys::run("mount", &["-t", "nfs", "-o", &opts, &target, &mp]).await?;

    // Report what was negotiated, not what was requested. macOS falls back on
    // version and transport without saying so.
    if let Some(m) = list_nfs_mounts()
        .await?
        .into_iter()
        .find(|m| m.mount_point == mp)
    {
        println!(
            "negotiated: {} on {} ({})",
            m.source, m.mount_point, m.options
        );
    }
    println!("to unmount: sudo umount {mp}");
    Ok(())
}

pub async fn unmount(cfg: &Config) -> Result<()> {
    sys::require_root("unmount")?;
    let mp = cfg.client.mount_point.display().to_string();
    sys::run("umount", &[&mp]).await?;
    info!("unmounted {mp}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_typical_macos_mount_line() {
        let line = "192.168.1.10:/Volumes/easystore/nfs-export on /Users/Shared/nfs/cluster (nfs, nodev, nosuid, mounted by graysen)";
        assert_eq!(
            parse_mount_line(line),
            Some(NfsMount {
                source: "192.168.1.10:/Volumes/easystore/nfs-export".into(),
                mount_point: "/Users/Shared/nfs/cluster".into(),
                options: "nfs, nodev, nosuid, mounted by graysen".into(),
            })
        );
    }

    #[test]
    fn rejects_non_mount_lines() {
        assert_eq!(parse_mount_line("garbage"), None);
        assert_eq!(parse_mount_line(""), None);
    }
}
