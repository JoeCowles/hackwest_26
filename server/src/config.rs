use anyhow::{Context, Result};
use clap::Parser;
use fs2::FileExt;
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    net::SocketAddr,
    path::PathBuf,
};

#[derive(Parser, Debug, Clone)]
#[command(version, about = "Orchard storage telemetry server for macOS")]
pub struct Config {
    #[arg(long, env = "ORCHARD_BIND", default_value = "127.0.0.1:8787")]
    pub bind: SocketAddr,
    #[arg(long, env = "ORCHARD_DATA_DIR")]
    pub data_dir: Option<PathBuf>,
    #[arg(long)]
    pub headless: bool,
    #[arg(long, env = "ORCHARD_TLS_CERT", requires = "tls_key")]
    pub tls_cert: Option<PathBuf>,
    #[arg(long, env = "ORCHARD_TLS_KEY", requires = "tls_cert")]
    pub tls_key: Option<PathBuf>,
}

pub struct LocalFiles {
    pub directory: PathBuf,
    pub admin_token: String,
    pub credential_path: PathBuf,
    pub _instance_lock: File,
}

impl Config {
    pub fn prepare(&self) -> Result<LocalFiles> {
        anyhow::ensure!(
            self.bind.ip().is_loopback() || self.tls_cert.is_some(),
            "A non-loopback bind requires --tls-cert and --tls-key; use loopback for local HTTP development"
        );
        anyhow::ensure!(
            self.tls_cert.is_some() == self.tls_key.is_some(),
            "Both TLS certificate and key are required"
        );
        let directory = self.data_dir.clone().unwrap_or_else(|| {
            dirs::data_local_dir()
                .unwrap_or_else(std::env::temp_dir)
                .join("Orchard Server")
        });
        fs::create_dir_all(&directory)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
        }
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(directory.join("server.lock"))?;
        lock.try_lock_exclusive()
            .context("Another Orchard Server is already using this data directory")?;
        let credential_path = directory.join("admin-token");
        let admin_token = if credential_path.exists() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                anyhow::ensure!(
                    fs::symlink_metadata(&credential_path)?
                        .file_type()
                        .is_file(),
                    "Admin credential must be a regular file"
                );
                fs::set_permissions(&credential_path, fs::Permissions::from_mode(0o600))?;
            }
            let mut value = String::new();
            File::open(&credential_path)?.read_to_string(&mut value)?;
            value.trim().to_string()
        } else {
            let value = crate::store::secret("admin");
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&credential_path)?;
            file.write_all(value.as_bytes())?;
            file.sync_all()?;
            value
        };
        anyhow::ensure!(
            admin_token.len() >= 32,
            "Admin credential must contain at least 32 bytes"
        );
        Ok(LocalFiles {
            directory,
            admin_token,
            credential_path,
            _instance_lock: lock,
        })
    }
}
