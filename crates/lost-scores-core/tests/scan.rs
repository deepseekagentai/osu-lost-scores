mod common;

use std::fs;

use common::{Replay, TICKS, write};
use lost_scores_core::{Scan, scan};
use tempfile::TempDir;

const RELAX: u32 = 128;
const BEATMAP_X: &[u8] = b"osu file format v14\r\n\r\n[Metadata]\r\nTitle:B\r\nArtist:A\r\nCreator:m\r\nVersion:x\r\nBeatmapID:11\r\nBeatmapSetID:1\r\n";
const BEATMAP_X_MD5: &str = "de7522578ee5be024c63a06eba14e2f6";
const BEATMAP_X_PATH: &str = "Songs/1 A - B/A - B (m) [x].osu";
const BEATMAP_Y: &[u8] = b"osu file format v14\r\n\r\n[Metadata]\r\nTitle:D\r\nArtist:C\r\nCreator:m\r\nVersion:y\r\nBeatmapID:22\r\nBeatmapSetID:2\r\n";
const MISSING_MD5: &str = "ffffffffffffffffffffffffffffffff";

fn replay(mode: u8, player: &str, beatmap_md5: &str, mods: u32, ticks: i64) -> Vec<u8> {
    Replay {
        mode,
        player,
        beatmap_md5,
        counts: [300, 20, 1, 2],
        max_combo: 500,
        mods,
        ticks,
    }
    .bytes()
}

fn osu_folder() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, BEATMAP_X_PATH, BEATMAP_X);
    write(root, "Songs/2 C - D/C - D (m) [y].osu", BEATMAP_Y);
    write(root, "Songs/2 C - D/audio.mp3", b"ID3");
    write(
        root,
        "Data/r/a.osr",
        &replay(0, "Lemon", BEATMAP_X_MD5, 0, TICKS + 2),
    );
    write(
        root,
        "Data/r/b.osr",
        &replay(0, " lemon ", MISSING_MD5, 0, TICKS + 1),
    );
    write(
        root,
        "Data/r/c.osr",
        &replay(0, "Someone", BEATMAP_X_MD5, 0, TICKS),
    );
    write(
        root,
        "Data/r/d.osr",
        &replay(1, "Lemon", BEATMAP_X_MD5, 0, TICKS),
    );
    write(
        root,
        "Data/r/e.osr",
        &replay(0, "Lemon", BEATMAP_X_MD5, RELAX, TICKS),
    );
    write(root, "Data/r/f.osr", b"not a replay");
    write(root, "Data/r/notes.txt", b"");
    dir
}

fn beatmap_paths(report: &Scan) -> Vec<Option<&str>> {
    report
        .replays
        .iter()
        .map(|replay| replay.beatmap.as_ref().map(|b| b.path.to_str().unwrap()))
        .collect()
}

#[test]
fn selects_the_players_standard_replays_in_time_order() {
    let dir = osu_folder();
    let report = scan(dir.path(), &dir.path().join("Songs"), &["LEMON".into()]).unwrap();
    let files: Vec<_> = report
        .replays
        .iter()
        .map(|r| r.file_name.as_str())
        .collect();
    assert_eq!(files, ["b.osr", "a.osr"]);
    assert_eq!(beatmap_paths(&report), [None, Some(BEATMAP_X_PATH)]);
}

#[test]
fn counts_every_skipped_replay() {
    let dir = osu_folder();
    let report = scan(dir.path(), &dir.path().join("Songs"), &["Lemon".into()]).unwrap();
    let unreadable: Vec<_> = report
        .unreadable_replays
        .iter()
        .map(|r| r.file_name.as_str())
        .collect();
    assert_eq!(unreadable, ["f.osr"]);
    assert_eq!(
        (
            report.replay_files,
            report.other_mode,
            report.other_player,
            report.unranked_mods
        ),
        (6, 1, 1, 1)
    );
    assert_eq!((report.beatmap_files, report.unreadable_beatmaps), (2, 0));
}

#[test]
fn found_beatmap_carries_its_metadata() {
    let dir = osu_folder();
    let report = scan(dir.path(), &dir.path().join("Songs"), &["Lemon".into()]).unwrap();
    let beatmap = report
        .replays
        .iter()
        .find_map(|r| r.beatmap.as_ref())
        .unwrap();
    assert_eq!(
        (beatmap.metadata.title.as_str(), beatmap.metadata.beatmap_id),
        ("B", Some(11))
    );
}

#[test]
fn duplicate_beatmaps_resolve_to_the_smallest_path() {
    for (copy, expected) in [
        ("Songs/0 copy/x.osu", "Songs/0 copy/x.osu"),
        ("Songs/9 copy/x.osu", BEATMAP_X_PATH),
    ] {
        let dir = osu_folder();
        write(dir.path(), copy, BEATMAP_X);
        let report = scan(dir.path(), &dir.path().join("Songs"), &["Lemon".into()]).unwrap();
        assert_eq!(beatmap_paths(&report), [None, Some(expected)], "{copy}");
        assert_eq!(report.beatmap_files, 3);
    }
}

#[test]
fn missing_game_folders_are_errors() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "Data/r/a.osr",
        &replay(0, "Lemon", MISSING_MD5, 0, TICKS),
    );
    assert_eq!(
        scan(dir.path(), &dir.path().join("Songs"), &["Lemon".into()])
            .unwrap_err()
            .path,
        dir.path().join("Songs")
    );
    write(dir.path(), BEATMAP_X_PATH, BEATMAP_X);
    fs::remove_dir_all(dir.path().join("Data")).unwrap();
    assert_eq!(
        scan(dir.path(), &dir.path().join("Songs"), &["Lemon".into()])
            .unwrap_err()
            .path,
        dir.path().join("Data").join("r")
    );
}

#[test]
fn replays_under_a_previous_name_are_selected_too() {
    let dir = osu_folder();
    write(
        dir.path(),
        "Data/r/old.osr",
        &replay(0, "Old Name", BEATMAP_X_MD5, 0, TICKS + 3),
    );
    let report = scan(
        dir.path(),
        &dir.path().join("Songs"),
        &["Lemon".into(), "old name".into()],
    )
    .unwrap();
    let files: Vec<_> = report
        .replays
        .iter()
        .map(|r| r.file_name.as_str())
        .collect();
    assert_eq!(files, ["b.osr", "a.osr", "old.osr"]);
}
