//! launchd LaunchDaemon management for the monitor.

use crate::sys;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use tracing::info;

const LABEL: &str = "local.orchard-nfs.monitor";
const LOG_DIR: &str = "/usr/local/var/log/orchard-nfs";

fn plist_path() -> PathBuf {
    PathBuf::from(format!("/Library/LaunchDaemons/{LABEL}.plist"))
}

pub fn render_plist(binary: &Path, config: &Path) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{bin}</string>
    <string>--config</string>
    <string>{cfg}</string>
    <string>monitor</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>StandardOutPath</key>
  <string>{log}/stdout.log</string>
  <key>StandardErrorPath</key>
  <string>{log}/stderr.log</string>
</dict>
</plist>
"#,
        bin = binary.display(),
        cfg = config.display(),
        log = LOG_DIR,
    )
}

pub async fn install(config: &Path) -> Result<()> {
    sys::require_macos()?;
    sys::require_root("install-daemon")?;

    let binary = std::env::current_exe().context("resolving own binary path")?;
    let config = config
        .canonicalize()
        .with_context(|| format!("resolving config path {}", config.display()))?;

    std::fs::create_dir_all(LOG_DIR)?;

    let plist = plist_path();
    std::fs::write(&plist, render_plist(&binary, &config))?;
    sys::run("chown", &["root:wheel", &plist.display().to_string()]).await?;
    sys::run("chmod", &["644", &plist.display().to_string()]).await?;
    info!("wrote {}", plist.display());

    // Replace any previous instance before loading.
    let _ = sys::run_ok("launchctl", &["bootout", &format!("system/{LABEL}")]).await;
    sys::run("launchctl", &["bootstrap", "system", &plist.display().to_string()]).await?;
    info!("loaded {LABEL}; logs in {LOG_DIR}/");
    Ok(())
}

pub async fn uninstall() -> Result<()> {
    sys::require_macos()?;
    sys::require_root("uninstall-daemon")?;

    if !sys::run_ok("launchctl", &["bootout", &format!("system/{LABEL}")]).await? {
        info!("was not loaded");
    }
    let plist = plist_path();
    if plist.exists() {
        std::fs::remove_file(&plist)?;
        info!("removed {}", plist.display());
    }
    info!("logs left in place at {LOG_DIR}/");
    Ok(())
}

pub async fn status() -> Result<()> {
    let target = format!("system/{LABEL}");
    match sys::run("launchctl", &["print", &target]).await {
        Ok(out) => {
            println!("loaded");
            for line in out.lines() {
                let t = line.trim_start();
                if t.starts_with("state ") || t.starts_with("pid ") || t.starts_with("last exit ") {
                    println!("  {t}");
                }
            }
        }
        Err(_) => println!("not loaded"),
    }
    let plist = plist_path();
    println!(
        "plist {}: {}",
        if plist.exists() { "present" } else { "absent" },
        plist.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plist_embeds_binary_and_config() {
        let p = render_plist(Path::new("/opt/orchard-nfs"), Path::new("/etc/orchard/nfs.toml"));
        assert!(p.contains("<string>/opt/orchard-nfs</string>"));
        assert!(p.contains("<string>/etc/orchard/nfs.toml</string>"));
        assert!(p.contains("<string>monitor</string>"));
        assert!(p.contains(LABEL));
    }
}
