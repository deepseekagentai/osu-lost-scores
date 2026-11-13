use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MapStatus {
    Ranked,
    Approved,
    Qualified,
    Loved,
    Pending,
    Wip,
    Graveyard,
    #[serde(other)]
    Unknown,
}

impl MapStatus {
    pub const fn gives_pp(self) -> bool {
        matches!(self, Self::Ranked | Self::Approved)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct MapInfo {
    pub beatmap_id: Option<u32>,
    pub status: MapStatus,
    pub ranked_at: Option<i64>,
}

#[derive(Deserialize)]
struct CatalogFile {
    maps: HashMap<String, MapInfo>,
}

#[derive(Debug, Default)]
pub struct MapCatalog {
    maps: HashMap<String, MapInfo>,
}

#[derive(Debug, Error)]
pub enum CatalogError {
    #[error("cannot read map catalog {}", .path.display())]
    Io { path: PathBuf, source: io::Error },
    #[error("map catalog {} is not valid", .path.display())]
    Format {
        path: PathBuf,
        source: serde_json::Error,
    },
}

impl MapCatalog {
    pub fn load(path: &Path) -> Result<Self, CatalogError> {
        let data = fs::read(path).map_err(|source| CatalogError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let file: CatalogFile =
            serde_json::from_slice(&data).map_err(|source| CatalogError::Format {
                path: path.to_path_buf(),
                source,
            })?;
        Ok(Self { maps: file.maps })
    }

    pub fn get(&self, md5: &str) -> Option<&MapInfo> {
        self.maps.get(md5)
    }

    pub fn len(&self) -> usize {
        self.maps.len()
    }

    pub fn is_empty(&self) -> bool {
        self.maps.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{CatalogError, MapCatalog, MapInfo, MapStatus};

    const CATALOG: &str = r#"{
        "version": 1,
        "maps": {
            "aaaa": {"beatmap_id": 11, "status": "ranked", "ranked_at": 1557000000},
            "bbbb": {"beatmap_id": 22, "status": "loved", "ranked_at": null},
            "cccc": {"beatmap_id": null, "status": "not found", "ranked_at": null}
        }
    }"#;

    #[test]
    fn loads_maps_by_md5() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("maps.json");
        fs::write(&path, CATALOG).unwrap();
        let catalog = MapCatalog::load(&path).unwrap();
        assert_eq!(catalog.len(), 3);
        assert_eq!(
            catalog.get("aaaa"),
            Some(&MapInfo {
                beatmap_id: Some(11),
                status: MapStatus::Ranked,
                ranked_at: Some(1557000000)
            })
        );
        assert_eq!(catalog.get("cccc").unwrap().status, MapStatus::Unknown);
        assert_eq!(catalog.get("dddd"), None);
    }

    #[test]
    fn only_ranked_and_approved_maps_give_pp() {
        let giving: Vec<_> = [
            MapStatus::Ranked,
            MapStatus::Approved,
            MapStatus::Qualified,
            MapStatus::Loved,
            MapStatus::Pending,
            MapStatus::Wip,
            MapStatus::Graveyard,
            MapStatus::Unknown,
        ]
        .into_iter()
        .filter(|status| status.gives_pp())
        .collect();
        assert_eq!(giving, [MapStatus::Ranked, MapStatus::Approved]);
    }

    #[test]
    fn unreadable_or_invalid_catalog_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing.json");
        assert!(
            matches!(MapCatalog::load(&missing), Err(CatalogError::Io { path, .. }) if path == missing)
        );
        let broken = dir.path().join("broken.json");
        fs::write(&broken, "{").unwrap();
        assert!(matches!(
            MapCatalog::load(&broken),
            Err(CatalogError::Format { .. })
        ));
    }
}
