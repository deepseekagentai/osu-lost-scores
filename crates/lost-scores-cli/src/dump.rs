use std::fmt;
use std::path::Path;

use lost_scores_core::{LocalBeatmap, Scan, ScannedReplay};
use serde::Serialize;

#[derive(Serialize)]
pub struct Dump<'a> {
    pub stats: Stats,
    unreadable_replays: Vec<Unreadable<'a>>,
    replays: Vec<Replay<'a>>,
}

#[derive(Serialize)]
pub struct Stats {
    replay_files: usize,
    unreadable_replays: usize,
    other_mode: usize,
    other_player: usize,
    unranked_mods: usize,
    selected: usize,
    beatmap_files: usize,
    unreadable_beatmaps: usize,
    selected_with_beatmap: usize,
    with_pp: usize,
    pp_failed_beatmaps: usize,
}

#[derive(Serialize)]
struct Unreadable<'a> {
    file: &'a str,
    error: String,
}

#[derive(Serialize)]
struct Replay<'a> {
    file: &'a str,
    beatmap_md5: &'a str,
    unix_time: i64,
    score: u32,
    count_300: u16,
    count_100: u16,
    count_50: u16,
    count_miss: u16,
    max_combo: u16,
    perfect: bool,
    mods: u32,
    pp: Option<f64>,
    beatmap: Option<Beatmap<'a>>,
}

#[derive(Serialize)]
struct Beatmap<'a> {
    path: String,
    beatmap_id: Option<u32>,
    beatmapset_id: Option<u32>,
    artist: &'a str,
    title: &'a str,
    version: &'a str,
    creator: &'a str,
}

impl<'a> Dump<'a> {
    pub fn new(scan: &'a Scan, pp_failed_beatmaps: usize) -> Self {
        Self {
            stats: Stats {
                replay_files: scan.replay_files,
                unreadable_replays: scan.unreadable_replays.len(),
                other_mode: scan.other_mode,
                other_player: scan.other_player,
                unranked_mods: scan.unranked_mods,
                selected: scan.replays.len(),
                beatmap_files: scan.beatmap_files,
                unreadable_beatmaps: scan.unreadable_beatmaps,
                selected_with_beatmap: scan
                    .replays
                    .iter()
                    .filter(|replay| replay.beatmap.is_some())
                    .count(),
                with_pp: scan
                    .replays
                    .iter()
                    .filter(|replay| replay.pp.is_some())
                    .count(),
                pp_failed_beatmaps,
            },
            unreadable_replays: scan
                .unreadable_replays
                .iter()
                .map(|replay| Unreadable {
                    file: &replay.file_name,
                    error: replay.error.to_string(),
                })
                .collect(),
            replays: scan.replays.iter().map(Replay::from).collect(),
        }
    }
}

impl<'a> From<&'a ScannedReplay> for Replay<'a> {
    fn from(replay: &'a ScannedReplay) -> Self {
        let header = &replay.header;
        Self {
            file: &replay.file_name,
            beatmap_md5: &header.beatmap_md5,
            unix_time: header.unix_time(),
            score: header.score,
            count_300: header.count_300,
            count_100: header.count_100,
            count_50: header.count_50,
            count_miss: header.count_miss,
            max_combo: header.max_combo,
            perfect: header.perfect,
            mods: header.mods.bits(),
            pp: replay.pp,
            beatmap: replay.beatmap.as_ref().map(Beatmap::from),
        }
    }
}

impl<'a> From<&'a LocalBeatmap> for Beatmap<'a> {
    fn from(beatmap: &'a LocalBeatmap) -> Self {
        let metadata = &beatmap.metadata;
        Self {
            path: slashed(&beatmap.path),
            beatmap_id: metadata.beatmap_id,
            beatmapset_id: metadata.beatmapset_id,
            artist: &metadata.artist,
            title: &metadata.title,
            version: &metadata.version,
            creator: &metadata.creator,
        }
    }
}

