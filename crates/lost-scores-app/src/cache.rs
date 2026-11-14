use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::setup;

const AVATARS: &str = "avatars";
const COVERS: &str = "covers";

pub fn avatars_dir() -> Option<PathBuf> {
    setup::cache_root().map(|root| root.join(AVATARS))
}

pub fn covers_dir() -> Option<PathBuf> {
    setup::cache_root().map(|root| root.join(COVERS))
}

pub fn clear(cache_root: &Path) -> Result<usize> {
    let mut removed = 0;
    for name in [AVATARS, COVERS] {
        let dir = cache_root.join(name);
        removed += count_files(&dir).with_context(|| format!("cannot read {}", dir.display()))?;
        match fs::remove_dir_all(&dir) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| format!("cannot remove {}", dir.display()));
            }
        }
    }
    Ok(removed)
}

fn count_files(dir: &Path) -> io::Result<usize> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    };
    let mut files = 0;
    for entry in entries {
        let entry = entry?;
        files += if entry.file_type()?.is_dir() {
            count_files(&entry.path())?
        } else {
            1
        };
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::clear;

    #[test]
    fn clear_keeps_the_catalog_history_and_osu_files() {
        let root = tempfile::tempdir().unwrap();
        let path = |name: &str| root.path().join(name);
        for file in [
            "avatars/1_40.png",
            "covers/2.jpg",
            "covers/sub/3.jpg",
            "osu/4.osu",
            "scans/2026-01-01_10-00-00/scan.json",
            "maps.json",
            "config.json",
            "credentials.json",
        ] {
            fs::create_dir_all(path(file).parent().unwrap()).unwrap();
            fs::write(path(file), b"x").unwrap();
        }
        assert_eq!(clear(root.path()).unwrap(), 3);
        assert!(!path("avatars").exists() && !path("covers").exists());
        for kept in [
            "osu/4.osu",
            "scans/2026-01-01_10-00-00/scan.json",
            "maps.json",
            "config.json",
            "credentials.json",
        ] {
            assert!(path(kept).is_file(), "{kept}");
        }
        assert_eq!(clear(root.path()).unwrap(), 0);
    }
}
