use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn lost_scores(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_lost-scores"))
        .args(args)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_CACHE_HOME", home.join("cache"))
        .env_remove("LOST_SCORES_CLIENT_ID")
        .env_remove("LOST_SCORES_CLIENT_SECRET")
        .output()
        .unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn osu_folder(home: &Path) -> PathBuf {
    let osu = home.join("osu!");
    fs::create_dir_all(osu.join("Data").join("r")).unwrap();
    fs::create_dir_all(osu.join("Songs")).unwrap();
    osu
}

fn save_keys(home: &Path) {
    let dir = home.join("config").join("lost-scores");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("credentials.json"),
        r#"{"client_id":"1","client_secret":"s"}"#,
    )
    .unwrap();
}

fn potential(home: &Path, extra: &[&str]) -> Output {
    let osu = osu_folder(home);
    let osu = osu.to_str().unwrap();
    let mut args = vec!["potential", "--osu-dir", osu, "--player", "p"];
    args.extend(extra);
    lost_scores(home, &args)
}

#[test]
fn help_says_what_the_bare_command_does_and_shows_examples() {
    let home = tempfile::tempdir().unwrap();
    let help = text(&lost_scores(home.path(), &["--help"]).stdout);
    assert!(help.contains("Without a command, lists your lost plays"));
    assert!(help.contains("lost-scores potential"));
}

#[test]
fn help_names_the_files_it_keeps() {
    let home = tempfile::tempdir().unwrap();
    let help = text(&lost_scores(home.path(), &["--help"]).stdout);
    let config = home.path().join("config").join("lost-scores");
    for file in [
        config.join("config.json"),
        config.join("credentials.json"),
        home.path()
            .join("data")
            .join("lost-scores")
            .join("maps.json"),
    ] {
        let file = file.display().to_string();
        assert!(help.contains(&file), "{file} is not in the help");
    }
}

#[test]
fn login_help_lists_no_scan_options() {
    let home = tempfile::tempdir().unwrap();
    let help = text(&lost_scores(home.path(), &["login", "--help"]).stdout);
    assert!(!help.contains("--osu-dir"));
    assert!(!help.contains("--check-missing"));
}

#[test]
fn potential_offline_with_saved_keys_says_to_drop_offline() {
    let home = tempfile::tempdir().unwrap();
    save_keys(home.path());
    let output = potential(home.path(), &["--offline"]);
    assert!(!output.status.success());
    assert!(text(&output.stderr).contains("run without --offline"));
}

#[test]
fn potential_without_keys_says_to_log_in() {
    let home = tempfile::tempdir().unwrap();
    let output = potential(home.path(), &[]);
    assert!(!output.status.success());
    assert!(text(&output.stderr).contains("run `lost-scores login` first"));
}

#[test]
fn a_folder_without_replays_is_not_taken_for_osu() {
    let home = tempfile::tempdir().unwrap();
    let empty = home.path().join("empty");
    fs::create_dir(&empty).unwrap();
    let output = lost_scores(
        home.path(),
        &["--offline", "--osu-dir", empty.to_str().unwrap()],
    );
    assert!(!output.status.success());
    assert!(text(&output.stderr).contains("is not an osu! folder"));
}

#[test]
fn an_empty_osu_folder_lists_no_lost_plays() {
    let home = tempfile::tempdir().unwrap();
    let osu = osu_folder(home.path());
    let output = lost_scores(
        home.path(),
        &[
            "--offline",
            "--timings",
            "--osu-dir",
            osu.to_str().unwrap(),
            "--player",
            "p",
        ],
    );
    assert!(output.status.success());
    let log = text(&output.stderr);
    assert!(log.contains("0 replays read"));
    assert!(log.contains("total"));
}

fn check_missing_offline(home: &Path) -> String {
    let osu = osu_folder(home);
    let output = lost_scores(
        home,
        &[
            "--offline",
            "--check-missing",
            "--osu-dir",
            osu.to_str().unwrap(),
            "--player",
            "p",
        ],
    );
    assert!(output.status.success());
    text(&output.stderr)
}

#[test]
fn check_offline_with_saved_keys_says_to_drop_offline_not_to_log_in() {
    let home = tempfile::tempdir().unwrap();
    save_keys(home.path());
    let log = check_missing_offline(home.path());
    assert!(
        log.contains("needs the osu! API: run without --offline"),
        "{log}"
    );
    assert!(!log.contains("login"), "{log}");
}

#[test]
fn check_offline_without_keys_says_to_log_in() {
    let home = tempfile::tempdir().unwrap();
    let log = check_missing_offline(home.path());
    assert!(
        log.contains("needs the osu! API: run `lost-scores login` first"),
        "{log}"
    );
}
