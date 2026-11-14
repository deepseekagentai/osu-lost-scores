use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use osu_formats::{ReplayError, ReplayHeader};
use rayon::prelude::*;
use thiserror::Error;

use crate::LocalBeatmap;
use crate::songs::index_songs;
use crate::ticker::Ticker;

const STANDARD_MODE: u8 = 0;

#[derive(Debug)]
pub struct Scan {
    pub replay_files: usize,
    pub unreadable_replays: Vec<UnreadableReplay>,
    pub other_mode: usize,
    pub other_player: usize,
    pub unranked_mods: usize,
    pub beatmap_files: usize,
    pub unreadable_beatmaps: usize,
    pub replays: Vec<ScannedReplay>,
    pub timings: ScanTimings,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ScanTimings {
    pub songs: Duration,
    pub replays: Duration,
}

#[derive(Debug)]
pub struct ScannedReplay {
    pub file_name: String,
    pub header: ReplayHeader,
    pub beatmap: Option<LocalBeatmap>,
    pub pp: Option<f64>,
}

#[derive(Debug)]
pub struct UnreadableReplay {
    pub file_name: String,
    pub error: ReplayReadError,
}

#[derive(Debug, Error)]
pub enum ReplayReadError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Format(#[from] ReplayError),
}

#[derive(Debug, Error)]
#[error("cannot read {}", .path.display())]
pub struct ScanError {
    pub path: PathBuf,
    pub source: io::Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Songs,
    Replays,
}

pub fn scan(osu_dir: &Path, songs_dir: &Path, players: &[String]) -> Result<Scan, ScanError> {
    scan_with(osu_dir, songs_dir, players, &|_, _, _| {})
}

pub fn scan_with(
    osu_dir: &Path,
    songs_dir: &Path,
    players: &[String],
    progress: &(dyn Fn(Stage, usize, usize) + Sync),
) -> Result<Scan, ScanError> {
    let started = Instant::now();
    let replay_paths = list_replays(&osu_dir.join("Data").join("r"))?;
    let listed = started.elapsed();
    let started = Instant::now();
    let songs = index_songs(osu_dir, songs_dir, &|done, total| {
        progress(Stage::Songs, done, total)
    })?;
    let songs_took = started.elapsed();
    let started = Instant::now();
    let report_replays = |done, total| progress(Stage::Replays, done, total);
    let replay_ticker = Ticker::new(replay_paths.len(), &report_replays);
    let parsed: Vec<_> = replay_paths
        .into_par_iter()
        .map(|path| {
            let file_name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let header = read_header(&path);
            replay_ticker.tick();
            (file_name, header)
        })
        .collect();

    let wanted: Vec<String> = players
        .iter()
        .map(|name| name.trim().to_lowercase())
        .collect();
    let mut report = Scan {
        replay_files: parsed.len(),
        unreadable_replays: Vec::new(),
        other_mode: 0,
        other_player: 0,
        unranked_mods: 0,
        beatmap_files: songs.files,
        unreadable_beatmaps: songs.unreadable,
        replays: Vec::new(),
        timings: ScanTimings::default(),
    };
    for (file_name, header) in parsed {
        match header {
            Err(error) => report
                .unreadable_replays
                .push(UnreadableReplay { file_name, error }),
            Ok(header) if header.mode != STANDARD_MODE => report.other_mode += 1,
            Ok(header) if !wanted.contains(&header.player_name.trim().to_lowercase()) => {
                report.other_player += 1
            }
            Ok(header) if !header.mods.is_ranked_in_standard() => report.unranked_mods += 1,
            Ok(header) => {
                let beatmap = songs.beatmaps.get(&header.beatmap_md5).cloned();
                report.replays.push(ScannedReplay {
                    file_name,
                    header,
                    beatmap,
                    pp: None,
                });
            }
        }
    }
    report.replays.sort_by(|a, b| {
        (a.header.timestamp_ticks, &a.file_name).cmp(&(b.header.timestamp_ticks, &b.file_name))
    });
    report
        .unreadable_replays
        .sort_by(|a, b| a.file_name.cmp(&b.file_name));
    report.timings = ScanTimings {
        songs: songs_took,
        replays: listed + started.elapsed(),
    };
    Ok(report)
}

fn list_replays(dir: &Path) -> Result<Vec<PathBuf>, ScanError> {
    let error = |source| ScanError {
        path: dir.to_path_buf(),
        source,
    };
    let mut paths = Vec::new();
    for entry in fs::read_dir(dir).map_err(error)? {
        let path = entry.map_err(error)?.path();
        if path.extension() == Some(OsStr::new("osr")) {
            paths.push(path);
        }
    }
    Ok(paths)
}

fn read_header(path: &Path) -> Result<ReplayHeader, ReplayReadError> {
    Ok(ReplayHeader::parse(&fs::read(path)?)?)
}
