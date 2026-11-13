use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use md5::{Digest, Md5};
use osu_formats::BeatmapMetadata;
use rayon::prelude::*;
use walkdir::WalkDir;

use crate::ScanError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalBeatmap {
    pub path: PathBuf,
    pub metadata: BeatmapMetadata,
}

pub(crate) struct SongsIndex {
    pub(crate) beatmaps: HashMap<String, LocalBeatmap>,
    pub(crate) files: usize,
    pub(crate) unreadable: usize,
}

pub(crate) fn index_songs(osu_dir: &Path, songs_dir: &Path) -> Result<SongsIndex, ScanError> {
    let mut paths = Vec::new();
    let mut unreadable = 0;
    for entry in WalkDir::new(songs_dir) {
        match entry {
            Ok(entry) => {
                if !entry.file_type().is_dir()
                    && entry.path().extension() == Some(OsStr::new("osu"))
                {
                    paths.push(entry.into_path());
                }
            }
            Err(error) if error.depth() == 0 => {
                return Err(ScanError {
                    path: songs_dir.to_path_buf(),
                    source: error.into(),
                });
            }
            Err(_) => unreadable += 1,
        }
    }
    let files = paths.len();
    let mut hashed: Vec<_> = paths
        .into_par_iter()
        .filter_map(|path| {
            let data = fs::read(&path).ok()?;
            let md5 = hex(Md5::digest(&data).as_slice());
            Some((path, md5, BeatmapMetadata::parse(&data)))
        })
        .collect();
    unreadable += files - hashed.len();
    hashed.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    let mut beatmaps = HashMap::with_capacity(hashed.len());
    for (path, md5, metadata) in hashed {
        let path = path
            .strip_prefix(osu_dir)
            .map(Path::to_path_buf)
            .unwrap_or(path);
        beatmaps
            .entry(md5)
            .or_insert(LocalBeatmap { path, metadata });
    }
    Ok(SongsIndex {
        beatmaps,
        files,
        unreadable,
    })
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    bytes
        .iter()
        .flat_map(|&byte| {
            [
                DIGITS[usize::from(byte >> 4)],
                DIGITS[usize::from(byte & 0x0f)],
            ]
        })
        .map(char::from)
        .collect()
}
