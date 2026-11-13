use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::ScannedReplay;
use crate::pp::reference_mod_bits;

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

    pub fn from_name(name: &str) -> Self {
        match name {
            "ranked" => Self::Ranked,
            "approved" => Self::Approved,
            "qualified" => Self::Qualified,
            "loved" => Self::Loved,
            "pending" => Self::Pending,
            "wip" => Self::Wip,
            "graveyard" => Self::Graveyard,
            _ => Self::Unknown,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct MapInfo {
    pub beatmap_id: Option<u32>,
    pub status: MapStatus,
    pub ranked_at: Option<i64>,
}

#[derive(Deserialize)]
struct CatalogFile {
    maps: HashMap<String, MapInfo>,
}

#[derive(Serialize)]
struct SavedCatalog<'a> {
    version: u32,
    maps: &'a HashMap<String, MapInfo>,
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

    pub fn save(&self, path: &Path) -> Result<(), CatalogError> {
        let io = |source| CatalogError::Io {
            path: path.to_path_buf(),
            source,
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(io)?;
        }
        let file = SavedCatalog {
            version: 1,
            maps: &self.maps,
        };
        let data = serde_json::to_vec(&file).map_err(|source| CatalogError::Format {
            path: path.to_path_buf(),
            source,
        })?;
        fs::write(path, data).map_err(io)
    }

    pub fn insert(&mut self, md5: String, info: MapInfo) {
        self.maps.insert(md5, info);
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wanted {
    pub md5: String,
    pub beatmap_id: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FetchedMap {
    pub checksum: Option<String>,
    pub info: MapInfo,
}

pub trait MapSource {
    type Error;

    fn batch(&mut self, ids: &[u32]) -> Result<Vec<FetchedMap>, Self::Error>;
    fn lookup(&mut self, md5: &str) -> Result<Option<FetchedMap>, Self::Error>;
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FillReport {
    pub by_id: usize,
    pub by_md5: usize,
    pub not_found: usize,
}

pub fn wanted(replays: &[ScannedReplay], catalog: &MapCatalog) -> Vec<Wanted> {
    let mut groups: HashMap<(&str, u32), usize> = HashMap::new();
    let mut hints: HashMap<&str, Option<u32>> = HashMap::new();
    for replay in replays.iter().filter(|replay| replay.pp.is_some()) {
        let md5 = replay.header.beatmap_md5.as_str();
        *groups
            .entry((md5, reference_mod_bits(replay.header.mods)))
            .or_default() += 1;
        let hint = replay
            .beatmap
            .as_ref()
            .and_then(|beatmap| beatmap.metadata.beatmap_id);
        let slot = hints.entry(md5).or_default();
        if slot.is_none() {
            *slot = hint;
        }
    }
    let maps: HashSet<&str> = groups
        .iter()
        .filter(|(_, plays)| **plays >= 2)
        .map(|((md5, _), _)| *md5)
        .filter(|md5| catalog.get(md5).is_none())
        .collect();
    let mut wanted: Vec<Wanted> = maps
        .into_iter()
        .map(|md5| Wanted {
            md5: md5.to_owned(),
            beatmap_id: hints.get(md5).copied().flatten(),
        })
        .collect();
    wanted.sort_by(|a, b| a.md5.cmp(&b.md5));
    wanted
}

pub fn fill<S: MapSource>(
    catalog: &mut MapCatalog,
    wanted: &[Wanted],
    source: &mut S,
) -> Result<FillReport, S::Error> {
    let mut report = FillReport::default();
    let mut ids: Vec<u32> = wanted.iter().filter_map(|item| item.beatmap_id).collect();
    ids.sort_unstable();
    ids.dedup();
    let by_checksum: HashMap<String, MapInfo> = if ids.is_empty() {
        HashMap::new()
    } else {
        source
            .batch(&ids)?
            .into_iter()
            .filter_map(|fetched| Some((fetched.checksum?, fetched.info)))
            .collect()
    };
    for item in wanted {
        if let Some(info) = by_checksum.get(&item.md5) {
            catalog.insert(item.md5.clone(), info.clone());
            report.by_id += 1;
            continue;
        }
        match source.lookup(&item.md5)? {
            Some(found) => {
                catalog.insert(item.md5.clone(), found.info);
                report.by_md5 += 1;
            }
            None => {
                catalog.insert(
                    item.md5.clone(),
                    MapInfo {
                        beatmap_id: None,
                        status: MapStatus::Unknown,
                        ranked_at: None,
                    },
                );
                report.not_found += 1;
            }
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use std::collections::HashMap;
    use std::path::PathBuf;

    use osu_formats::{BeatmapMetadata, Mods, ReplayHeader};

    use super::{
        CatalogError, FetchedMap, FillReport, MapCatalog, MapInfo, MapSource, MapStatus, Wanted,
        fill, wanted,
    };
    use crate::{LocalBeatmap, ScannedReplay};

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

    fn replay(md5: &str, mods: u32, pp: Option<f64>, beatmap_id: Option<u32>) -> ScannedReplay {
        ScannedReplay {
            file_name: format!("{md5}-{mods}.osr"),
            header: ReplayHeader {
                mode: 0,
                version: 20190906,
                beatmap_md5: md5.into(),
                player_name: "Lemon".into(),
                replay_md5: String::new(),
                count_300: 100,
                count_100: 0,
                count_50: 0,
                count_geki: 0,
                count_katu: 0,
                count_miss: 0,
                score: 1,
                max_combo: 100,
                perfect: false,
                mods: Mods::from_bits(mods),
                timestamp_ticks: 0,
            },
            beatmap: Some(LocalBeatmap {
                path: PathBuf::from(md5),
                metadata: BeatmapMetadata {
                    beatmap_id,
                    ..BeatmapMetadata::default()
                },
            }),
            pp,
        }
    }

    fn info(id: u32, status: MapStatus) -> MapInfo {
        MapInfo {
            beatmap_id: Some(id),
            status,
            ranked_at: Some(1_400_000_000),
        }
    }

    #[derive(Default)]
    struct FakeSource {
        batch: Vec<FetchedMap>,
        lookups: HashMap<String, FetchedMap>,
        batch_calls: Vec<Vec<u32>>,
        lookup_calls: Vec<String>,
    }

    impl MapSource for FakeSource {
        type Error = ();

        fn batch(&mut self, ids: &[u32]) -> Result<Vec<FetchedMap>, ()> {
            self.batch_calls.push(ids.to_vec());
            Ok(self.batch.clone())
        }

        fn lookup(&mut self, md5: &str) -> Result<Option<FetchedMap>, ()> {
            self.lookup_calls.push(md5.into());
            Ok(self.lookups.get(md5).cloned())
        }
    }

    #[test]
    fn status_names_follow_the_api() {
        assert_eq!(MapStatus::from_name("ranked"), MapStatus::Ranked);
        assert_eq!(MapStatus::from_name("loved"), MapStatus::Loved);
        assert_eq!(MapStatus::from_name("graveyard"), MapStatus::Graveyard);
        assert_eq!(MapStatus::from_name("something new"), MapStatus::Unknown);
    }

    #[test]
    fn wanted_lists_unknown_maps_that_can_have_lost_plays() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("maps.json");
        std::fs::write(
            &path,
            r#"{"version":1,"maps":{"c":{"beatmap_id":3,"status":"ranked","ranked_at":null}}}"#,
        )
        .unwrap();
        let catalog = MapCatalog::load(&path).unwrap();
        let replays = [
            replay("a", 0, Some(100.0), Some(11)),
            replay("a", 0, Some(90.0), Some(11)),
            replay("b", 0, Some(100.0), Some(22)),
            replay("b", 8, Some(90.0), Some(22)),
            replay("c", 0, Some(100.0), Some(3)),
            replay("c", 0, Some(90.0), Some(3)),
            replay("d", 0, Some(100.0), None),
            replay("d", 0, None, None),
            replay("e", 4, Some(100.0), None),
            replay("e", 0, Some(90.0), None),
        ];
        assert_eq!(
            wanted(&replays, &catalog),
            [
                Wanted {
                    md5: "a".into(),
                    beatmap_id: Some(11)
                },
                Wanted {
                    md5: "e".into(),
                    beatmap_id: None
                },
            ]
        );
    }

    #[test]
    fn fill_matches_batches_by_checksum_and_falls_back_to_lookup() {
        let mut source = FakeSource {
            batch: vec![
                FetchedMap {
                    checksum: Some("a".into()),
                    info: info(11, MapStatus::Ranked),
                },
                FetchedMap {
                    checksum: Some("newer version".into()),
                    info: info(22, MapStatus::Ranked),
                },
            ],
            lookups: HashMap::from([(
                "b".to_string(),
                FetchedMap {
                    checksum: Some("b".into()),
                    info: info(222, MapStatus::Loved),
                },
            )]),
            ..FakeSource::default()
        };
        let wanted = [
            Wanted {
                md5: "a".into(),
                beatmap_id: Some(11),
            },
            Wanted {
                md5: "b".into(),
                beatmap_id: Some(22),
            },
            Wanted {
                md5: "c".into(),
                beatmap_id: None,
            },
        ];
        let mut catalog = MapCatalog::default();
        let report = fill(&mut catalog, &wanted, &mut source).unwrap();
        assert_eq!(
            report,
            FillReport {
                by_id: 1,
                by_md5: 1,
                not_found: 1
            }
        );
        assert_eq!(source.batch_calls, [vec![11, 22]]);
        assert_eq!(source.lookup_calls, ["b", "c"]);
        assert_eq!(catalog.get("a"), Some(&info(11, MapStatus::Ranked)));
        assert_eq!(catalog.get("b"), Some(&info(222, MapStatus::Loved)));
        assert_eq!(catalog.get("c").unwrap().status, MapStatus::Unknown);
    }

    #[test]
    fn saved_catalog_loads_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("maps.json");
        let mut catalog = MapCatalog::default();
        catalog.insert("a".into(), info(11, MapStatus::Ranked));
        catalog.save(&path).unwrap();
        let loaded = MapCatalog::load(&path).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded.get("a"), Some(&info(11, MapStatus::Ranked)));
    }
}
