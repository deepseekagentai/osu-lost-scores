use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use md5::{Digest, Md5};
use osu_formats::BeatmapMetadata;
use thiserror::Error;

use crate::files::write_atomic;
use crate::lost;
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

pub fn missing_maps(replays: &[ScannedReplay], catalog: &MapCatalog) -> Vec<String> {
    let without_file = replays.iter().filter(|replay| replay.beatmap.is_none());
    lost::candidate_maps(without_file)
        .into_iter()
        .filter(|md5| {
            !catalog
                .get(md5)
                .is_some_and(|info| info.status == MapStatus::Unknown && info.beatmap_id.is_none())
        })
        .map(str::to_owned)
        .collect()
}

pub fn fetch_missing<E, S: MapSource<Error = E> + OsuFiles<Error = E>>(
    md5s: &[String],
    replays: &mut [ScannedReplay],
    catalog: &mut MapCatalog,
    cache_dir: &Path,
    source: &mut S,
    mut progress: impl FnMut(usize),
) -> Result<MissingReport, MissingError<E>> {
    let mut report = MissingReport::default();
    for (done, md5) in md5s.iter().enumerate() {
        report.maps += 1;
        let path = cache_dir.join(format!("{md5}.osu"));
        let cached = fs::read(&path).ok().filter(|bytes| md5_hex(bytes) == *md5);
        let bytes = match cached {
            Some(bytes) => {
                report.cached += 1;
                Some(bytes)
            }
            None => download(md5, &path, catalog, source, &mut report)?,
        };
        if let Some(bytes) = bytes {
            attach(replays, md5, &path, &bytes);
        }
        progress(done + 1);
    }
    Ok(report)
}

fn download<E, S: MapSource<Error = E> + OsuFiles<Error = E>>(
    md5: &str,
    path: &Path,
    catalog: &mut MapCatalog,
    source: &mut S,
    report: &mut MissingReport,
) -> Result<Option<Vec<u8>>, MissingError<E>> {
    let found = source.lookup(md5).map_err(MissingError::Source)?;
    let Some((beatmap_id, found)) = found.and_then(|map| Some((map.info.beatmap_id?, map))) else {
        catalog.insert(
            md5.to_owned(),
            MapInfo {
                beatmap_id: None,
                status: MapStatus::Unknown,
                ranked_at: None,
                outdated: false,
                checked_at: None,
            },
        );
        report.not_found += 1;
        return Ok(None);
    };
    catalog.insert(md5.to_owned(), found.info);
    let bytes = source.download(beatmap_id).map_err(MissingError::Source)?;
    if md5_hex(&bytes) != md5 {
        report.mismatched += 1;
        return Ok(None);
    }
    let cache_error = |source| MissingError::Cache {
        path: path.to_path_buf(),
        source,
    };
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(cache_error)?;
    }
    write_atomic(path, &bytes).map_err(cache_error)?;
    report.downloaded += 1;
    Ok(Some(bytes))
}

fn attach(replays: &mut [ScannedReplay], md5: &str, path: &Path, bytes: &[u8]) {
    let metadata = BeatmapMetadata::parse(bytes);
    let missing = replays
        .iter_mut()
        .filter(|replay| replay.beatmap.is_none() && replay.header.beatmap_md5 == md5);
    for replay in missing {
        replay.beatmap = Some(LocalBeatmap {
            path: path.to_path_buf(),
            metadata: metadata.clone(),
        });
    }
}

