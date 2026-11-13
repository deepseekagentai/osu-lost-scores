use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};

use crate::detect;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct UserConfig {
    pub usernames: Vec<String>,
    pub beatmap_directory: Option<String>,
}

#[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    pub osu_dir: Option<PathBuf>,
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
    fs::read(path)
        .ok()
        .and_then(|data| serde_json::from_slice(&data).ok())
        .unwrap_or_default()
}

pub fn save_settings(path: &Path, settings: &Settings) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("cannot create {}", parent.display()))?;
    }
    fs::write(path, serde_json::to_vec_pretty(settings)?)
        .with_context(|| format!("cannot write {}", path.display()))
}

pub fn config_file() -> Option<PathBuf> {
    app_dir(&["XDG_CONFIG_HOME", "APPDATA"], ".config").map(|dir| dir.join("config.json"))
}

pub fn maps_file() -> Option<PathBuf> {
    app_dir(&["XDG_DATA_HOME", "APPDATA"], ".local/share").map(|dir| dir.join("maps.json"))
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
        Settings, UserConfig, load_settings, player, read_user_config, save_settings, songs_dir,
    };

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
        };
        save_settings(&path, &settings).unwrap();
        assert_eq!(load_settings(&path), settings);
    }
}
