use std::path::Path;

use lost_scores_app::history::ScanEntry;

const MEGABYTE: f64 = 1_048_576.0;

pub fn size_text(bytes: u64) -> String {
    let megabytes = bytes as f64 / MEGABYTE;
    if megabytes >= 1024.0 {
        format!("{:.1} GB", megabytes / 1024.0)
    } else if megabytes >= 10.0 {
        format!("{megabytes:.0} MB")
    } else {
        format!("{megabytes:.1} MB")
    }
}

pub fn shown_after_delete(entries: &[ScanEntry], shown: Option<&Path>) -> usize {
    shown
        .and_then(|shown| entries.iter().position(|entry| entry.folder == shown))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use lost_scores_app::history::ScanEntry;

    use super::{shown_after_delete, size_text};

    #[test]
    fn sizes_read_in_megabytes() {
        assert_eq!(size_text(331_776), "0.3 MB");
        assert_eq!(size_text(9_900_000), "9.4 MB");
        assert_eq!(size_text(40_000_000), "38 MB");
        assert_eq!(size_text(1_288_490_189), "1.2 GB");
    }

    fn entry(name: &str, made_at: i64) -> ScanEntry {
        ScanEntry {
            folder: PathBuf::from("/scans").join(name),
            made_at,
        }
    }

    #[test]
    fn the_shown_scan_stays_unless_it_was_deleted() {
        let left = [entry("c", 3), entry("b", 2)];
        assert_eq!(shown_after_delete(&left, Some(Path::new("/scans/b"))), 1);
        assert_eq!(shown_after_delete(&left, Some(Path::new("/scans/a"))), 0);
        assert_eq!(shown_after_delete(&left, None), 0);
        assert_eq!(shown_after_delete(&[], Some(Path::new("/scans/a"))), 0);
    }
}
