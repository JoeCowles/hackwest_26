use cider_server::config::{database_path, default_data_directory};
use std::{fs, process::Command};

#[test]
fn default_directory_keeps_existing_instance_and_rejects_ambiguity() {
    let dir = tempfile::tempdir().unwrap();
    let current = dir.path().join("Cider Server");
    let legacy = dir.path().join("Orchard Server");
    assert_eq!(default_data_directory(dir.path()).unwrap(), current);
    fs::create_dir(&legacy).unwrap();
    assert_eq!(default_data_directory(dir.path()).unwrap(), legacy);
    assert!(
        !current.exists(),
        "Reading the default must not copy or move data"
    );
    fs::create_dir(&current).unwrap();
    assert!(default_data_directory(dir.path()).is_err());
    fs::remove_dir(&legacy).unwrap();
    assert_eq!(default_data_directory(dir.path()).unwrap(), current);
}

#[test]
fn database_keeps_existing_filename_and_rejects_ambiguity() {
    let dir = tempfile::tempdir().unwrap();
    let current = dir.path().join("cider.sqlite3");
    let legacy = dir.path().join("orchard.sqlite3");
    assert_eq!(database_path(dir.path()).unwrap(), current);
    fs::write(&legacy, b"existing-database").unwrap();
    assert_eq!(database_path(dir.path()).unwrap(), legacy);
    assert_eq!(fs::read(&legacy).unwrap(), b"existing-database");
    assert!(!current.exists());
    fs::write(&current, b"other-database").unwrap();
    assert!(database_path(dir.path()).is_err());
    fs::remove_file(&legacy).unwrap();
    assert_eq!(database_path(dir.path()).unwrap(), current);
}

#[cfg(unix)]
#[test]
fn compatibility_paths_reject_symlinks_and_wrong_file_types() {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("orchard.sqlite3")).unwrap();
    assert!(database_path(dir.path()).is_err());
    let target = tempfile::tempdir().unwrap();
    symlink(target.path(), dir.path().join("Orchard Server")).unwrap();
    assert!(default_data_directory(dir.path()).is_err());
}

fn server() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cider-server"));
    for variable in [
        "CIDER_BIND",
        "CIDER_DATA_DIR",
        "CIDER_TLS_CERT",
        "CIDER_TLS_KEY",
        "ORCHARD_BIND",
        "ORCHARD_DATA_DIR",
        "ORCHARD_TLS_CERT",
        "ORCHARD_TLS_KEY",
    ] {
        command.env_remove(variable);
    }
    command
}

#[test]
fn command_line_overrides_current_env_which_overrides_legacy_env() {
    let cases = [
        (None, false, "legacy-invalid"),
        (None, true, "primary-invalid"),
        (Some("--bind=cli-invalid"), true, "cli-invalid"),
    ];
    for (argument, primary, expected) in cases {
        let mut command = server();
        command.env("ORCHARD_BIND", "legacy-invalid");
        if primary {
            command.env("CIDER_BIND", "primary-invalid");
        }
        if let Some(argument) = argument {
            command.arg(argument);
        }
        let output = command.output().unwrap();
        assert!(!output.status.success());
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(error.contains(expected), "{error}");
    }
}

#[test]
fn legacy_and_current_tls_environment_values_satisfy_pair_validation() {
    let mut command = server();
    let dir = tempfile::tempdir().unwrap();
    // Once both TLS arguments are present, startup reaches the explicit missing
    // data-directory failure. A parser regression fails first on --tls-key.
    let blocker = dir.path().join("not-a-directory");
    fs::write(&blocker, b"blocked").unwrap();
    let output = command
        .env("CIDER_TLS_CERT", "cert.pem")
        .env("ORCHARD_TLS_KEY", "key.pem")
        .env("ORCHARD_DATA_DIR", blocker.join("data"))
        .arg("--headless")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(
        !error.contains("required arguments were not provided"),
        "{error}"
    );
    assert!(error.contains("Not a directory"), "{error}");
}
