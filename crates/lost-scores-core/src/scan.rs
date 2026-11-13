use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use osu_formats::{ReplayError, ReplayHeader};
use rayon::prelude::*;
use thiserror::Error;

use crate::LocalBeatmap;
use crate::songs::index_songs;

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

pub fn scan(osu_dir: &Path, player: &str) -> Result<Scan, ScanError> {
    let replay_paths = list_replays(&osu_dir.join("Data").join("r"))?;
    let songs = index_songs(osu_dir)?;
    let parsed: Vec<_> = replay_paths
        .into_par_iter()
        .map(|path| {
            let file_name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            (file_name, read_header(&path))
        })
        .collect();

    let wanted = player.trim().to_lowercase();
    let mut report = Scan {
        replay_files: parsed.len(),
        unreadable_replays: Vec::new(),
        other_mode: 0,
        other_player: 0,
        unranked_mods: 0,
        beatmap_files: songs.files,
        unreadable_beatmaps: songs.unreadable,
        replays: Vec::new(),
    };
    for (file_name, header) in parsed {
        match header {
            Err(error) => report
                .unreadable_replays
                .push(UnreadableReplay { file_name, error }),
            Ok(header) if header.mode != STANDARD_MODE => report.other_mode += 1,
            Ok(header) if header.player_name.trim().to_lowercase() != wanted => {
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
