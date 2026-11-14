use std::path::Path;

use anyhow::Result;
use lost_scores_app::history::{SavedScan, ScanEntry};

pub struct Newest {
    pub loaded: Option<(usize, SavedScan)>,
    pub failures: Vec<(i64, String)>,
}

pub fn load_newest(entries: &[ScanEntry], load: impl Fn(&Path) -> Result<SavedScan>) -> Newest {
    let mut failures = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        match load(&entry.folder) {
            Ok(scan) => {
                return Newest {
                    loaded: Some((index, scan)),
                    failures,
                };
            }
            Err(error) => failures.push((entry.made_at, format!("{error:#}"))),
        }
    }
    Newest {
        loaded: None,
        failures,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use anyhow::bail;
    use lost_scores_app::history::{SavedScan, ScanEntry, VERSION};

    use super::load_newest;

    fn entry(name: &str, made_at: i64) -> ScanEntry {
        ScanEntry {
            folder: PathBuf::from(name),
            made_at,
        }
    }

    fn scan(made_at: i64) -> SavedScan {
        SavedScan {
            version: VERSION,
            made_at,
            player_id: 1,
            player_name: "Player".into(),
            include_all: false,
            check_missing: false,
            lost: Vec::new(),
            lost_total: 0,
            lost_ranked: 0,
            potential: None,
        }
    }

    #[test]
    fn a_broken_newest_scan_falls_back_to_the_next_one() {
        let entries = [entry("c", 30), entry("b", 20), entry("a", 10)];
        let result = load_newest(&entries, |folder| {
            if folder.ends_with("c") {
                bail!("cannot read scan.json")
            }
            Ok(scan(if folder.ends_with("b") { 20 } else { 10 }))
        });
        let (index, loaded) = result.loaded.unwrap();
        assert_eq!((index, loaded.made_at), (1, 20));
        assert_eq!(result.failures, [(30, "cannot read scan.json".to_owned())]);
    }

    #[test]
    fn no_loadable_scan_gives_every_failure() {
        let entries = [entry("b", 20), entry("a", 10)];
        let result = load_newest(&entries, |_| bail!("bad"));
        assert!(result.loaded.is_none());
        assert_eq!(result.failures.len(), 2);
    }
}
