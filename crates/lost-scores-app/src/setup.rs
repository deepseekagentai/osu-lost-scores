use std::env;
use std::fs;
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use lost_scores_core::files::write_atomic;
use serde::{Deserialize, Serialize};

use crate::detect;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct UserConfig {
    pub usernames: Vec<String>,
    pub beatmap_directory: Option<String>,
}

pub const SCORES_TO_SHOW: RangeInclusive<u16> = 1..=250;
const DEFAULT_SCORES_TO_SHOW: u16 = 10;

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub osu_dir: Option<PathBuf>,
    pub player: Option<String>,
    pub include_all: bool,
    pub check_missing: bool,
    pub show_lost_in_top_image: bool,
    pub scores_to_show: u16,
    pub catbox_confirmed: bool,
    pub update_checked_at: Option<i64>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            osu_dir: None,
            player: None,
            include_all: false,
            check_missing: false,
            show_lost_in_top_image: false,
            scores_to_show: DEFAULT_SCORES_TO_SHOW,
            catbox_confirmed: false,
            update_checked_at: None,
        }
    }
}

pub fn scores_to_show_typed(text: &str) -> Option<u16> {
    let typed: i128 = text.trim().parse().ok()?;
    let low = i128::from(*SCORES_TO_SHOW.start());
    let high = i128::from(*SCORES_TO_SHOW.end());
    u16::try_from(typed.clamp(low, high)).ok()
}

pub fn parse_scores_to_show(text: &str) -> Option<u16> {
    text.trim()
        .parse()
        .ok()
        .filter(|count| SCORES_TO_SHOW.contains(count))
}

pub fn read_user_config(osu_dir: &Path) -> UserConfig {
    let mut config = UserConfig::default();
    let Ok(entries) = fs::read_dir(osu_dir) else {
        return config;
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            name.starts_with("osu!.") && name.ends_with(".cfg") && name != "osu!.cfg"
        })
        .collect();
    files.sort();
    for path in files {
        let Ok(data) = fs::read(&path) else {
            continue;
        };
        for line in String::from_utf8_lossy(&data).lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim();
            if value.is_empty() {
                continue;
            }
            match key.trim() {
                "Username" if !config.usernames.iter().any(|known| known == value) => {
                    config.usernames.push(value.to_owned())
                }
                "BeatmapDirectory" if config.beatmap_directory.is_none() => {
                    config.beatmap_directory = Some(value.to_owned())
                }
                _ => {}
            }
        }
    }
    config
}

pub fn player(config: &UserConfig) -> Result<String> {
    match config.usernames.as_slice() {
        [name] => Ok(name.clone()),
        [] => bail!("no player name in the osu! config, pass --player <name>"),
        names => bail!(
            "several players in the osu! config ({}), pass --player <name>",
            names.join(", ")
        ),
    }
}

pub fn songs_dir(osu_dir: &Path, config: &UserConfig) -> Result<PathBuf> {
    let Some(value) = &config.beatmap_directory else {
        return Ok(osu_dir.join("Songs"));
    };
    let path = Path::new(value);
    if !path.is_absolute() {
        return Ok(osu_dir.join(path));
    }
    ensure!(
        path.is_dir(),
        "the Songs folder from the osu! config does not exist: {}, pass --songs-dir <path>",
        path.display()
    );
    Ok(path.to_path_buf())
}

pub fn load_settings(path: &Path) -> Settings {
    let mut settings: Settings = fs::read(path)
        .ok()
        .and_then(|data| serde_json::from_slice(&data).ok())
        .unwrap_or_default();
    settings.scores_to_show = settings
        .scores_to_show
        .clamp(*SCORES_TO_SHOW.start(), *SCORES_TO_SHOW.end());
    settings
}

pub fn save_settings(path: &Path, settings: &Settings) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("cannot create {}", parent.display()))?;
    }
    write_atomic(path, &serde_json::to_vec_pretty(settings)?)
        .with_context(|| format!("cannot write {}", path.display()))
}

pub fn config_file() -> Option<PathBuf> {
    app_dir(&["XDG_CONFIG_HOME", "APPDATA"], ".config").map(|dir| dir.join("config.json"))
}

pub fn credentials_file() -> Option<PathBuf> {
    app_dir(&["XDG_CONFIG_HOME", "APPDATA"], ".config").map(|dir| dir.join("credentials.json"))
}

pub fn session_file() -> Option<PathBuf> {
    app_dir(&["XDG_CONFIG_HOME", "APPDATA"], ".config").map(|dir| dir.join("session.json"))
}

pub fn cache_root() -> Option<PathBuf> {
    app_dir(&["XDG_CACHE_HOME", "LOCALAPPDATA"], ".cache")
}

pub fn data_root() -> Option<PathBuf> {
    app_dir(&["XDG_DATA_HOME", "APPDATA"], ".local/share")
}

pub fn osu_cache_dir() -> Option<PathBuf> {
    cache_root().map(|dir| dir.join("osu"))
}

pub fn maps_file() -> Option<PathBuf> {
    data_root().map(|dir| dir.join("maps.json"))
}

fn app_dir(vars: &[&str], home_fallback: &str) -> Option<PathBuf> {
    let base = vars
        .iter()
        .find_map(|var| env::var_os(var).filter(|value| !value.is_empty()))
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(home_fallback)))?;
    Some(base.join("lost-scores"))
}

