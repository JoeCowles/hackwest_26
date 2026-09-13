use anyhow::{Context, Result};
use clap::{CommandFactory, FromArgMatches, Parser};
use fs2::FileExt;
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    net::SocketAddr,
    path::{Path, PathBuf},
};

#[derive(Parser, Debug, Clone)]
#[command(version, about = "Cider storage telemetry server for macOS")]
pub struct Config {
    #[arg(long, env = "CIDER_BIND", default_value = "127.0.0.1:8787")]
    pub bind: SocketAddr,
    #[arg(long, env = "CIDER_DATA_DIR")]
    pub data_dir: Option<PathBuf>,
    #[arg(long)]
    pub headless: bool,
    #[arg(long, env = "CIDER_TLS_CERT", requires = "tls_key")]
    pub tls_cert: Option<PathBuf>,
    #[arg(long, env = "CIDER_TLS_KEY", requires = "tls_cert")]
    pub tls_key: Option<PathBuf>,
}

/// Reuse the credential already copied by the operator, including files from
/// releases that rewrote viewer-token at startup. Replacement is explicit.
pub(crate) fn load_or_create_viewer_token(directory: &Path) -> Result<String> {
    let path = directory.join("viewer-token");
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = match options.open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let token = crate::store::secret("viewer");
            let mut create = OpenOptions::new();
            create.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                create.mode(0o600).custom_flags(libc::O_NOFOLLOW);
            }
            let mut file = create
                .open(&path)
                .context("Cannot create viewer credential")?;
            file.write_all(token.as_bytes())?;
            file.sync_all()?;
            #[cfg(unix)]
            File::open(directory)?.sync_all()?;
            return Ok(token);
        }
        Err(error) => return Err(error).context("Cannot open viewer credential"),
    };
    let metadata = file.metadata()?;
    anyhow::ensure!(
        metadata.is_file(),
        "Viewer credential must be a regular file"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        anyhow::ensure!(
            metadata.mode() & 0o077 == 0,
            "Viewer credential must be owner-only (0600)"
        );
        // SAFETY: geteuid has no preconditions and does not mutate process state.
        anyhow::ensure!(
            metadata.uid() == unsafe { libc::geteuid() },
            "Viewer credential must belong to the server user"
        );
    }
    let mut bytes = Vec::new();
    file.take(4097).read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() <= 4096,
        "Viewer credential file exceeds 4096 bytes"
    );
    let value = std::str::from_utf8(&bytes)
        .context("Viewer credential must contain ASCII text")?
        .trim();
    anyhow::ensure!(
        value.len() >= 32
            && value.strip_prefix("viewer_").is_some_and(|suffix| suffix
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))),
        "Viewer credential must contain at least 32 bytes and use viewer_ followed by ASCII letters, digits, underscores, or hyphens"
    );
    Ok(value.to_owned())
}

pub struct LocalFiles {
    pub directory: PathBuf,
    pub admin_token: String,
    pub credential_path: PathBuf,
    pub _instance_lock: File,
}

pub fn default_data_directory(base: &Path) -> Result<PathBuf> {
    compatible_path(
        &base.join("Cider Server"),
        &base.join("Orchard Server"),
        true,
    )
}

pub fn database_path(directory: &Path) -> Result<PathBuf> {
    compatible_path(
        &directory.join("cider.sqlite3"),
        &directory.join("orchard.sqlite3"),
        false,
    )
}

/// Compatibility selects an existing instance without moving or copying it.
/// Preserve the legacy names here when changing product-facing labels.
fn compatible_path(current: &Path, legacy: &Path, directory: bool) -> Result<PathBuf> {
    let exists = |path: &Path| -> Result<bool> {
        match fs::symlink_metadata(path) {
            Ok(metadata) => {
                anyhow::ensure!(
                    if directory {
                        metadata.is_dir()
                    } else {
                        metadata.is_file()
                    },
                    "Server data path must be a {} without symlinks: {}",
                    if directory {
                        "directory"
                    } else {
                        "regular file"
                    },
                    path.display()
                );
                Ok(true)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error)
                .with_context(|| format!("Cannot inspect server data path {}", path.display())),
        }
    };
    let current_exists = exists(current)?;
    let legacy_exists = exists(legacy)?;
    anyhow::ensure!(
        !(current_exists && legacy_exists),
        "Both current and legacy server data paths exist: {} and {}; select or resolve the intended instance explicitly",
        current.display(),
        legacy.display()
    );
    Ok(if legacy_exists { legacy } else { current }.to_path_buf())
}

impl Config {
    pub fn parse_compatible() -> Self {
        let mut command = Self::command();
        // Keep legacy environment names for existing service definitions. Clap
        // still gives explicit command-line flags precedence over either name.
        for (argument, current, legacy) in [
            ("bind", "CIDER_BIND", "ORCHARD_BIND"),
            ("data_dir", "CIDER_DATA_DIR", "ORCHARD_DATA_DIR"),
            ("tls_cert", "CIDER_TLS_CERT", "ORCHARD_TLS_CERT"),
            ("tls_key", "CIDER_TLS_KEY", "ORCHARD_TLS_KEY"),
        ] {
            if std::env::var_os(current).is_none() && std::env::var_os(legacy).is_some() {
                command = command.mut_arg(argument, |argument| argument.env(legacy));
            }
        }
        Self::from_arg_matches(&command.get_matches()).unwrap_or_else(|error| error.exit())
    }

    pub fn prepare(&self) -> Result<LocalFiles> {
        anyhow::ensure!(
            self.bind.ip().is_loopback() || self.tls_cert.is_some(),
            "A non-loopback bind requires --tls-cert and --tls-key; use loopback for local HTTP development"
        );
        anyhow::ensure!(
            self.tls_cert.is_some() == self.tls_key.is_some(),
            "Both TLS certificate and key are required"
        );
        let directory = match &self.data_dir {
            Some(directory) => directory.clone(),
            None => {
                default_data_directory(&dirs::data_local_dir().unwrap_or_else(std::env::temp_dir))?
            }
        };
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
            .context("Another Cider Server is already using this data directory")?;
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
