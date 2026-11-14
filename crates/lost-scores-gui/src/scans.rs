use std::path::Path;

use lost_scores_app::history::{self, ScanEntry};

const MEGABYTE: f64 = 1_048_576.0;

pub fn size_text(bytes: u64) -> String {
    let megabytes = bytes as f64 / MEGABYTE;
    if (megabytes * 10.0).round() < 100.0 {
        format!("{megabytes:.1} MB")
    } else if megabytes.round() < 1024.0 {
        format!("{megabytes:.0} MB")
    } else {
        format!("{:.1} GB", megabytes / 1024.0)
    }
}

pub fn shown_after_delete(entries: &[ScanEntry], shown: Option<&Path>) -> usize {
    shown
        .and_then(|shown| entries.iter().position(|entry| entry.folder == shown))
        .unwrap_or(0)
}

fn older_scans(count: usize) -> String {
    let noun = if count == 1 { "scan" } else { "scans" };
    format!("{count} older {noun}")
}

pub fn older_prompt(count: usize, bytes: u64) -> String {
    format!(
        "Move {} ({}) to the trash? The newest scan stays.",
        older_scans(count),
        size_text(bytes)
    )
}

pub fn older_done(count: usize) -> String {
    format!("Moved {} to the trash", older_scans(count))
}

pub fn trash_message(error: &trash::Error) -> String {
    match error {
        #[cfg(all(
            unix,
            not(target_os = "macos"),
            not(target_os = "ios"),
            not(target_os = "android")
        ))]
        trash::Error::FileSystem { path, source } => format!("{}: {source}", path.display()),
        trash::Error::Os { code, description } if description.trim().is_empty() => {
            format!("the system refused with error {code}")
        }
        trash::Error::Os { description, .. } => description.trim().to_owned(),
        trash::Error::Unknown { description } if description.trim().is_empty() => {
            "the trash failed for an unknown reason".to_owned()
        }
        trash::Error::Unknown { description } => description.trim().to_owned(),
        trash::Error::TargetedRoot => "the root folder cannot be moved to the trash".to_owned(),
        trash::Error::CouldNotAccess { target } => format!("cannot access {target}"),
        trash::Error::CanonicalizePath { original } => {
            format!("cannot resolve the path {}", original.display())
        }
        trash::Error::ConvertOsString { original } => {
            format!(
                "cannot read the name {} as text",
                original.to_string_lossy()
            )
        }
        trash::Error::RestoreCollision { path, .. } => {
            format!("{} already exists", path.display())
        }
        trash::Error::RestoreTwins { path, .. } => {
            format!("several items belong at {}", path.display())
        }
    }
}

pub struct Reread {
    pub entries: Vec<ScanEntry>,
    pub index: usize,
    pub shown_gone: bool,
}

