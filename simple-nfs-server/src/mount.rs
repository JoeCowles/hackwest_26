//! Client-side mount of the cluster export.

use crate::config::Config;
use crate::sys;
use anyhow::{Context, Result, ensure};
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
pub fn parse_mount_line(line: &str, expected_mount_point: Option<&str>) -> Option<NfsMount> {
    let (paths, flags) = line.rsplit_once(" (")?;
    let options = flags.strip_suffix(')')?;
    // A configured mountpoint supplies the delimiter's meaning even when the
    // export's directory name itself includes " on ".
    let (source, mount_point) = if let Some(mount_point) = expected_mount_point {
        (
            paths.strip_suffix(&format!(" on {mount_point}"))?,
            mount_point,
        )
    } else {
        paths.split_once(" on ")?
    };
    Some(NfsMount {
        source: source.to_string(),
        mount_point: mount_point.to_string(),
        options: options.to_string(),
    })
}

async fn list_mounts(mount_point: &str) -> Result<Vec<NfsMount>> {
    // Include local filesystems too: mounting over a different filesystem must
    // not be treated as an empty target.
    let out = sys::run("mount", &[]).await?;
    Ok(out
        .lines()
        .filter_map(|line| parse_mount_line(line, Some(mount_point)))
        .collect())
}

fn matching_mount<'a>(
    mounts: &'a [NfsMount],
    mount_point: &str,
    expected_source: &str,
) -> Result<Option<&'a NfsMount>> {
    let mut matching = mounts
        .iter()
        .filter(|mount| mount.mount_point == mount_point);
    let mount = matching.next();
    ensure!(
        matching.next().is_none(),
        "multiple mounts occupy {mount_point}; inspect them before proceeding"
    );
    if let Some(mount) = mount {
        ensure!(
            mount.source == expected_source
                && mount.options.split(',').any(|flag| flag.trim() == "nfs"),
            "{mount_point} is mounted from {}; expected NFS source {expected_source}",
            mount.source
        );
    }
    Ok(mount)
}

fn mount_source(cfg: &Config) -> String {
    let server = &cfg.client.nfs_server;
    let server = if server.contains(':') {
        format!("[{server}]")
    } else {
        server.clone()
    };
    format!("{server}:{}", cfg.server.export_path.display())
}

pub async fn mount(cfg: &Config) -> Result<()> {
    sys::require_macos()?;
    sys::require_root("mount")?;

    let mp = cfg.client.mount_point.display().to_string();
    let target = mount_source(cfg);
    if let Some(existing) = matching_mount(&list_mounts(&mp).await?, &mp, &target)? {
        info!(
            "already mounted at {mp}: {} ({})",
            existing.source, existing.options
        );
        return Ok(());
    }

    std::fs::create_dir_all(&cfg.client.mount_point)?;

    let opts = cfg.mount_options();
    info!("mounting {target} -> {mp} (-o {opts})");
    sys::run("mount", &["-t", "nfs", "-o", &opts, &target, &mp]).await?;

    let mounts = list_mounts(&mp).await?;
    let m = matching_mount(&mounts, &mp, &target)?
        .with_context(|| format!("mount command succeeded but {target} is not present at {mp}"))?;
    println!(
        "mounted: {} on {}; observed mount flags: {}",
        m.source, m.mount_point, m.options
    );
    println!("to unmount: sudo umount {mp}");
    Ok(())
}

pub async fn unmount(cfg: &Config) -> Result<()> {
    sys::require_macos()?;
    sys::require_root("unmount")?;
    let mp = cfg.client.mount_point.display().to_string();
    if matching_mount(&list_mounts(&mp).await?, &mp, &mount_source(cfg))?.is_none() {
        info!("no mount at {mp}; nothing to unmount");
        return Ok(());
    }
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
            parse_mount_line(line, None),
            Some(NfsMount {
                source: "192.168.1.10:/Volumes/easystore/nfs-export".into(),
                mount_point: "/Users/Shared/nfs/cluster".into(),
                options: "nfs, nodev, nosuid, mounted by graysen".into(),
            })
        );
    }

    #[test]
    fn rejects_non_mount_lines() {
        assert_eq!(parse_mount_line("garbage", None), None);
        assert_eq!(parse_mount_line("", None), None);
    }

    #[test]
    fn an_existing_mount_must_match_the_configured_export() {
        let mounts =
            vec![parse_mount_line("storage:/other on /Volumes/share (nfs, nodev)", None).unwrap()];
        assert!(matching_mount(&mounts, "/Volumes/share", "storage:/expected").is_err());
        assert!(
            matching_mount(&mounts, "/Volumes/share", "storage:/other")
                .unwrap()
                .is_some()
        );
        assert!(
            matching_mount(&mounts, "/Volumes/absent", "storage:/expected")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn known_mountpoint_disambiguates_spaces_in_the_export_name() {
        let line = "storage:/Volumes/Data on Share on /Volumes/share (nfs, nodev)";
        let mount = parse_mount_line(line, Some("/Volumes/share")).unwrap();
        assert_eq!(mount.source, "storage:/Volumes/Data on Share");
        assert_eq!(mount.mount_point, "/Volumes/share");
    }
}