fn slashed(path: &Path) -> String {
    if path.is_absolute() {
        return path.display().to_string();
    }
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

impl fmt::Display for Stats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "replay files: {} ({} selected, {} other players, {} other modes, {} unranked mods, {} unreadable)",
            self.replay_files,
            self.selected,
            self.other_player,
            self.other_mode,
            self.unranked_mods,
            self.unreadable_replays
        )?;
        write!(
            f,
            "beatmap files: {} ({} unreadable); {} selected replays have a local beatmap, {} with pp ({} beatmaps failed to load)",
            self.beatmap_files,
            self.unreadable_beatmaps,
            self.selected_with_beatmap,
            self.with_pp,
            self.pp_failed_beatmaps
        )
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use lost_scores_core::{LocalBeatmap, ReplayReadError, Scan, ScannedReplay, UnreadableReplay};
    use osu_formats::{BeatmapMetadata, Mods, ReplayError, ReplayHeader};
    use serde_json::{Value, json};

    use super::{Beatmap, Dump};

    fn header(beatmap_md5: &str, timestamp_ticks: i64) -> ReplayHeader {
        ReplayHeader {
            mode: 0,
            version: 20190906,
            beatmap_md5: beatmap_md5.into(),
            player_name: "Lemon".into(),
            replay_md5: String::new(),
            count_300: 557,
            count_100: 160,
            count_50: 64,
            count_geki: 12,
            count_katu: 34,
            count_miss: 125,
            score: 447406,
            max_combo: 106,
            perfect: false,
            mods: Mods::from_bits(72),
            timestamp_ticks,
        }
    }

    fn dumped() -> Value {
        let beatmap = LocalBeatmap {
            path: PathBuf::from("Songs").join("1 A - B").join("x.osu"),
            metadata: BeatmapMetadata {
                title: "B".into(),
                artist: "A".into(),
                creator: "m".into(),
                version: "x".into(),
                beatmap_id: Some(11),
                beatmapset_id: Some(1),
            },
        };
        let scan = Scan {
            replay_files: 10,
            unreadable_replays: vec![UnreadableReplay {
                file_name: "f.osr".into(),
                error: ReplayReadError::Format(ReplayError::Truncated),
            }],
            other_mode: 1,
            other_player: 4,
            unranked_mods: 2,
            beatmap_files: 7,
            unreadable_beatmaps: 1,
            timings: Default::default(),
            replays: vec![
                ScannedReplay {
                    file_name: "a.osr".into(),
                    header: header("aaaa", 638_657_110_200_000_000),
                    beatmap: Some(beatmap),
                    pp: Some(4.5),
                },
                ScannedReplay {
                    file_name: "b.osr".into(),
                    header: header("bbbb", 638_657_110_210_000_000),
                    beatmap: None,
                    pp: None,
                },
            ],
        };
        serde_json::to_value(Dump::new(&scan, 2)).unwrap()
    }

    #[test]
    fn stats_count_selected_replays_and_found_beatmaps() {
        assert_eq!(
            dumped()["stats"],
            json!({
                "replay_files": 10,
                "unreadable_replays": 1,
                "other_mode": 1,
                "other_player": 4,
                "unranked_mods": 2,
                "selected": 2,
                "beatmap_files": 7,
                "unreadable_beatmaps": 1,
                "selected_with_beatmap": 1,
                "with_pp": 1,
                "pp_failed_beatmaps": 2,
            })
        );
    }

    #[test]
    fn an_absolute_beatmap_path_is_printed_as_it_is() {
        let path = std::env::temp_dir().join("osu").join("aaaa.osu");
        let beatmap = LocalBeatmap {
            path: path.clone(),
            metadata: BeatmapMetadata::default(),
        };
        assert_eq!(Beatmap::from(&beatmap).path, path.display().to_string());
    }

    #[test]
    fn replays_carry_raw_mods_unix_time_and_beatmap() {
        let value = dumped();
        assert_eq!(
            value["replays"][0],
            json!({
                "file": "a.osr",
                "beatmap_md5": "aaaa",
                "unix_time": 1730114220,
                "score": 447406,
                "count_300": 557,
                "count_100": 160,
                "count_50": 64,
                "count_miss": 125,
                "max_combo": 106,
                "perfect": false,
                "mods": 72,
                "pp": 4.5,
                "beatmap": {
                    "path": "Songs/1 A - B/x.osu",
                    "beatmap_id": 11,
                    "beatmapset_id": 1,
                    "artist": "A",
                    "title": "B",
                    "version": "x",
                    "creator": "m",
                },
            })
        );
        assert_eq!(value["replays"][1]["file"], "b.osr");
        assert_eq!(value["replays"][1]["beatmap"], Value::Null);
        assert_eq!(value["replays"][1]["pp"], Value::Null);
        assert_eq!(value["unreadable_replays"][0]["file"], "f.osr");
    }
}
