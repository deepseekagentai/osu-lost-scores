use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::Path;

use anyhow::{Context, Result};
use osu_api::Credentials;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct StoredKeys {
    client_id: String,
    client_secret: String,
}

pub fn from_env() -> Option<Credentials> {
    let var = |name| env::var(name).ok().filter(|value| !value.is_empty());
    Some(Credentials {
        client_id: var("LOST_SCORES_CLIENT_ID")?,
        client_secret: var("LOST_SCORES_CLIENT_SECRET")?,
    })
}

pub fn load(path: &Path) -> Option<Credentials> {
    let stored: StoredKeys = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    Some(Credentials {
        client_id: stored.client_id,
        client_secret: stored.client_secret,
    })
}

pub fn save(path: &Path, credentials: &Credentials) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("cannot create {}", parent.display()))?;
    }
    let data = serde_json::to_vec_pretty(&StoredKeys {
        client_id: credentials.client_id.clone(),
        client_secret: credentials.client_secret.clone(),
    })?;
    write_private(path, &data).with_context(|| format!("cannot write {}", path.display()))
}

#[cfg(unix)]
fn write_private(path: &Path, data: &[u8]) -> io::Result<()> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    file.write_all(data)
}

#[cfg(not(unix))]
fn write_private(path: &Path, data: &[u8]) -> io::Result<()> {
    fs::File::create(path)?.write_all(data)
}

pub fn delete(path: &Path) -> Result<bool> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).with_context(|| format!("cannot delete {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use osu_api::Credentials;

    use super::{delete, load, save};

    fn keys() -> Credentials {
        Credentials {
            client_id: "123".into(),
            client_secret: "secret".into(),
        }
    }

    #[test]
    fn saved_keys_load_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("credentials.json");
        assert_eq!(load(&path), None);
        save(&path, &keys()).unwrap();
        assert_eq!(load(&path), Some(keys()));
    }

    #[cfg(unix)]
    #[test]
    fn keys_file_is_readable_only_by_its_owner() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.json");
        std::fs::write(&path, "{}").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        save(&path, &keys()).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn delete_reports_whether_keys_existed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.json");
        save(&path, &keys()).unwrap();
        assert!(delete(&path).unwrap());
        assert!(!delete(&path).unwrap());
        assert_eq!(load(&path), None);
    }
}