pub fn reread(dir: &Path, shown: Option<&Path>) -> Reread {
    let entries = history::list(dir);
    let index = shown_after_delete(&entries, shown);
    let shown_gone = shown.is_some_and(|shown| entries.iter().all(|entry| entry.folder != shown));
    Reread {
        entries,
        index,
        shown_gone,
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use lost_scores_app::history::{self, ScanEntry};

    use super::{older_done, older_prompt, reread, shown_after_delete, size_text, trash_message};

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

    #[test]
    fn sizes_pick_the_unit_after_rounding() {
        assert_eq!(size_text(10_434_000), "10 MB");
        assert_eq!(size_text(10_485_760), "10 MB");
        assert_eq!(size_text(1_073_300_000), "1.0 GB");
        assert_eq!(size_text(1_073_741_824), "1.0 GB");
        assert_eq!(size_text(0), "0.0 MB");
    }

    #[test]
    fn one_older_scan_is_singular() {
        assert_eq!(
            older_prompt(1, 331_776),
            "Move 1 older scan (0.3 MB) to the trash? The newest scan stays."
        );
        assert_eq!(
            older_prompt(2, 650_000),
            "Move 2 older scans (0.6 MB) to the trash? The newest scan stays."
        );
        assert_eq!(older_done(1), "Moved 1 older scan to the trash");
        assert_eq!(older_done(3), "Moved 3 older scans to the trash");
    }

    #[test]
    fn trash_errors_read_in_words() {
        let os = trash::Error::Os {
            code: 5,
            description: "Access is denied.".into(),
        };
        assert_eq!(trash_message(&os), "Access is denied.");
        let silent = trash::Error::Os {
            code: 32,
            description: " ".into(),
        };
        assert_eq!(trash_message(&silent), "the system refused with error 32");
        let said = trash::Error::Unknown {
            description: "no trash folder on this drive\n".into(),
        };
        assert_eq!(trash_message(&said), "no trash folder on this drive");
        let unsaid = trash::Error::Unknown {
            description: String::new(),
        };
        assert_eq!(
            trash_message(&unsaid),
            "the trash failed for an unknown reason"
        );
        assert_eq!(
            trash_message(&trash::Error::TargetedRoot),
            "the root folder cannot be moved to the trash"
        );
        let blocked = trash::Error::CouldNotAccess {
            target: "/scans/2026-11-12_09-00-00Z".into(),
        };
        assert_eq!(
            trash_message(&blocked),
            "cannot access /scans/2026-11-12_09-00-00Z"
        );
        let lost = trash::Error::CanonicalizePath {
            original: PathBuf::from("/scans/gone"),
        };
        assert_eq!(trash_message(&lost), "cannot resolve the path /scans/gone");
        let named = trash::Error::ConvertOsString {
            original: std::ffi::OsString::from("scan"),
        };
        assert_eq!(trash_message(&named), "cannot read the name scan as text");
        let collision = trash::Error::RestoreCollision {
            path: PathBuf::from("/scans/x"),
            remaining_items: Vec::new(),
        };
        assert_eq!(trash_message(&collision), "/scans/x already exists");
        let twins = trash::Error::RestoreTwins {
            path: PathBuf::from("/scans/x"),
            items: Vec::new(),
        };
        assert_eq!(trash_message(&twins), "several items belong at /scans/x");
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            let denied = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
            let error = trash::Error::FileSystem {
                path: PathBuf::from("/scans/2026-11-12_09-00-00Z"),
                source: denied,
            };
            assert_eq!(
                trash_message(&error),
                "/scans/2026-11-12_09-00-00Z: permission denied"
            );
        }
    }

    #[test]
    fn a_partial_move_leaves_a_list_without_the_moved_folder() {
        let dir = tempfile::tempdir().unwrap();
        let mut folders = Vec::new();
        for name in [
            "2026-11-14_09-00-00Z",
            "2026-11-13_09-00-00Z",
            "2026-11-12_09-00-00Z",
        ] {
            let folder = dir.path().join(name);
            std::fs::create_dir(&folder).unwrap();
            std::fs::write(folder.join("scan.json"), b"{}").unwrap();
            folders.push(folder);
        }
        let older = folders[1..].to_vec();
        let result = history::remove(dir.path(), &older, |handed| {
            std::fs::remove_dir_all(&handed[0]).unwrap();
            Err(anyhow::anyhow!("a file is open in another program"))
        });
        assert!(result.is_err());
        let read = reread(dir.path(), Some(&folders[1]));
        let left: Vec<_> = read
            .entries
            .iter()
            .map(|entry| entry.folder.clone())
            .collect();
        assert_eq!(left, vec![folders[0].clone(), folders[2].clone()]);
        assert!(read.shown_gone);
        assert_eq!(read.index, 0);
        let kept = reread(dir.path(), Some(&folders[2]));
        assert!(!kept.shown_gone);
        assert_eq!(kept.index, 1);
        assert!(!reread(dir.path(), None).shown_gone);
    }
}
