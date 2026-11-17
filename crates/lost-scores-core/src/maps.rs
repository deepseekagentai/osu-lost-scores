use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::ScannedReplay;
use crate::files::write_atomic;
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

pub const RECHECK_AFTER: i64 = 7 * 24 * 60 * 60;

impl MapStatus {
    pub const fn gives_pp(self) -> bool {
        matches!(self, Self::Ranked | Self::Approved)
    }

    pub const fn can_change(self) -> bool {
        matches!(
            self,
            Self::Pending | Self::Qualified | Self::Wip | Self::Graveyard
        )
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub beatmapset_id: Option<u32>,
    pub status: MapStatus,
    pub ranked_at: Option<i64>,
    #[serde(default)]
    pub outdated: bool,
    #[serde(default)]
    pub checked_at: Option<i64>,
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

#[derive(Clone, Debug, Default, PartialEq)]
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
        write_atomic(path, &data).map_err(io)
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

    fn batch_size(&self) -> usize {
        BATCH_SIZE
    }

    fn lookup_size(&self) -> usize {
        1
    }

    fn lookups(&mut self, md5s: &[String]) -> Result<Vec<Option<FetchedMap>>, Self::Error> {
        md5s.iter().map(|md5| self.lookup(md5)).collect()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FillReport {
    pub by_id: usize,
    pub by_md5: usize,
    pub outdated: usize,
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

pub const BATCH_SIZE: usize = 50;

pub fn fill<S: MapSource + ?Sized>(
    catalog: &mut MapCatalog,
    wanted: &[Wanted],
    source: &mut S,
    now: i64,
    mut progress: impl FnMut(usize, usize),
) -> Result<FillReport, S::Error> {
    let mut report = FillReport::default();
    let mut ids: Vec<u32> = wanted.iter().filter_map(|item| item.beatmap_id).collect();
    ids.sort_unstable();
    ids.dedup();
    let size = source.batch_size();
    let group = source.lookup_size().max(1);
    let total = wanted.len();
    let mut per_id: HashMap<u32, usize> = HashMap::new();
    for id in wanted.iter().filter_map(|item| item.beatmap_id) {
        *per_id.entry(id).or_default() += 1;
    }
    let mut done = 0;
    progress(done, total);
    let mut fetched = Vec::new();
    for chunk in ids.chunks(size) {
        fetched.extend(source.batch(chunk)?);
        done += chunk
            .iter()
            .map(|id| per_id.get(id).copied().unwrap_or(0))
            .sum::<usize>();
        progress(done, total);
    }
    let by_checksum: HashMap<&str, &MapInfo> = fetched
        .iter()
        .filter_map(|map| Some((map.checksum.as_deref()?, &map.info)))
        .collect();
    let known_ids: HashMap<u32, &MapInfo> = fetched
        .iter()
        .filter_map(|map| Some((map.info.beatmap_id?, &map.info)))
        .collect();
    let mut unmatched = Vec::new();
    for item in wanted {
        match by_checksum.get(item.md5.as_str()) {
            Some(info) => {
                catalog.insert(
                    item.md5.clone(),
                    MapInfo {
                        checked_at: Some(now),
                        ..(*info).clone()
                    },
                );
                report.by_id += 1;
            }
            None => unmatched.push(item),
        }
    }
    for chunk in unmatched.chunks(group) {
        let md5s: Vec<String> = chunk.iter().map(|item| item.md5.clone()).collect();
        let answers = source.lookups(&md5s)?;
        done += chunk
            .iter()
            .filter(|item| item.beatmap_id.is_none())
            .count();
        progress(done, total);
        for (item, found) in chunk.iter().zip(answers) {
            let known = item.beatmap_id.and_then(|id| known_ids.get(&id));
            let info = match (found, known) {
                (Some(found), _) => {
                    report.by_md5 += 1;
                    found.info
                }
                (None, Some(info)) => {
                    report.outdated += 1;
                    MapInfo {
                        outdated: true,
                        ..(*info).clone()
                    }
                }
                (None, None) => {
                    report.not_found += 1;
                    MapInfo {
                        beatmap_id: None,
                        beatmapset_id: None,
                        status: MapStatus::Unknown,
                        ranked_at: None,
                        outdated: false,
                        checked_at: None,
                    }
                }
            };
            catalog.insert(
                item.md5.clone(),
                MapInfo {
                    checked_at: Some(now),
                    ..info
                },
            );
        }
    }
    Ok(report)
}

pub fn stale(catalog: &MapCatalog, now: i64) -> Vec<u32> {
    let mut ids: Vec<u32> = catalog
        .maps
        .values()
        .filter(|info| info.status.can_change())
        .filter(|info| info.checked_at.is_none_or(|at| now - at > RECHECK_AFTER))
        .filter_map(|info| info.beatmap_id)
        .collect();
    ids.sort_unstable();
    ids.dedup();
    ids
}

pub fn refresh<S: MapSource + ?Sized>(
    catalog: &mut MapCatalog,
    ids: &[u32],
    source: &mut S,
    now: i64,
    mut progress: impl FnMut(usize, usize),
) -> Result<usize, S::Error> {
    let size = source.batch_size();
    let mut done = 0;
    progress(done, ids.len());
    let mut fetched = Vec::new();
    for chunk in ids.chunks(size) {
        fetched.extend(source.batch(chunk)?);
        done += chunk.len();
        progress(done, ids.len());
    }
    let by_id: HashMap<u32, &MapInfo> = fetched
        .iter()
        .filter_map(|map| Some((map.info.beatmap_id?, &map.info)))
        .collect();
    let mut changed = 0;
    for info in catalog.maps.values_mut() {
        let Some(id) = info.beatmap_id.filter(|id| ids.contains(id)) else {
            continue;
        };
        if let Some(found) = by_id.get(&id) {
            if info.status != found.status {
                changed += 1;
            }
            info.status = found.status;
            info.ranked_at = found.ranked_at;
            info.beatmapset_id = found.beatmapset_id.or(info.beatmapset_id);
        }
        info.checked_at = Some(now);
    }
    Ok(changed)
}

pub fn fill_set_ids<S: MapSource + ?Sized>(
    catalog: &mut MapCatalog,
    ids: &[u32],
    source: &mut S,
) -> Result<HashMap<u32, u32>, S::Error> {
    let mut sets = HashMap::new();
    for chunk in ids.chunks(source.batch_size()) {
        for map in source.batch(chunk)? {
            if let (Some(id), Some(set)) = (map.info.beatmap_id, map.info.beatmapset_id) {
                sets.insert(id, set);
            }
        }
    }
    for info in catalog.maps.values_mut() {
        if info.beatmapset_id.is_none()
            && let Some(set) = info.beatmap_id.and_then(|id| sets.get(&id))
        {
            info.beatmapset_id = Some(*set);
        }
    }
    Ok(sets)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use std::collections::HashMap;
    use std::path::PathBuf;

    use osu_formats::{BeatmapMetadata, Mods, ReplayHeader};

    use super::{
        BATCH_SIZE, CatalogError, FetchedMap, FillReport, MapCatalog, MapInfo, MapSource,
        MapStatus, RECHECK_AFTER, Wanted, fill, fill_set_ids, refresh, stale, wanted,
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
                beatmapset_id: None,
                status: MapStatus::Ranked,
                ranked_at: Some(1557000000),
                outdated: false,
                checked_at: None,
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
            beatmapset_id: None,
            status,
            ranked_at: Some(1_400_000_000),
            outdated: false,
            checked_at: None,
        }
    }

    #[derive(Default)]
    struct FakeSource {
        batch: Vec<FetchedMap>,
        lookups: HashMap<String, FetchedMap>,
        batch_calls: Vec<Vec<u32>>,
        lookup_calls: Vec<String>,
        size: Option<usize>,
        group: Option<usize>,
        groups: Vec<Vec<String>>,
    }

    impl MapSource for FakeSource {
        type Error = ();

        fn batch_size(&self) -> usize {
            self.size.unwrap_or(BATCH_SIZE)
        }

        fn lookup_size(&self) -> usize {
            self.group.unwrap_or(1)
        }

        fn lookups(&mut self, md5s: &[String]) -> Result<Vec<Option<FetchedMap>>, ()> {
            self.groups.push(md5s.to_vec());
            md5s.iter().map(|md5| self.lookup(md5)).collect()
        }

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
        let report = fill(&mut catalog, &wanted, &mut source, NOW, |_, _| {}).unwrap();
        assert_eq!(
            report,
            FillReport {
                by_id: 1,
                by_md5: 1,
                outdated: 0,
                not_found: 1
            }
        );
        assert_eq!(source.batch_calls, [vec![11, 22]]);
        assert_eq!(source.lookup_calls, ["b", "c"]);
        assert_eq!(
            catalog.get("a"),
            Some(&MapInfo {
                checked_at: Some(NOW),
                ..info(11, MapStatus::Ranked)
            })
        );
        assert_eq!(
            catalog.get("b"),
            Some(&MapInfo {
                checked_at: Some(NOW),
                ..info(222, MapStatus::Loved)
            })
        );
        assert_eq!(catalog.get("c").unwrap().status, MapStatus::Unknown);
    }

    #[test]
    fn old_version_of_a_known_map_keeps_the_map_status() {
        let mut source = FakeSource {
            batch: vec![FetchedMap {
                checksum: Some("current version".into()),
                info: info(33, MapStatus::Ranked),
            }],
            ..FakeSource::default()
        };
        let wanted = [Wanted {
            md5: "old version".into(),
            beatmap_id: Some(33),
        }];
        let mut catalog = MapCatalog::default();
        let report = fill(&mut catalog, &wanted, &mut source, NOW, |_, _| {}).unwrap();
        assert_eq!(report.outdated, 1);
        assert_eq!(source.lookup_calls, ["old version"]);
        assert_eq!(
            catalog.get("old version"),
            Some(&MapInfo {
                outdated: true,
                checked_at: Some(NOW),
                ..info(33, MapStatus::Ranked)
            })
        );
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

    #[test]
    fn set_numbers_are_filled_in_only_where_they_are_missing() {
        let mut catalog = MapCatalog::default();
        catalog.insert("old".into(), info(11, MapStatus::Ranked));
        catalog.insert(
            "known".into(),
            MapInfo {
                beatmapset_id: Some(7),
                ..info(12, MapStatus::Ranked)
            },
        );
        let mut source = FakeSource {
            batch: vec![
                FetchedMap {
                    checksum: Some("old".into()),
                    info: MapInfo {
                        beatmapset_id: Some(5),
                        ..info(11, MapStatus::Loved)
                    },
                },
                FetchedMap {
                    checksum: Some("known".into()),
                    info: MapInfo {
                        beatmapset_id: Some(9),
                        ..info(12, MapStatus::Ranked)
                    },
                },
            ],
            size: Some(1),
            ..FakeSource::default()
        };
        let sets = fill_set_ids(&mut catalog, &[11, 12], &mut source).unwrap();
        assert_eq!(source.batch_calls, [vec![11], vec![12]]);
        assert_eq!(sets.get(&11), Some(&5));
        assert_eq!(catalog.get("old").unwrap().beatmapset_id, Some(5));
        assert_eq!(catalog.get("old").unwrap().status, MapStatus::Ranked);
        assert_eq!(catalog.get("known").unwrap().beatmapset_id, Some(7));
    }

    #[test]
    fn an_old_catalog_without_set_numbers_still_loads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("maps.json");
        fs::write(
            &path,
            r#"{"version":1,"maps":{"a":{"beatmap_id":11,"status":"ranked","ranked_at":1400000000}}}"#,
        )
        .unwrap();
        let loaded = MapCatalog::load(&path).unwrap();
        assert_eq!(loaded.get("a").unwrap().beatmapset_id, None);
        let mut known = loaded.clone();
        known.insert(
            "b".into(),
            MapInfo {
                beatmapset_id: Some(3),
                ..info(12, MapStatus::Ranked)
            },
        );
        known.save(&path).unwrap();
        assert_eq!(
            MapCatalog::load(&path)
                .unwrap()
                .get("b")
                .unwrap()
                .beatmapset_id,
            Some(3)
        );
    }

    const NOW: i64 = 1_800_000_000;

    fn entry(id: u32, status: MapStatus, checked_at: Option<i64>) -> MapInfo {
        MapInfo {
            beatmap_id: Some(id),
            beatmapset_id: None,
            status,
            ranked_at: None,
            outdated: false,
            checked_at,
        }
    }

    #[test]
    fn final_and_unknown_statuses_are_never_stale() {
        let mut catalog = MapCatalog::default();
        let statuses = [
            MapStatus::Ranked,
            MapStatus::Approved,
            MapStatus::Loved,
            MapStatus::Unknown,
        ];
        for (id, status) in (1..).zip(statuses) {
            catalog.insert(format!("m{id}"), entry(id, status, None));
        }
        assert!(stale(&catalog, NOW).is_empty());
    }

    #[test]
    fn a_catalog_without_checked_at_counts_as_stale() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("maps.json");
        fs::write(
            &path,
            r#"{"version":1,"maps":{"m":{"beatmap_id":5,"status":"graveyard","ranked_at":null}}}"#,
        )
        .unwrap();
        assert_eq!(stale(&MapCatalog::load(&path).unwrap(), NOW), [5]);
    }

    #[test]
    fn a_week_old_check_is_stale_and_a_day_old_one_is_not() {
        let mut catalog = MapCatalog::default();
        catalog.insert(
            "old".into(),
            entry(1, MapStatus::Graveyard, Some(NOW - RECHECK_AFTER - 1)),
        );
        catalog.insert(
            "new".into(),
            entry(2, MapStatus::Graveyard, Some(NOW - 86_400)),
        );
        assert_eq!(stale(&catalog, NOW), [1]);
    }

    #[test]
    fn refresh_updates_status_and_ranked_date() {
        let mut catalog = MapCatalog::default();
        catalog.insert("m".into(), entry(5, MapStatus::Graveyard, None));
        let mut source = FakeSource {
            batch: vec![FetchedMap {
                checksum: Some("m".into()),
                info: MapInfo {
                    ranked_at: Some(1_700_000_000),
                    ..entry(5, MapStatus::Ranked, None)
                },
            }],
            ..FakeSource::default()
        };
        assert_eq!(
            refresh(&mut catalog, &[5], &mut source, NOW, |_, _| {}).unwrap(),
            1
        );
        let info = catalog.get("m").unwrap();
        assert_eq!(
            (info.status, info.ranked_at, info.checked_at),
            (MapStatus::Ranked, Some(1_700_000_000), Some(NOW))
        );
    }

    #[test]
    fn fill_records_when_it_checked() {
        let mut source = FakeSource {
            batch: vec![FetchedMap {
                checksum: Some("a".into()),
                info: info(11, MapStatus::Ranked),
            }],
            ..FakeSource::default()
        };
        let mut catalog = MapCatalog::default();
        let wanted = [Wanted {
            md5: "a".into(),
            beatmap_id: Some(11),
        }];
        fill(&mut catalog, &wanted, &mut source, NOW, |_, _| {}).unwrap();
        assert_eq!(catalog.get("a").unwrap().checked_at, Some(NOW));
    }

    fn wanted_ids(count: u32) -> Vec<Wanted> {
        (1..=count)
            .map(|id| Wanted {
                md5: format!("m{id}"),
                beatmap_id: Some(id),
            })
            .collect()
    }

    #[test]
    fn fill_asks_for_at_most_50_maps_per_request() {
        let mut source = FakeSource::default();
        let mut catalog = MapCatalog::default();
        fill(&mut catalog, &wanted_ids(51), &mut source, NOW, |_, _| {}).unwrap();
        let sizes: Vec<usize> = source.batch_calls.iter().map(Vec::len).collect();
        assert_eq!(sizes, [50, 1]);
    }

    #[test]
    fn fill_and_refresh_ask_as_many_maps_as_the_source_takes() {
        let mut source = FakeSource {
            size: Some(3),
            ..FakeSource::default()
        };
        fill(
            &mut MapCatalog::default(),
            &wanted_ids(7),
            &mut source,
            NOW,
            |_, _| {},
        )
        .unwrap();
        let ids: Vec<u32> = (1..=7).collect();
        refresh(
            &mut MapCatalog::default(),
            &ids,
            &mut source,
            NOW,
            |_, _| {},
        )
        .unwrap();
        let sizes: Vec<usize> = source.batch_calls.iter().map(Vec::len).collect();
        assert_eq!(sizes, [3, 3, 1, 3, 3, 1]);
    }

    #[test]
    fn fill_asks_md5s_in_groups_of_the_source_lookup_size() {
        let found = FetchedMap {
            checksum: Some("b".into()),
            info: info(22, MapStatus::Ranked),
        };
        let mut source = FakeSource {
            lookups: HashMap::from([("b".to_owned(), found)]),
            group: Some(2),
            ..FakeSource::default()
        };
        let wanted: Vec<Wanted> = ["a", "b", "c"]
            .into_iter()
            .map(|md5| Wanted {
                md5: md5.into(),
                beatmap_id: None,
            })
            .collect();
        let mut catalog = MapCatalog::default();
        let mut steps = Vec::new();
        let report = fill(&mut catalog, &wanted, &mut source, NOW, |done, total| {
            steps.push((done, total))
        })
        .unwrap();
        assert_eq!(source.groups, [vec!["a", "b"], vec!["c"]]);
        assert_eq!(steps, [(0, 3), (2, 3), (3, 3)]);
        assert_eq!((report.by_md5, report.not_found), (1, 2));
        assert_eq!(catalog.get("b").unwrap().beatmap_id, Some(22));
    }

    #[test]
    fn fill_reports_progress_in_maps_after_every_request() {
        let mut source = FakeSource {
            batch: vec![FetchedMap {
                checksum: Some("a".into()),
                info: info(11, MapStatus::Ranked),
            }],
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
        let mut steps = Vec::new();
        fill(
            &mut MapCatalog::default(),
            &wanted,
            &mut source,
            NOW,
            |done, total| steps.push((done, total)),
        )
        .unwrap();
        assert_eq!(steps, [(0, 3), (2, 3), (2, 3), (3, 3)]);
    }

    #[test]
    fn refresh_asks_for_at_most_50_maps_per_request_and_reports_each() {
        let mut catalog = MapCatalog::default();
        for id in 1..=51 {
            catalog.insert(format!("m{id}"), entry(id, MapStatus::Graveyard, None));
        }
        let ids: Vec<u32> = (1..=51).collect();
        let mut source = FakeSource::default();
        let mut steps = Vec::new();
        refresh(&mut catalog, &ids, &mut source, NOW, |done, total| {
            steps.push((done, total))
        })
        .unwrap();
        let sizes: Vec<usize> = source.batch_calls.iter().map(Vec::len).collect();
        assert_eq!(sizes, [50, 1]);
        assert_eq!(steps, [(0, 51), (50, 51), (51, 51)]);
    }
}