fn md5_hex(bytes: &[u8]) -> String {
    hex(Md5::digest(bytes).as_slice())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fs;

    use md5::{Digest, Md5};
    use osu_formats::{BeatmapMetadata, Mods, ReplayHeader};

    use super::*;
    use crate::lost::SCORE_V1_END;
    use crate::maps::{FetchedMap, MapCatalog, MapInfo, MapSource, MapStatus};
    use crate::songs::hex;
    use crate::{LocalBeatmap, ScannedReplay};

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
                    checked_at: None,
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

    fn scored(md5: &str, score: u32) -> ScannedReplay {
        let mut replay = replay(md5);
        replay.header.score = score;
        replay
    }

    fn in_songs(md5: &str, score: u32) -> ScannedReplay {
        let mut replay = scored(md5, score);
        replay.beatmap = Some(LocalBeatmap {
            path: format!("Songs/{md5}.osu").into(),
            metadata: BeatmapMetadata::default(),
        });
        replay
    }

    fn fetch(
        md5s: &[&str],
        replays: &mut [ScannedReplay],
        catalog: &mut MapCatalog,
        cache: &Path,
        fake: &mut Fake,
    ) -> MissingReport {
        let md5s: Vec<String> = md5s.iter().map(|md5| md5.to_string()).collect();
        fetch_missing(&md5s, replays, catalog, cache, fake, |_| {}).unwrap()
    }

    #[test]
    fn only_maps_that_can_hold_a_lost_play_are_looked_up() {
        let mut hidden = scored("other_mods", 900_000);
        hidden.header.mods = Mods::from_bits(8);
        let mut late = scored("late", 900_000);
        late.header.timestamp_ticks = (SCORE_V1_END + 1) * 10_000_000 + 621_355_968_000_000_000;
        let replays = [
            replay("single"),
            replay("pair"),
            scored("pair", 900_000),
            replay("same_score"),
            replay("same_score"),
            replay("other_mods"),
            hidden,
            replay("late"),
            late,
        ];
        assert_eq!(missing_maps(&replays, &MapCatalog::default()), ["pair"]);
    }

    #[test]
    fn maps_in_songs_are_not_looked_up() {
        let replays = [
            in_songs("local", 1_000_000),
            in_songs("local", 900_000),
            replay("missing"),
            scored("missing", 900_000),
        ];
        assert_eq!(missing_maps(&replays, &MapCatalog::default()), ["missing"]);
    }

    #[test]
    fn downloads_the_map_of_a_replay_missing_from_songs() {
        let cache = tempfile::tempdir().unwrap();
        let md5 = md5_of(MAP);
        let mut fake = Fake::default();
        fake.maps.insert(md5.clone(), 5);
        fake.files.insert(5, MAP.to_vec());
        let mut replays = [replay(&md5)];
        let report = fetch(
            &[&md5],
            &mut replays,
            &mut MapCatalog::default(),
            cache.path(),
            &mut fake,
        );
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
        let report = fetch(
            &[&md5],
            &mut replays,
            &mut MapCatalog::default(),
            cache.path(),
            &mut fake,
        );
        assert!(fake.calls.is_empty());
        assert_eq!(report.cached, 1);
        assert!(replays[0].beatmap.is_some());
    }

    #[test]
    fn a_damaged_cached_file_is_downloaded_again() {
        let cache = tempfile::tempdir().unwrap();
        let md5 = md5_of(MAP);
        let path = cache.path().join(format!("{md5}.osu"));
        fs::write(&path, &MAP[..20]).unwrap();
        let mut fake = Fake::default();
        fake.maps.insert(md5.clone(), 5);
        fake.files.insert(5, MAP.to_vec());
        let mut replays = [replay(&md5)];
        let report = fetch(
            &[&md5],
            &mut replays,
            &mut MapCatalog::default(),
            cache.path(),
            &mut fake,
        );
        assert_eq!((report.cached, report.downloaded), (0, 1));
        assert_eq!(fs::read(&path).unwrap(), MAP);
        assert_eq!(replays[0].beatmap.as_ref().unwrap().metadata.title, "T");
    }

    #[test]
    fn a_map_gone_from_the_server_is_remembered() {
        let cache = tempfile::tempdir().unwrap();
        let mut fake = Fake::default();
        let mut catalog = MapCatalog::default();
        let mut replays = [replay("gone"), scored("gone", 900_000)];
        assert_eq!(missing_maps(&replays, &catalog), ["gone"]);
        let report = fetch(
            &["gone"],
            &mut replays,
            &mut catalog,
            cache.path(),
            &mut fake,
        );
        assert_eq!(report.not_found, 1);
        let info = catalog.get("gone").unwrap();
        assert_eq!((info.beatmap_id, info.status), (None, MapStatus::Unknown));
        assert!(missing_maps(&replays, &catalog).is_empty());
    }

    #[test]
    fn a_changed_map_is_not_attached() {
        let cache = tempfile::tempdir().unwrap();
        let md5 = md5_of(MAP);
        let mut fake = Fake::default();
        fake.maps.insert(md5.clone(), 5);
        fake.files.insert(5, b"osu file format v14\r\n".to_vec());
        let mut replays = [replay(&md5)];
        let report = fetch(
            &[&md5],
            &mut replays,
            &mut MapCatalog::default(),
            cache.path(),
            &mut fake,
        );
        assert!(replays[0].beatmap.is_none());
        assert_eq!(report.mismatched, 1);
        assert!(!cache.path().join(format!("{md5}.osu")).exists());
    }

    #[test]
    fn progress_is_reported_for_every_map() {
        let cache = tempfile::tempdir().unwrap();
        let md5 = md5_of(MAP);
        let mut fake = Fake::default();
        fake.maps.insert(md5.clone(), 5);
        fake.files.insert(5, MAP.to_vec());
        let mut replays = [replay(&md5), replay("gone")];
        let mut done = Vec::new();
        fetch_missing(
            &[md5, "gone".into()],
            &mut replays,
            &mut MapCatalog::default(),
            cache.path(),
            &mut fake,
            |count| done.push(count),
        )
        .unwrap();
        assert_eq!(done, [1, 2]);
    }
}