pub fn resolve_osu_dir(arg: Option<&Path>) -> Result<PathBuf> {
    let config = config_file();
    let mut settings = config.as_deref().map(load_settings).unwrap_or_default();
    let chosen = match arg {
        Some(dir) => {
            ensure!(
                detect::is_osu_dir(dir),
                "{} is not an osu! folder: there is no Data/r inside",
                dir.display()
            );
            dir.to_path_buf()
        }
        None => {
            if let Some(saved) = settings
                .osu_dir
                .clone()
                .filter(|dir| detect::is_osu_dir(dir))
            {
                return Ok(saved);
            }
            match detect::find_osu_dirs(&detect::candidates(&detect::Places::current())).as_slice()
            {
                [one] => one.clone(),
                [] => bail!(
                    "osu! folder not found, pass --osu-dir <path> once and it will be remembered"
                ),
                many => bail!(
                    "several osu! folders found, pick one with --osu-dir: {}",
                    many.iter()
                        .map(|dir| dir.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            }
        }
    };
    if let Some(config) = &config
        && settings.osu_dir.as_deref() != Some(chosen.as_path())
    {
        settings.osu_dir = Some(chosen.clone());
        save_settings(config, &settings)?;
    }
    Ok(chosen)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{
        Settings, UserConfig, load_settings, parse_scores_to_show, player, read_user_config,
        save_settings, scores_to_show_typed, songs_dir,
    };

    #[test]
    fn out_of_range_scores_to_show_are_clamped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        for (stored, expected) in [(0, 1), (5000, 250), (65535, 250), (42, 42)] {
            fs::write(&path, format!(r#"{{"scores_to_show":{stored}}}"#)).unwrap();
            assert_eq!(load_settings(&path).scores_to_show, expected);
        }
    }

    #[test]
    fn the_catbox_answer_is_off_until_stored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        fs::write(&path, r#"{"scores_to_show":5}"#).unwrap();
        let mut settings = load_settings(&path);
        assert!(!settings.catbox_confirmed);
        settings.catbox_confirmed = true;
        save_settings(&path, &settings).unwrap();
        assert!(load_settings(&path).catbox_confirmed);
    }

    #[test]
    fn user_config_gives_player_and_songs_folder() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("osu!.cfg"), "Username = NotThisOne\r\n").unwrap();
        fs::write(
            dir.path().join("osu!.Bob.cfg"),
            "Username = Lemon\r\nPassword = hidden\r\nBeatmapDirectory = Maps\r\n",
        )
        .unwrap();
        let config = read_user_config(dir.path());
        assert_eq!(
            config,
            UserConfig {
                usernames: vec!["Lemon".into()],
                beatmap_directory: Some("Maps".into()),
            }
        );
        assert_eq!(player(&config).unwrap(), "Lemon");
        assert_eq!(
            songs_dir(dir.path(), &config).unwrap(),
            dir.path().join("Maps")
        );
    }

    #[test]
    fn songs_default_to_the_songs_folder() {
        let dir = tempfile::tempdir().unwrap();
        let config = read_user_config(dir.path());
        assert_eq!(
            songs_dir(dir.path(), &config).unwrap(),
            dir.path().join("Songs")
        );
    }

    #[test]
    fn missing_or_ambiguous_username_is_an_error() {
        assert!(player(&UserConfig::default()).is_err());
        let two = UserConfig {
            usernames: vec!["A".into(), "B".into()],
            beatmap_directory: None,
        };
        assert!(player(&two).is_err());
    }

    #[test]
    fn absolute_songs_folder_that_does_not_exist_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let config = UserConfig {
            usernames: vec![],
            beatmap_directory: Some(dir.path().join("gone").display().to_string()),
        };
        assert!(songs_dir(dir.path(), &config).is_err());
    }

    #[test]
    fn settings_survive_a_round_trip_and_default_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("config.json");
        assert_eq!(load_settings(&path), Settings::default());
        let settings = Settings {
            osu_dir: Some(dir.path().join("osu!")),
            player: Some("Lemon".into()),
            include_all: true,
            scores_to_show: 25,
            ..Settings::default()
        };
        save_settings(&path, &settings).unwrap();
        assert_eq!(load_settings(&path), settings);
    }

    #[test]
    fn old_config_files_load_with_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        fs::write(&path, r#"{"osu_dir":"/x"}"#).unwrap();
        let settings = load_settings(&path);
        assert_eq!(settings.osu_dir, Some("/x".into()));
        assert_eq!(settings.scores_to_show, 10);
        assert!(
            !settings.include_all && !settings.check_missing && !settings.show_lost_in_top_image
        );
    }

    #[test]
    fn typed_scores_to_show_snap_to_the_nearest_limit() {
        for (text, expected) in [
            ("251", Some(250)),
            ("1000", Some(250)),
            ("99999999999", Some(250)),
            ("0", Some(1)),
            ("-3", Some(1)),
            (" 25 ", Some(25)),
            ("abc", None),
            ("", None),
        ] {
            assert_eq!(scores_to_show_typed(text), expected, "{text:?}");
        }
    }

    #[test]
    fn scores_to_show_accepts_1_to_250_only() {
        for text in ["0", "251", "1000", "abc", ""] {
            assert_eq!(parse_scores_to_show(text), None, "{text:?}");
        }
        assert_eq!(parse_scores_to_show(" 25 "), Some(25));
        assert_eq!(parse_scores_to_show("250"), Some(250));
        assert_eq!(parse_scores_to_show("1"), Some(1));
    }
}
