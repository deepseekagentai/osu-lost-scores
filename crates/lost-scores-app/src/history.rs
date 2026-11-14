use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use jiff::Timestamp;
use jiff::civil::DateTime;
use jiff::tz::TimeZone;
use lost_scores_core::files::write_atomic;
use serde::{Deserialize, Serialize};

use crate::pipeline::PotentialResult;
use crate::rows::LostRow;
use crate::setup;

pub const VERSION: u32 = 1;
const FILE: &str = "scan.json";
const FOLDER_FORMAT: &str = "%Y-%m-%d_%H-%M-%S";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedScan {
    pub version: u32,
    pub made_at: i64,
    pub player_id: u32,
    pub player_name: String,
    pub include_all: bool,
    pub check_missing: bool,
    pub lost: Vec<LostRow>,
    pub lost_total: usize,
    pub lost_ranked: usize,
    pub potential: Option<PotentialResult>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScanEntry {
    pub folder: PathBuf,
    pub made_at: i64,
}

pub fn scans_dir() -> Option<PathBuf> {
    setup::data_root().map(|root| root.join("scans"))
}

pub fn save(dir: &Path, scan: &SavedScan) -> Result<PathBuf> {
    let made_at = Timestamp::from_second(scan.made_at)?.to_zoned(TimeZone::system());
    let folder = dir.join(made_at.strftime(FOLDER_FORMAT).to_string());
    fs::create_dir_all(&folder).with_context(|| format!("cannot create {}", folder.display()))?;
    let file = folder.join(FILE);
    write_atomic(&file, &serde_json::to_vec(scan)?)
        .with_context(|| format!("cannot write {}", file.display()))?;
    Ok(folder)
}

pub fn list(dir: &Path) -> Vec<ScanEntry> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut scans: Vec<ScanEntry> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let folder = entry.path();
            let name = folder.file_name()?.to_str()?;
            let made_at = DateTime::strptime(FOLDER_FORMAT, name)
                .ok()?
                .to_zoned(TimeZone::system())
                .ok()?
                .timestamp()
                .as_second();
            folder
                .join(FILE)
                .is_file()
                .then_some(ScanEntry { folder, made_at })
        })
        .collect();
    scans.sort_by_key(|scan| std::cmp::Reverse(scan.made_at));
    scans
}

pub fn load(folder: &Path) -> Result<SavedScan> {
    let file = folder.join(FILE);
    let data = fs::read(&file).with_context(|| format!("cannot read {}", file.display()))?;
    serde_json::from_slice(&data).with_context(|| format!("cannot parse {}", file.display()))
}

#[cfg(test)]
mod tests {
    use lost_scores_core::grade::Grade;
    use lost_scores_core::maps::MapStatus;

    use super::{SavedScan, VERSION, list, load, save};
    use crate::rows::{Hits, LostRow};

    fn row() -> LostRow {
        LostRow {
            pp: 321.5,
            kept_pp: 300.25,
            score: 1_234_567,
            kept_score: 1_200_000,
            mods: "HDDT".into(),
            unix_time: 1_700_000_000,
            played: "2023-11-14 22:13".into(),
            status: MapStatus::Ranked,
            beatmap_md5: "0b577cbe9bc774600d20cfa17c0aa94c".into(),
            beatmap_id: Some(11),
            artist: "A".into(),
            title: "B".into(),
            version: "x".into(),
            file: "a.osr".into(),
            creator: "m".into(),
            hits: Hits {
                n300: 300,
                n100: 20,
                n50: 1,
                misses: 2,
            },
            accuracy: 96.5,
            grade: Grade::S,
            max_combo: 500,
        }
    }

    fn scan(made_at: i64) -> SavedScan {
        SavedScan {
            version: VERSION,
            made_at,
            player_id: 7,
            player_name: "Lemon".into(),
            include_all: true,
            check_missing: false,
            lost: vec![row()],
            lost_total: 12,
            lost_ranked: 9,
            potential: None,
        }
    }

    #[test]
    fn a_saved_scan_loads_back() {
        let dir = tempfile::tempdir().unwrap();
        let saved = scan(1_700_000_000);
        let folder = save(dir.path(), &saved).unwrap();
        assert!(folder.starts_with(dir.path()) && folder.join("scan.json").is_file());
        assert_eq!(load(&folder).unwrap(), saved);
    }

    #[test]
    fn scans_are_listed_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let times = [1_600_000_000, 1_700_000_000, 1_650_000_000];
        for made_at in times {
            save(dir.path(), &scan(made_at)).unwrap();
        }
        std::fs::create_dir(dir.path().join("not-a-scan")).unwrap();
        let listed: Vec<i64> = list(dir.path()).iter().map(|e| e.made_at).collect();
        assert_eq!(listed, [1_700_000_000, 1_650_000_000, 1_600_000_000]);
        assert!(list(&dir.path().join("gone")).is_empty());
    }
}
