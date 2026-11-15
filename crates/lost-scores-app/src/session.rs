use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use lost_scores_core::files::write_private_atomic;
use serde::{Deserialize, Serialize};

pub use crate::keys::delete;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub token: String,
    pub osu_id: u32,
    pub username: String,
}

pub fn load(path: &Path) -> Option<Session> {
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

pub fn save(path: &Path, session: &Session) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("cannot create {}", parent.display()))?;
    }
    write_private_atomic(path, &serde_json::to_vec_pretty(session)?)
        .with_context(|| format!("cannot write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::{Session, delete, load, save};

    fn session() -> Session {
        Session {
            token: "tok".into(),
            osu_id: 7,
            username: "Some Name".into(),
        }
    }

    #[test]
    fn a_session_file_round_trips_and_is_private() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("session.json");
        assert_eq!(load(&path), None);
        save(&path, &session()).unwrap();
        assert_eq!(load(&path), Some(session()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        assert!(delete(&path).unwrap());
        assert!(!delete(&path).unwrap());
        assert_eq!(load(&path), None);
    }
}
