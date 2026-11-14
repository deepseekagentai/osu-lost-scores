use std::fs;
use std::io;
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
const FOLDER_FORMAT: &str = "%Y-%m-%d_%H-%M-%SZ";

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
    let made_at = Timestamp::from_second(scan.made_at)?.to_zoned(TimeZone::UTC);
    let name = made_at.strftime(FOLDER_FORMAT).to_string();
    fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let mut folder = dir.join(&name);
    let mut copy = 2;
    loop {
        match fs::create_dir(&folder) {
            Ok(()) => break,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                folder = dir.join(format!("{name}-{copy}"));
                copy += 1;
            }
            Err(error) => {
                return Err(error).with_context(|| format!("cannot create {}", folder.display()));
            }
        }
    }
    replace(&folder, scan)?;
    Ok(folder)
}

pub fn replace(folder: &Path, scan: &SavedScan) -> Result<()> {
    let file = folder.join(FILE);
    write_atomic(&file, &serde_json::to_vec(scan)?)
        .with_context(|| format!("cannot write {}", file.display()))
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
            let stamp = name.get(..20)?;
            let made_at = DateTime::strptime(FOLDER_FORMAT, stamp)
                .ok()?
                .to_zoned(TimeZone::UTC)
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

    use super::{SavedScan, VERSION, list, load, replace, save};
    use crate::pipeline::PotentialResult;
    use crate::rows::{Hits, LostRow, OnlineRow, PotentialRow};

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

    fn weighted_rows() -> (Vec<PotentialRow>, Vec<OnlineRow>) {
        let hits = Hits {
            n300: 812,
            n100: 31,
            n50: 2,
            misses: 1,
        };
        let potential = (0..5)
            .map(|i| {
                let weight = 0.95_f64.powi(i);
                let pp = 412.3456789 - 17.123456789 * f64::from(i);
                PotentialRow {
                    place: i as usize + 1,
                    pp,
                    weight,
                    weighted: pp * weight,
                    lost: i % 2 == 1,
                    beatmap_id: 100 + i as u32,
                    artist: "A".into(),
                    title: "B".into(),
                    version: "x".into(),
                    creator: "m".into(),
                    mods: "HDDT".into(),
                    accuracy: 98.7654321012345,
                    unix_time: 1_700_000_000 + i64::from(i),
                    played: "2023-11-14".into(),
                    hits,
                    score: 12_345_678_901,
                    grade: Grade::S,
                    status: MapStatus::Loved,
                    kept_pp: (i % 2 == 1).then_some(0.1 + 0.2),
                }
            })
            .collect();
        let online = (0..5)
            .map(|i| {
                let weight = 0.95_f64.powi(i);
                let pp = 399.99999999999994 - 13.37 * f64::from(i);
                OnlineRow {
                    place: i as usize + 1,
                    pp,
                    weight,
                    weighted: pp * weight,
                    beatmap_id: 200 + i as u32,
                    artist: "C".into(),
                    title: "D".into(),
                    version: "y".into(),
                    creator: "n".into(),
                    mods: "HR".into(),
                    accuracy: 99.12345678901234,
                    grade: Grade::XH,
                    hits,
                    score: 987_654_321,
                    unix_time: 1_690_000_000 + i64::from(i),
                    played: "2023-07-22".into(),
                }
            })
            .collect();
        (potential, online)
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
    fn a_saved_scan_with_a_potential_top_loads_back_exactly() {
        let dir = tempfile::tempdir().unwrap();
        let (rows, online) = weighted_rows();
        let mut saved = scan(1_700_000_000);
        saved.potential = Some(PotentialResult {
            current_pp: 8123.456789012345,
            potential_pp: 8234.567890123456,
            current_accuracy: 98.76543210987654,
            potential_accuracy: 98.87654321098765,
            global_rank: Some(12_345),
            lost_in_top: 2,
            rows,
            online,
        });
        let folder = save(dir.path(), &saved).unwrap();
        assert_eq!(load(&folder).unwrap(), saved);
    }

    #[test]
    fn a_replaced_scan_keeps_its_folder() {
        let dir = tempfile::tempdir().unwrap();
        let folder = save(dir.path(), &scan(1_700_000_000)).unwrap();
        let mut changed = scan(1_700_000_000);
        changed.lost_total = 99;
        replace(&folder, &changed).unwrap();
        assert_eq!(load(&folder).unwrap().lost_total, 99);
        assert_eq!(list(dir.path()).len(), 1);
    }

    #[test]
    fn folder_names_are_utc() {
        let dir = tempfile::tempdir().unwrap();
        let folder = save(dir.path(), &scan(1_700_000_000)).unwrap();
        assert_eq!(
            folder.file_name().unwrap().to_str().unwrap(),
            "2023-11-14_22-13-20Z"
        );
        assert_eq!(list(dir.path())[0].made_at, 1_700_000_000);
    }

    #[test]
    fn two_scans_in_the_same_second_get_two_folders() {
        let dir = tempfile::tempdir().unwrap();
        let mut first = scan(1_700_000_000);
        first.lost_total = 1;
        let mut second = scan(1_700_000_000);
        second.lost_total = 2;
        let mut third = scan(1_700_000_000);
        third.lost_total = 3;
        let folders: Vec<_> = [&first, &second, &third]
            .iter()
            .map(|scan| save(dir.path(), scan).unwrap())
            .collect();
        let names: Vec<_> = folders
            .iter()
            .map(|f| f.file_name().unwrap().to_str().unwrap().to_owned())
            .collect();
        assert_eq!(
            names,
            [
                "2023-11-14_22-13-20Z",
                "2023-11-14_22-13-20Z-2",
                "2023-11-14_22-13-20Z-3"
            ]
        );
        assert_eq!(load(&folders[0]).unwrap(), first);
        assert_eq!(load(&folders[1]).unwrap(), second);
        assert_eq!(list(dir.path()).len(), 3);
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
