use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use std::net::IpAddr;
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
        Self::parse(&raw).with_context(|| format!("parsing config at {}", path.display()))
    }

    fn parse(raw: &str) -> Result<Self> {
        let mut cfg: Config = toml::from_str(raw)?;
        cfg.validate()?;
        // Match native mount output without probing/canonicalizing remote paths.
        cfg.server.export_path = cfg.server.export_path.components().collect();
        cfg.client.mount_point = cfg.client.mount_point.components().collect();
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
        for path in [&s.export_path, &self.client.mount_point] {
            let text = path.to_str().context("paths must be UTF-8")?;
            ensure!(
                path.is_absolute()
                    && !text.chars().any(char::is_control)
                    && !text.split('/').any(|part| matches!(part, "." | "..")),
                "paths must be absolute and cannot contain controls, . or .. components"
            );
        }
        if let (Some(network), Some(mask)) = (&s.allowed_network, &s.allowed_mask) {
            let network = network
                .parse::<IpAddr>()
                .context("invalid network address")?;
            let mask = mask.parse::<IpAddr>().context("invalid network mask")?;
            let (network, mask, maximum) = match (network, mask) {
                (IpAddr::V4(network), IpAddr::V4(mask)) => (
                    u32::from(network) as u128,
                    u32::from(mask) as u128,
                    u32::MAX as u128,
                ),
                (IpAddr::V6(network), IpAddr::V6(mask)) => {
                    (u128::from(network), u128::from(mask), u128::MAX)
                }
                _ => bail!("network and mask must use the same address family"),
            };
            let inverse = maximum ^ mask;
            ensure!(
                mask != 0 && inverse.checked_add(1).is_some_and(u128::is_power_of_two),
                "network mask must be contiguous and must restrict clients"
            );
            ensure!(network & inverse == 0, "network address has host bits set");
        }
        for host in s
            .allowed_hosts
            .iter()
            .chain(std::iter::once(&self.client.nfs_server))
        {
            ensure!(
                valid_host(host),
                "host must be an IP address or a single DNS name"
            );
        }
        if let Some(identity) = &s.map_identity {
            ensure!(
                s.map_mode.is_some()
                    && !identity.is_empty()
                    && !identity.starts_with(':')
                    && identity
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"-_.:".contains(&b)),
                "map_identity requires a mapping mode and user/group names or numeric IDs"
            );
        }
        for option in &s.extra_export_opts {
            ensure!(
                valid_option(option) && option.starts_with('-') && option.len() > 1,
                "each export option must be one nonempty option token"
            );
            let name = option.split('=').next().unwrap_or(option);
            ensure!(
                !matches!(name, "-network" | "-mask" | "-mapall" | "-maproot" | "-r"),
                "client restrictions and identity mapping must use their dedicated config fields"
            );
        }
        ensure!(
            matches!(self.client.nfs_vers.as_str(), "2" | "3" | "4"),
            "nfs_vers must be 2, 3, or 4"
        );
        for option in &self.client.mount_opts {
            ensure!(
                valid_option(option)
                    && !option.starts_with('-')
                    && !option.starts_with("vers=")
                    && !option.starts_with("nfsvers="),
                "each mount option must be one option token; configure the version with nfs_vers"
            );
        }
        Ok(())
    }

    /// The `/etc/exports` line this configuration describes.
    pub fn export_line(&self) -> String {
        let s = &self.server;
        // exports(5) uses whitespace-delimited fields and backslash escaping.
        let mut path = String::new();
        for character in s.export_path.to_string_lossy().chars() {
            if matches!(character, ' ' | '\\' | '\'' | '"' | '#') {
                path.push('\\');
            }
            path.push(character);
        }
        let mut parts = vec![path];

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

fn valid_option(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.=:".contains(&b))
}

fn valid_host(value: &str) -> bool {
    if value.parse::<IpAddr>().is_ok() {
        return true;
    }
    let value = value.strip_suffix('.').unwrap_or(value);
    !value.is_empty()
        && value.len() <= 253
        && value.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
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

    #[test]
    fn empty_or_whitespace_host_restrictions_are_rejected() {
        for host in ["", " ", "client one"] {
            let mut cfg = base();
            cfg.server.allowed_network = None;
            cfg.server.allowed_mask = None;
            cfg.server.allowed_hosts = vec![host.into()];
            assert!(cfg.validate().is_err(), "accepted host {host:?}");
        }
    }

    #[test]
    fn export_paths_with_spaces_and_quotes_are_escaped_as_one_field() {
        let mut cfg = base();
        cfg.server.export_path = "/Volumes/External Disk/Owner's Files".into();
        cfg.validate().unwrap();
        assert!(
            cfg.export_line()
                .starts_with("/Volumes/External\\ Disk/Owner\\'s\\ Files ")
        );
    }

    #[test]
    fn malformed_paths_subnets_and_option_tokens_are_rejected() {
        let mut configs = Vec::new();
        let mut cfg = base();
        cfg.server.export_path = "relative/export".into();
        configs.push(cfg);
        let mut cfg = base();
        cfg.client.mount_point = "/Volumes/../other".into();
        configs.push(cfg);
        let mut cfg = base();
        cfg.server.allowed_mask = Some("255.0.255.0".into());
        configs.push(cfg);
        let mut cfg = base();
        cfg.server.extra_export_opts = vec!["-ro extra".into()];
        configs.push(cfg);
        let mut cfg = base();
        cfg.client.mount_opts = vec!["rw,hard".into()];
        configs.push(cfg);
        let mut cfg = base();
        cfg.client.nfs_server = String::new();
        configs.push(cfg);
        for cfg in configs {
            assert!(cfg.validate().is_err(), "accepted malformed config {cfg:?}");
        }
    }

    #[test]
    fn numeric_ipv6_subnet_restrictions_remain_supported() {
        let mut cfg = base();
        cfg.server.allowed_network = Some("2001:db8::".into());
        cfg.server.allowed_mask = Some("ffff:ffff::".into());
        cfg.validate().unwrap();
        assert!(
            cfg.export_line()
                .ends_with("-network 2001:db8:: -mask ffff:ffff::")
        );
    }

    #[test]
    fn loaded_paths_remove_duplicate_separators_and_trailing_slashes() {
        let config = include_str!("../config.example.toml")
            .replace(
                "/Volumes/easystore/nfs-export",
                "/Volumes//easystore/nfs-export/",
            )
            .replace("/Users/Shared/nfs/cluster", "/Users//Shared/nfs/cluster/");
        let cfg = Config::parse(&config).unwrap();
        assert_eq!(
            cfg.server.export_path.to_str().unwrap(),
            "/Volumes/easystore/nfs-export"
        );
        assert_eq!(
            cfg.client.mount_point.to_str().unwrap(),
            "/Users/Shared/nfs/cluster"
        );
    }
}
