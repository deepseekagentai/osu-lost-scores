mod common;

use std::fs;

use common::{Replay, TICKS, write};
use lost_scores_core::{pp, scan};

const SAMPLE: &[u8] = include_bytes!("fixtures/sample.osu");
const SAMPLE_MD5: &str = "0b577cbe9bc774600d20cfa17c0aa94c";
const SAMPLE_PATH: &str = "Songs/1 Sample/sample.osu";
const MISSING_MD5: &str = "ffffffffffffffffffffffffffffffff";
const HD_DT: u32 = 8 | 64;

fn osu_folder() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let played = Replay {
        mode: 0,
        player: "Lemon",
        beatmap_md5: SAMPLE_MD5,
        counts: [6, 1, 0, 0],
        max_combo: 8,
        mods: HD_DT,
        ticks: TICKS,
    };
    write(dir.path(), SAMPLE_PATH, SAMPLE);
    write(dir.path(), "Data/r/a.osr", &played.bytes());
    let missing = Replay {
        beatmap_md5: MISSING_MD5,
        ticks: TICKS + 1,
        ..played
    };
    write(dir.path(), "Data/r/b.osr", &missing.bytes());
    dir
}

#[test]
fn fills_pp_for_replays_with_a_local_beatmap() {
    let dir = osu_folder();
    let mut report = scan(dir.path(), &dir.path().join("Songs"), "Lemon").unwrap();
    let failed = pp::fill(dir.path(), &mut report.replays);
    let pps: Vec<_> = report
        .replays
        .iter()
        .map(|r| (r.file_name.as_str(), r.pp))
        .collect();
    assert_eq!(pps, [("a.osr", Some(4.8957636184902515)), ("b.osr", None)]);
    assert_eq!(failed, 0);
}

#[test]
fn beatmap_that_cannot_be_read_counts_as_a_failure() {
    let dir = osu_folder();
    let mut report = scan(dir.path(), &dir.path().join("Songs"), "Lemon").unwrap();
    fs::remove_file(dir.path().join(SAMPLE_PATH)).unwrap();
    let failed = pp::fill(dir.path(), &mut report.replays);
    assert_eq!(failed, 1);
    assert!(report.replays.iter().all(|r| r.pp.is_none()));
}
