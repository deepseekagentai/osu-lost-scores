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

pub fn newest_of_player(
    entries: &[ScanEntry],
    player_id: u32,
    load: impl Fn(&Path) -> Result<SavedScan>,
) -> Option<SavedScan> {
    entries
        .iter()
        .filter_map(|entry| load(&entry.folder).ok())
        .find(|scan| scan.player_id == player_id)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use anyhow::bail;
    use lost_scores_app::history::{SavedScan, ScanEntry, VERSION};

    use super::{load_newest, newest_of_player};

    fn entry(name: &str, made_at: i64) -> ScanEntry {
        ScanEntry {
            folder: PathBuf::from(name),
            made_at,
        }
    }

    fn scan(made_at: i64) -> SavedScan {
        player_scan(made_at, 1)
    }

    fn player_scan(made_at: i64, player_id: u32) -> SavedScan {
        SavedScan {
            version: VERSION,
            made_at,
            player_id,
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

    #[test]
    fn the_card_shows_the_newest_scan_of_this_player_on_start() {
        let entries = [entry("c", 30), entry("b", 20), entry("a", 10)];
        let load = |folder: &Path| {
            if folder.ends_with("b") {
                bail!("cannot read scan.json")
            }
            let (made_at, player) = if folder.ends_with("c") {
                (30, 2)
            } else {
                (10, 1)
            };
            Ok(player_scan(made_at, player))
        };
        let own = newest_of_player(&entries, 1, load).unwrap();
        assert_eq!((own.made_at, own.player_id), (10, 1));
        assert_eq!(newest_of_player(&entries, 2, load).unwrap().made_at, 30);
        assert!(newest_of_player(&entries, 3, load).is_none());
    }
}
