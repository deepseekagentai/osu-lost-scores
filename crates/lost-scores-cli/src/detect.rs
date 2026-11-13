use std::env;
use std::fs;
use std::path::{Path, PathBuf};

pub fn is_osu_dir(path: &Path) -> bool {
    path.join("Data").join("r").is_dir()
}

pub fn find_osu_dirs(patterns: &[PathBuf]) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for pattern in patterns {
        let mut current = vec![PathBuf::new()];
        for component in pattern.components() {
            let part = component.as_os_str();
            current = if part == "*" {
                current
                    .iter()
                    .flat_map(|dir| fs::read_dir(dir).into_iter().flatten())
                    .filter_map(Result::ok)
                    .map(|entry| entry.path())
                    .filter(|path| path.is_dir())
                    .collect()
            } else {
                current.into_iter().map(|dir| dir.join(part)).collect()
            };
        }
        found.extend(current.into_iter().filter(|dir| is_osu_dir(dir)));
    }
    found.sort();
    found.dedup();
    found
}

#[derive(Debug, Default)]
pub struct Places {
    pub home: Option<PathBuf>,
    pub user: Option<PathBuf>,
    pub data_home: Option<PathBuf>,
    pub wine_prefix: Option<PathBuf>,
    pub local_app_data: Option<PathBuf>,
}

impl Places {
    pub fn current() -> Self {
        let var = |name: &str| {
            env::var_os(name)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        };
        let home = var("HOME");
        Self {
            user: var("USER")
                .or_else(|| home.as_ref().and_then(|h| h.file_name()).map(PathBuf::from)),
            data_home: var("XDG_DATA_HOME")
                .or_else(|| home.as_ref().map(|h| h.join(".local/share"))),
            wine_prefix: var("WINEPREFIX"),
            local_app_data: var("LOCALAPPDATA"),
            home,
        }
    }
}

const INSIDE_PREFIX: [&str; 2] = ["drive_c/users/*/AppData/Local/osu!", "drive_c/osu!"];

pub fn candidates(places: &Places) -> Vec<PathBuf> {
    let mut found = Vec::new();
    if let Some(local) = &places.local_app_data {
        found.push(local.join("osu!"));
        found.extend(["C:\\osu!", "D:\\osu!"].map(PathBuf::from));
    }
    let saved_by_winello = places
        .data_home
        .as_ref()
        .and_then(|data| fs::read_to_string(data.join("osuconfig").join("osupath")).ok());
    if let Some(path) = saved_by_winello {
        found.push(PathBuf::from(path.trim()));
    }
    let mut prefixes: Vec<PathBuf> = places.wine_prefix.iter().cloned().collect();
    if let Some(home) = &places.home {
        prefixes.extend([
            home.join(".wine"),
            home.join("Games/*"),
            home.join(".local/share/bottles/bottles/*"),
            home.join(".var/app/com.usebottles.bottles/data/bottles/bottles/*"),
        ]);
    }
    for prefix in prefixes {
        found.extend(INSIDE_PREFIX.map(|inner| prefix.join(inner)));
    }
    if let Some(user) = &places.user {
        for media in ["/media", "/run/media"] {
            found.push(
                Path::new(media)
                    .join(user)
                    .join("*/Users/*/AppData/Local/osu!"),
            );
        }
    }
    found.push(PathBuf::from("/mnt/*/Users/*/AppData/Local/osu!"));
    found
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::{Places, candidates, find_osu_dirs, is_osu_dir};

    fn osu_folder(root: &Path) {
        fs::create_dir_all(root.join("Data").join("r")).unwrap();
    }

    #[test]
    fn wildcards_find_only_real_osu_folders() {
        let root = tempfile::tempdir().unwrap();
        let good = root.path().join("media/u/Disk1/Users/A/AppData/Local/osu!");
        osu_folder(&good);
        fs::create_dir_all(root.path().join("media/u/Disk2/Users/B/AppData/Local/osu!")).unwrap();
        let pattern = root.path().join("media/u/*/Users/*/AppData/Local/osu!");
        assert_eq!(find_osu_dirs(&[pattern]), vec![good.clone()]);
        assert!(is_osu_dir(&good));
    }

    #[test]
    fn launchers_and_wine_prefixes_are_searched() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let winello = root.path().join("anywhere/osu!");
        osu_folder(&winello);
        fs::create_dir_all(home.join(".local/share/osuconfig")).unwrap();
        fs::write(
            home.join(".local/share/osuconfig/osupath"),
            format!("{}\n", winello.display()),
        )
        .unwrap();
        let lutris = home.join("Games/osu/drive_c/users/me/AppData/Local/osu!");
        osu_folder(&lutris);
        let custom = root.path().join("prefix/drive_c/osu!");
        osu_folder(&custom);
        let places = Places {
            home: Some(home.clone()),
            data_home: Some(home.join(".local/share")),
            wine_prefix: Some(root.path().join("prefix")),
            ..Places::default()
        };
        let mut expected = vec![winello, lutris, custom];
        expected.sort();
        assert_eq!(find_osu_dirs(&candidates(&places)), expected);
    }
}
