//! Named secrets ("moodle_token", "teams_refresh_token"): OS keyring first,
//! a 0600 `secrets.toml` when no keyring is reachable (headless Linux).

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::paths;
use crate::{Error, Result};

const SERVICE: &str = "study";

pub fn get(name: &str) -> Result<Option<String>> {
    let result = keyring::Entry::new(SERVICE, name).and_then(|entry| entry.get_password());
    match result {
        Ok(value) => Ok(Some(value)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(err) if store_unavailable(&err) => file_get(&file_path(), name),
        Err(err) => Err(Error::Keyring(err.to_string())),
    }
}

pub fn set(name: &str, value: &str) -> Result<()> {
    let result = keyring::Entry::new(SERVICE, name).and_then(|entry| entry.set_password(value));
    match result {
        Ok(()) => Ok(()),
        Err(err) if store_unavailable(&err) => file_set(&file_path(), name, value),
        Err(err) => Err(Error::Keyring(err.to_string())),
    }
}

pub fn delete(name: &str) -> Result<()> {
    let result = keyring::Entry::new(SERVICE, name).and_then(|entry| entry.delete_credential());
    match result {
        // Clear the file too: the secret may have been stored there before
        // the keyring became reachable.
        Ok(()) | Err(keyring::Error::NoEntry) => file_delete(&file_path(), name),
        Err(err) if store_unavailable(&err) => file_delete(&file_path(), name),
        Err(err) => Err(Error::Keyring(err.to_string())),
    }
}

/// True when the keyring itself cannot be used here (no secret service,
/// locked, ...) as opposed to an error about this particular entry.
fn store_unavailable(err: &keyring::Error) -> bool {
    matches!(
        err,
        keyring::Error::NoDefaultStore
            | keyring::Error::PlatformFailure(_)
            | keyring::Error::NoStorageAccess(_)
    )
}

fn file_path() -> PathBuf {
    paths::config_dir().join("secrets.toml")
}

fn file_get(path: &Path, name: &str) -> Result<Option<String>> {
    Ok(read_file(path)?.remove(name))
}

fn file_set(path: &Path, name: &str, value: &str) -> Result<()> {
    let mut secrets = read_file(path)?;
    secrets.insert(name.to_string(), value.to_string());
    write_file(path, &secrets)
}

fn file_delete(path: &Path, name: &str) -> Result<()> {
    let mut secrets = read_file(path)?;
    if secrets.remove(name).is_none() {
        return Ok(());
    }
    write_file(path, &secrets)
}

fn read_file(path: &Path) -> Result<BTreeMap<String, String>> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(err) => return Err(err.into()),
    };
    Ok(toml::from_str(&text)?)
}

fn write_file(path: &Path, secrets: &BTreeMap<String, String>) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(toml::to_string(secrets)?.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_store_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("secrets.toml");

        assert_eq!(file_get(&path, "moodle_token").unwrap(), None);

        file_set(&path, "moodle_token", "abc").unwrap();
        file_set(&path, "teams_refresh_token", "xyz").unwrap();
        assert_eq!(
            file_get(&path, "moodle_token").unwrap(),
            Some("abc".to_string())
        );
        assert_eq!(
            file_get(&path, "teams_refresh_token").unwrap(),
            Some("xyz".to_string())
        );

        file_delete(&path, "moodle_token").unwrap();
        assert_eq!(file_get(&path, "moodle_token").unwrap(), None);
        assert_eq!(
            file_get(&path, "teams_refresh_token").unwrap(),
            Some("xyz".to_string())
        );

        // Deleting what is not there is fine.
        file_delete(&path, "moodle_token").unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn file_store_is_private() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secrets.toml");
        file_set(&path, "moodle_token", "abc").unwrap();

        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
