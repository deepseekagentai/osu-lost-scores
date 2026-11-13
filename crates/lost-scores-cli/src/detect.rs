use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const OSU_COMMAND_KEY: &str = r"[software\\classes\\osu\\shell\\open\\command]";

pub fn is_osu_dir(path: &Path) -> bool {
    path.join("Data").join("r").is_dir()
}

pub fn find_osu_dirs(patterns: &[PathBuf]) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = patterns
        .iter()
        .flat_map(|pattern| expand(pattern))
        .filter(|dir| is_osu_dir(dir))
        .collect();
    found.sort();
    found.dedup();
    found
}

fn expand(pattern: &Path) -> Vec<PathBuf> {
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
    current
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
    for prefix in &prefixes {
        found.extend(INSIDE_PREFIX.map(|inner| prefix.join(inner)));
    }
    found.extend(
        prefixes
            .iter()
            .flat_map(|prefix| expand(prefix))
            .filter_map(|prefix| folder_from_wine_registry(&prefix)),
    );
    if cfg!(windows) {
        found.extend(folder_from_windows_registry());
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

pub fn exe_from_command(command: &str) -> Option<&str> {
    let command = command.trim();
    let exe = match command.strip_prefix('"') {
        Some(rest) => &rest[..rest.find('"')?],
        None => &command[..command.to_ascii_lowercase().find(".exe")? + 4],
    };
    (!exe.is_empty()).then_some(exe)
}

pub fn command_from_reg_query(output: &str) -> Option<String> {
    output.lines().find_map(|line| {
        let (_, value) = line
            .split_once("REG_SZ")
            .or_else(|| line.split_once("REG_EXPAND_SZ"))?;
        let value = value.trim();
        (!value.is_empty()).then(|| value.to_owned())
    })
}

pub fn command_from_wine_registry(contents: &str) -> Option<String> {
    let mut in_key = false;
    for line in contents.lines().map(str::trim) {
        if line.starts_with('[') {
            in_key = line.to_ascii_lowercase().starts_with(OSU_COMMAND_KEY);
            continue;
        }
        if in_key && let Some(value) = line.strip_prefix("@=") {
            return Some(unescape(value.strip_prefix('"')?.strip_suffix('"')?));
        }
    }
    None
}

fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => out.extend(chars.next()),
            _ => out.push(c),
        }
    }
    out
}

pub fn windows_path_in_prefix(prefix: &Path, windows_path: &str) -> Option<PathBuf> {
    let (drive, rest) = windows_path.split_once(':')?;
    let mut letters = drive.chars();
    let letter = letters
        .next()
        .filter(|c| c.is_ascii_alphabetic() && letters.next().is_none())?
        .to_ascii_lowercase();
    let base = if letter == 'c' {
        prefix.join("drive_c")
    } else {
        prefix.join("dosdevices").join(format!("{letter}:"))
    };
    Some(
        rest.split('\\')
            .filter(|part| !part.is_empty())
            .fold(base, |path, part| path.join(part)),
    )
}

fn folder_from_wine_registry(prefix: &Path) -> Option<PathBuf> {
    ["user.reg", "system.reg"].into_iter().find_map(|file| {
        let contents = fs::read(prefix.join(file)).ok()?;
        let command = command_from_wine_registry(&String::from_utf8_lossy(&contents))?;
        let exe = windows_path_in_prefix(prefix, exe_from_command(&command)?)?;
        exe.parent().map(Path::to_path_buf)
    })
}

fn folder_from_windows_registry() -> Option<PathBuf> {
    let output = Command::new("reg")
        .args(["query", r"HKCR\osu\shell\open\command", "/ve"])
        .output()
        .ok()?;
    let command = command_from_reg_query(&String::from_utf8_lossy(&output.stdout))?;
    Path::new(exe_from_command(&command)?)
        .parent()
        .map(Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::{
        Places, candidates, command_from_reg_query, command_from_wine_registry, exe_from_command,
        find_osu_dirs, is_osu_dir, windows_path_in_prefix,
    };

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

    const WINE_USER_REG: &str = "WINE REGISTRY Version 2\n;; All keys relative to \\\\User\\\\S-1-5-21-0-0-0-1000\n\n[Software\\\\Classes\\\\osu] 1700000000\n@=\"URL:osu! Protocol\"\n\n[Software\\\\Classes\\\\osu\\\\shell\\\\open\\\\command] 1700000000\n#time=1da0000000000000\n@=\"\\\"C:\\\\Games\\\\osu!\\\\osu!.exe\\\" \\\"%1\\\"\"\n\n[Software\\\\Wine] 1700000000\n";

    #[test]
    fn exe_is_taken_from_the_open_command() {
        for (command, expected) in [
            (
                r#""C:\Games\osu!\osu!.exe" "%1""#,
                Some(r"C:\Games\osu!\osu!.exe"),
            ),
            (r#"D:\osu!\osu!.exe "%1""#, Some(r"D:\osu!\osu!.exe")),
            ("", None),
        ] {
            assert_eq!(exe_from_command(command), expected, "{command}");
        }
    }

    #[test]
    fn reg_query_output_is_read_in_any_language() {
        for (output, expected) in [
            (
                "\r\nHKEY_CLASSES_ROOT\\osu\\shell\\open\\command\r\n    (Default)    REG_SZ    \"C:\\Games\\osu!\\osu!.exe\" \"%1\"\r\n\r\n",
                r#""C:\Games\osu!\osu!.exe" "%1""#,
            ),
            (
                "\r\nHKEY_CLASSES_ROOT\\osu\\shell\\open\\command\r\n    (По умолчанию)    REG_SZ    \"D:\\osu!\\osu!.exe\" \"%1\"\r\n",
                r#""D:\osu!\osu!.exe" "%1""#,
            ),
        ] {
            assert_eq!(command_from_reg_query(output).as_deref(), Some(expected));
        }
        assert_eq!(command_from_reg_query("ERROR: not found"), None);
    }

    #[test]
    fn wine_registry_value_is_unescaped() {
        assert_eq!(
            command_from_wine_registry(WINE_USER_REG).as_deref(),
            Some(r#""C:\Games\osu!\osu!.exe" "%1""#)
        );
        assert_eq!(command_from_wine_registry("[Software\\\\Wine] 1\n"), None);
    }

    #[cfg(unix)]
    #[test]
    fn windows_paths_map_into_the_prefix() {
        let prefix = Path::new("/p");
        assert_eq!(
            windows_path_in_prefix(prefix, r"C:\Games\osu!\osu!.exe"),
            Some(Path::new("/p/drive_c/Games/osu!/osu!.exe").to_path_buf())
        );
        assert_eq!(
            windows_path_in_prefix(prefix, r"z:\home\me\osu!\osu!.exe"),
            Some(Path::new("/p/dosdevices/z:/home/me/osu!/osu!.exe").to_path_buf())
        );
        assert_eq!(windows_path_in_prefix(prefix, "osu!.exe"), None);
    }

    #[test]
    fn custom_folder_is_found_through_the_wine_registry() {
        let root = tempfile::tempdir().unwrap();
        let prefix = root.path().join("prefix");
        let custom = prefix.join("drive_c/Games/osu!");
        osu_folder(&custom);
        fs::write(prefix.join("user.reg"), WINE_USER_REG).unwrap();
        let places = Places {
            wine_prefix: Some(prefix),
            ..Places::default()
        };
        assert!(find_osu_dirs(&candidates(&places)).contains(&custom));
    }
}
