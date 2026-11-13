use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use md5::{Digest, Md5};
use osu_formats::BeatmapMetadata;
use thiserror::Error;

use crate::maps::{MapCatalog, MapInfo, MapSource, MapStatus};
use crate::songs::hex;
use crate::{LocalBeatmap, ScannedReplay};

pub trait OsuFiles {
    type Error;

    fn download(&mut self, beatmap_id: u32) -> Result<Vec<u8>, Self::Error>;
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MissingReport {
    pub maps: usize,
    pub cached: usize,
    pub downloaded: usize,
    pub not_found: usize,
    pub mismatched: usize,
}

#[derive(Debug, Error)]
pub enum MissingError<E> {
    #[error(transparent)]
    Source(E),
    #[error("cannot write {}", .path.display())]
    Cache { path: PathBuf, source: io::Error },
}

pub fn fetch_missing<E, S: MapSource<Error = E> + OsuFiles<Error = E>>(
    replays: &mut [ScannedReplay],
    catalog: &mut MapCatalog,
    cache_dir: &Path,
    source: &mut S,
) -> Result<MissingReport, MissingError<E>> {
    let mut by_md5: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (index, replay) in replays.iter().enumerate() {
        if replay.beatmap.is_none() {
            by_md5
                .entry(replay.header.beatmap_md5.clone())
                .or_default()
                .push(index);
        }
    }
    let mut report = MissingReport::default();
    for (md5, indices) in by_md5 {
        let gone = catalog
            .get(&md5)
            .is_some_and(|info| info.status == MapStatus::Unknown && info.beatmap_id.is_none());
        if gone {
            continue;
        }
        report.maps += 1;
        let path = cache_dir.join(format!("{md5}.osu"));
        let bytes = match fs::read(&path) {
            Ok(bytes) => {
                report.cached += 1;
                bytes
            }
            Err(_) => {
                let found = source.lookup(&md5).map_err(MissingError::Source)?;
                let Some(beatmap_id) = found.as_ref().and_then(|map| map.info.beatmap_id) else {
                    catalog.insert(
                        md5,
                        MapInfo {
                            beatmap_id: None,
                            status: MapStatus::Unknown,
                            ranked_at: None,
                            outdated: false,
                        },
                    );
                    report.not_found += 1;
                    continue;
                };
                if let Some(found) = found {
                    catalog.insert(md5.clone(), found.info);
                }
                let bytes = source.download(beatmap_id).map_err(MissingError::Source)?;
                if hex(Md5::digest(&bytes).as_slice()) != md5 {
                    report.mismatched += 1;
                    continue;
                }
                fs::create_dir_all(cache_dir)
                    .and_then(|()| fs::write(&path, &bytes))
                    .map_err(|source| MissingError::Cache {
                        path: path.clone(),
                        source,
                    })?;
                report.downloaded += 1;
                bytes
            }
        };
        let metadata = BeatmapMetadata::parse(&bytes);
        for index in indices {
            replays[index].beatmap = Some(LocalBeatmap {
                path: path.clone(),
                metadata: metadata.clone(),
            });
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fs;

    use md5::{Digest, Md5};
    use osu_formats::{Mods, ReplayHeader};

    use super::*;
    use crate::ScannedReplay;
    use crate::maps::{FetchedMap, MapCatalog, MapInfo, MapSource, MapStatus};
    use crate::songs::hex;

    const MAP: &[u8] = b"osu file format v14\r\n\r\n[Metadata]\r\nTitle:T\r\nArtist:A\r\nCreator:c\r\nVersion:v\r\nBeatmapID:5\r\nBeatmapSetID:1\r\n";

    fn md5_of(bytes: &[u8]) -> String {
        hex(Md5::digest(bytes).as_slice())
    }

    #[derive(Default)]
    struct Fake {
        maps: HashMap<String, u32>,
        files: HashMap<u32, Vec<u8>>,
        calls: Vec<String>,
    }

    impl MapSource for Fake {
        type Error = String;

        fn batch(&mut self, _ids: &[u32]) -> Result<Vec<FetchedMap>, String> {
            Ok(Vec::new())
        }

        fn lookup(&mut self, md5: &str) -> Result<Option<FetchedMap>, String> {
            self.calls.push(format!("lookup {md5}"));
            Ok(self.maps.get(md5).map(|&id| FetchedMap {
                checksum: Some(md5.into()),
                info: MapInfo {
                    beatmap_id: Some(id),
                    status: MapStatus::Ranked,
                    ranked_at: None,
                    outdated: false,
                },
            }))
        }
    }

    impl OsuFiles for Fake {
        type Error = String;

        fn download(&mut self, beatmap_id: u32) -> Result<Vec<u8>, String> {
            self.calls.push(format!("download {beatmap_id}"));
            Ok(self.files.get(&beatmap_id).cloned().unwrap_or_default())
        }
    }

    fn replay(md5: &str) -> ScannedReplay {
        ScannedReplay {
            file_name: "r.osr".into(),
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
                score: 1_000_000,
                max_combo: 100,
                perfect: false,
                mods: Mods::from_bits(0),
                timestamp_ticks: 0,
            },
            beatmap: None,
            pp: None,
        }
    }

    #[test]
    fn downloads_the_map_of_a_replay_missing_from_songs() {
        let cache = tempfile::tempdir().unwrap();
        let md5 = md5_of(MAP);
        let mut fake = Fake::default();
        fake.maps.insert(md5.clone(), 5);
        fake.files.insert(5, MAP.to_vec());
        let mut replays = [replay(&md5)];
        let report = fetch_missing(
            &mut replays,
            &mut MapCatalog::default(),
            cache.path(),
            &mut fake,
        )
        .unwrap();
        let path = cache.path().join(format!("{md5}.osu"));
        let beatmap = replays[0].beatmap.as_ref().unwrap();
        assert_eq!(beatmap.path, path);
        assert_eq!(beatmap.metadata.title, "T");
        assert!(path.is_file());
        assert_eq!((report.maps, report.downloaded), (1, 1));
    }

    #[test]
    fn a_cached_file_needs_no_request() {
        let cache = tempfile::tempdir().unwrap();
        let md5 = md5_of(MAP);
        fs::write(cache.path().join(format!("{md5}.osu")), MAP).unwrap();
        let mut fake = Fake::default();
        let mut replays = [replay(&md5)];
        let report = fetch_missing(
            &mut replays,
            &mut MapCatalog::default(),
            cache.path(),
            &mut fake,
        )
        .unwrap();
        assert!(fake.calls.is_empty());
        assert_eq!(report.cached, 1);
        assert!(replays[0].beatmap.is_some());
    }

    #[test]
    fn a_map_gone_from_the_server_is_remembered() {
        let cache = tempfile::tempdir().unwrap();
        let mut fake = Fake::default();
        let mut catalog = MapCatalog::default();
        let report =
            fetch_missing(&mut [replay("gone")], &mut catalog, cache.path(), &mut fake).unwrap();
        assert_eq!(report.not_found, 1);
        let info = catalog.get("gone").unwrap();
        assert_eq!((info.beatmap_id, info.status), (None, MapStatus::Unknown));
        fetch_missing(&mut [replay("gone")], &mut catalog, cache.path(), &mut fake).unwrap();
        assert_eq!(fake.calls, ["lookup gone"]);
    }

    #[test]
    fn a_changed_map_is_not_attached() {
        let cache = tempfile::tempdir().unwrap();
        let md5 = md5_of(MAP);
        let mut fake = Fake::default();
        fake.maps.insert(md5.clone(), 5);
        fake.files.insert(5, b"osu file format v14\r\n".to_vec());
        let mut replays = [replay(&md5)];
        let report = fetch_missing(
            &mut replays,
            &mut MapCatalog::default(),
            cache.path(),
            &mut fake,
        )
        .unwrap();
        assert!(replays[0].beatmap.is_none());
        assert_eq!(report.mismatched, 1);
        assert!(!cache.path().join(format!("{md5}.osu")).exists());
    }
}
