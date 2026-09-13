use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
pub struct Config {
    pub server: ServerConfig,
    pub client: ClientConfig,
}

#[derive(Debug, Deserialize)]
pub struct ServerConfig {
    /// Directory to export. Must exist on a local volume.
    pub export_path: PathBuf,
    /// Subnet form. Mutually exclusive with `allowed_hosts`.
    #[serde(default)]
    pub allowed_network: Option<String>,
    #[serde(default)]
    pub allowed_mask: Option<String>,
    /// Host-list form. Mutually exclusive with `allowed_network`/`allowed_mask`.
    #[serde(default)]
    pub allowed_hosts: Vec<String>,
    /// `mapall`, `maproot`, or absent for no identity mapping.
    #[serde(default)]
    pub map_mode: Option<MapMode>,
    #[serde(default)]
    pub map_identity: Option<String>,
    #[serde(default)]
    pub extra_export_opts: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MapMode {
    Mapall,
    Maproot,
}

#[derive(Debug, Deserialize)]
pub struct ClientConfig {
    pub nfs_server: String,
    pub mount_point: PathBuf,
    #[serde(default = "default_vers")]
    pub nfs_vers: String,
    #[serde(default = "default_mount_opts")]
    pub mount_opts: Vec<String>,
}

fn default_vers() -> String {
    "3".to_string()
}

/// `resvport` is required against a macOS-served export; omitting it surfaces as
/// a permission error that reads like an ACL problem and is not one.
fn default_mount_opts() -> Vec<String> {
    ["resvport", "rw", "hard", "intr"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("reading config at {}", path.display()))?;
        let cfg: Config = toml::from_str(&raw)
            .with_context(|| format!("parsing config at {}", path.display()))?;
        cfg.validate()?;
        Ok(cfg)
    }

    fn validate(&self) -> Result<()> {
        let s = &self.server;

        let has_subnet = s.allowed_network.is_some() || s.allowed_mask.is_some();
        let has_hosts = !s.allowed_hosts.is_empty();

        if has_subnet && has_hosts {
            bail!("set allowed_network/allowed_mask or allowed_hosts, not both");
        }
        if !has_subnet && !has_hosts {
            bail!(
                "no client restriction configured: set allowed_network + allowed_mask, \
                 or allowed_hosts. There is deliberately no default that exports to everyone."
            );
        }
        if has_subnet && (s.allowed_network.is_none() || s.allowed_mask.is_none()) {
            bail!("allowed_network and allowed_mask must be set together");
        }
        if s.map_mode.is_some() && s.map_identity.is_none() {
            bail!("map_mode requires map_identity");
        }
        Ok(())
    }

    /// The `/etc/exports` line this configuration describes.
    pub fn export_line(&self) -> String {
        let s = &self.server;
        let mut parts = vec![s.export_path.display().to_string()];

        if let (Some(mode), Some(identity)) = (s.map_mode, s.map_identity.as_ref()) {
            let flag = match mode {
                MapMode::Mapall => "mapall",
                MapMode::Maproot => "maproot",
            };
            parts.push(format!("-{flag}={identity}"));
        }

        parts.extend(s.extra_export_opts.iter().cloned());

        match (&s.allowed_network, &s.allowed_mask) {
            (Some(net), Some(mask)) => {
                parts.push(format!("-network {net}"));
                parts.push(format!("-mask {mask}"));
            }
            _ => parts.extend(s.allowed_hosts.iter().cloned()),
        }

        parts.join(" ")
    }

    /// Mount options as passed to `mount -t nfs -o`.
    pub fn mount_options(&self) -> String {
        let mut opts = vec![format!("vers={}", self.client.nfs_vers)];
        opts.extend(self.client.mount_opts.iter().cloned());
        opts.join(",")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Config {
        Config {
            server: ServerConfig {
                export_path: PathBuf::from("/Volumes/easystore/nfs-export"),
                allowed_network: Some("192.168.1.0".into()),
                allowed_mask: Some("255.255.255.0".into()),
                allowed_hosts: vec![],
                map_mode: Some(MapMode::Mapall),
                map_identity: Some("nobody:nobody".into()),
                extra_export_opts: vec![],
            },
            client: ClientConfig {
                nfs_server: "192.168.1.10".into(),
                mount_point: PathBuf::from("/Users/Shared/nfs/cluster"),
                nfs_vers: default_vers(),
                mount_opts: default_mount_opts(),
            },
        }
    }

    #[test]
    fn export_line_subnet_form() {
        let cfg = base();
        assert_eq!(
            cfg.export_line(),
            "/Volumes/easystore/nfs-export -mapall=nobody:nobody -network 192.168.1.0 -mask 255.255.255.0"
        );
    }

    #[test]
    fn export_line_host_form() {
        let mut cfg = base();
        cfg.server.allowed_network = None;
        cfg.server.allowed_mask = None;
        cfg.server.allowed_hosts = vec!["10.0.0.11".into(), "10.0.0.12".into()];
        assert_eq!(
            cfg.export_line(),
            "/Volumes/easystore/nfs-export -mapall=nobody:nobody 10.0.0.11 10.0.0.12"
        );
    }

    #[test]
    fn rejects_export_with_no_client_restriction() {
        let mut cfg = base();
        cfg.server.allowed_network = None;
        cfg.server.allowed_mask = None;
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("no client restriction"), "got: {err}");
    }

    #[test]
    fn rejects_both_access_forms() {
        let mut cfg = base();
        cfg.server.allowed_hosts = vec!["10.0.0.11".into()];
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn mount_options_lead_with_version() {
        assert_eq!(base().mount_options(), "vers=3,resvport,rw,hard,intr");
    }
}
