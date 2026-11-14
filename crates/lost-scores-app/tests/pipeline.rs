mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::{Isolated, Replay, isolate_environment, write};
use lost_scores_app::online::Player;
use lost_scores_app::pipeline::{
    Api, Event, Source, Step, find, load_catalog, lost, potential, read, refresh_statuses,
};
use lost_scores_app::setup;
use lost_scores_app::timings::Timings;
use lost_scores_core::maps::{FetchedMap, MapCatalog, MapInfo, MapSource, MapStatus};
use lost_scores_core::missing::OsuFiles;
use osu_api::{ApiError, Score, ScoreBeatmap, ScoreBeatmapset, ScoreStatistics};
use tempfile::TempDir;

const SAMPLE: &[u8] = include_bytes!("fixtures/sample.osu");
const SAMPLE_MD5: &str = "0b577cbe9bc774600d20cfa17c0aa94c";
const SAMPLE_PATH: &str = "Songs/1 Sample/sample.osu";
const EARLY_TICKS: i64 = 638_355_968_000_000_000;
const NOW: i64 = 1_800_000_000;

#[derive(Default)]
struct Fake {
    calls: Vec<String>,
    top: Vec<Score>,
    fail_batch: bool,
    bare_player: bool,
}

impl MapSource for Fake {
    type Error = ApiError;

    fn batch(&mut self, ids: &[u32]) -> Result<Vec<FetchedMap>, ApiError> {
        self.calls.push(format!("batch {ids:?}"));
        if self.fail_batch {
            return Err(ApiError::Network("down".into()));
        }
        Ok(Vec::new())
    }

    fn lookup(&mut self, md5: &str) -> Result<Option<FetchedMap>, ApiError> {
        self.calls.push(format!("lookup {md5}"));
        Ok(Some(FetchedMap {
            checksum: Some(md5.to_owned()),
            info: MapInfo {
                beatmap_id: Some(11),
                status: MapStatus::Ranked,
                ranked_at: None,
                outdated: false,
                checked_at: None,
            },
        }))
    }
}

impl OsuFiles for Fake {
    type Error = ApiError;

    fn download(&mut self, beatmap_id: u32) -> Result<Vec<u8>, ApiError> {
        self.calls.push(format!("download {beatmap_id}"));
        Ok(Vec::new())
    }
}

impl Api for Fake {
    fn player(&mut self, name: &str) -> Result<Player, ApiError> {
        self.calls.push(format!("player {name}"));
        Ok(Player {
            id: 7,
            names: vec![name.to_owned()],
            pp: (!self.bare_player).then_some(1000.0),
            accuracy: (!self.bare_player).then_some(98.0),
            global_rank: Some(1),
        })
    }

    fn top(&mut self, user_id: u32) -> Result<Vec<Score>, ApiError> {
        self.calls.push(format!("top {user_id}"));
        Ok(self.top.clone())
    }
}

fn online_score(beatmap_id: u32, pp: f64) -> Score {
    Score {
        id: u64::from(beatmap_id),
        pp: Some(pp),
        accuracy: 0.98,
        mods: Vec::new(),
        score: 1_000_000,
        rank: "S".into(),
        created_at: "2024-09-07T21:44:00Z".into(),
        statistics: ScoreStatistics {
            count_300: 100,
            count_100: 2,
            count_50: 0,
            count_miss: 0,
        },
        beatmap: ScoreBeatmap {
            id: beatmap_id,
            version: "Expert".into(),
            checksum: None,
        },
        beatmapset: ScoreBeatmapset {
            id: 2,
            artist: "Artist".into(),
            title: "Title".into(),
            creator: "Mapper".into(),
        },
    }
}

struct OsuFolder {
    dir: TempDir,
    _environment: Isolated,
}

impl OsuFolder {
    fn path(&self) -> &Path {
        self.dir.path()
    }
}

fn osu_folder() -> OsuFolder {
    let environment = isolate_environment();
    let dir = tempfile::tempdir().unwrap();
    fill_osu_folder(dir.path());
    setup::resolve_osu_dir(Some(dir.path())).unwrap();
    OsuFolder {
        dir,
        _environment: environment,
    }
}

fn fill_osu_folder(root: &Path) {
    write(root, SAMPLE_PATH, SAMPLE);
    let best_pp = Replay {
        mode: 0,
        player: "Lemon",
        beatmap_md5: SAMPLE_MD5,
        counts: [7, 0, 0, 0],
        score: 500_000,
        max_combo: 8,
        mods: 0,
        ticks: EARLY_TICKS,
    };
    let best_score = Replay {
        counts: [5, 2, 0, 0],
        score: 900_000,
        ticks: EARLY_TICKS + 1,
        ..best_pp
    };
    write(root, "Data/r/a.osr", &best_pp.bytes());
    write(root, "Data/r/b.osr", &best_score.bytes());
}

fn source(dir: &Path, maps: Option<PathBuf>) -> Source {
    Source {
        osu_dir: Some(dir.to_path_buf()),
        player: Some("Lemon".into()),
        songs_dir: None,
        check_missing: false,
        maps,
    }
}

fn collect(events: &mut Vec<Event>) -> impl FnMut(Event) + '_ {
    move |event| events.push(event)
}

fn log_lines(events: &[Event]) -> Vec<&str> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::Log(line) => Some(line.as_str()),
            Event::Progress { .. } => None,
        })
        .collect()
}

#[test]
fn read_reports_the_lines_the_cli_printed() {
    let osu = osu_folder();
    let dir = osu.path();
    let mut fake = Fake::default();
    let mut events = Vec::new();
    let read = read(
        &source(dir, None),
        Some(&mut fake),
        "run lost-scores login first",
        &mut Timings::new(),
        &mut collect(&mut events),
    )
    .unwrap();
    let lines = log_lines(&events);
    assert!(lines[0].starts_with("osu!:"), "{lines:?}");
    assert!(lines[1].starts_with("player: Lemon"), "{lines:?}");
    assert!(lines[2].starts_with("2 replays read in"), "{lines:?}");
    assert_eq!(read.scan.replays.len(), 2);
    assert_eq!(read.pp_failed, 0);
    assert_eq!(read.player.unwrap().id, 7);
}

#[test]
fn read_without_the_api_still_reports_the_player() {
    let osu = osu_folder();
    let dir = osu.path();
    let mut events = Vec::new();
    let read = read(
        &source(dir, None),
        None,
        "run lost-scores login first",
        &mut Timings::new(),
        &mut collect(&mut events),
    )
    .unwrap();
    assert!(read.player.is_none());
    assert!(log_lines(&events)[1].starts_with("player: Lemon"));
}

#[test]
fn load_catalog_reports_progress_for_each_request() {
    let osu = osu_folder();
    let dir = osu.path();
    let work = tempfile::tempdir().unwrap();
    let maps = work.path().join("maps.json");
    let mut fake = Fake::default();
    let mut timings = Timings::new();
    let mut events = Vec::new();
    let scan = read(
        &source(dir, Some(maps.clone())),
        None,
        "",
        &mut timings,
        &mut |_| {},
    )
    .unwrap()
    .scan;
    let catalog = load_catalog(
        Some(&maps),
        &scan,
        Some(&mut fake),
        "",
        NOW,
        &mut timings,
        &mut collect(&mut events),
    )
    .unwrap();
    let last = events
        .iter()
        .filter_map(|event| match event {
            Event::Progress {
                step: Step::Catalog,
                done,
                total,
            } => Some((*done, *total)),
            _ => None,
        })
        .next_back()
        .unwrap();
    assert_eq!(last, (1, 1));
    assert_eq!(catalog.get(SAMPLE_MD5).unwrap().status, MapStatus::Ranked);
    assert!(maps.is_file());
    assert!(
        log_lines(&events)
            .iter()
            .any(|line| line.starts_with("asking the osu! API about 1 maps"))
    );
}

#[test]
fn load_catalog_without_the_api_says_how_to_get_statuses() {
    let osu = osu_folder();
    let dir = osu.path();
    let work = tempfile::tempdir().unwrap();
    let maps = work.path().join("maps.json");
    let mut timings = Timings::new();
    let scan = read(
        &source(dir, Some(maps.clone())),
        None,
        "",
        &mut timings,
        &mut |_| {},
    )
    .unwrap()
    .scan;
    let mut events = Vec::new();
    let catalog = load_catalog(
        Some(&maps),
        &scan,
        None,
        "run without --offline",
        NOW,
        &mut timings,
        &mut collect(&mut events),
    )
    .unwrap();
    assert!(catalog.is_empty());
    assert_eq!(
        log_lines(&events),
        ["1 maps have no status yet; to fetch them from the osu! API, run without --offline"]
    );
}

#[test]
fn potential_merges_the_fake_online_top() {
    let osu = osu_folder();
    let dir = osu.path();
    let work = tempfile::tempdir().unwrap();
    let maps = work.path().join("maps.json");
    let mut fake = Fake {
        top: vec![online_score(100, 500.0), online_score(101, 400.0)],
        ..Fake::default()
    };
    let mut timings = Timings::new();
    let read = read(
        &source(dir, Some(maps.clone())),
        Some(&mut fake),
        "",
        &mut timings,
        &mut |_| {},
    )
    .unwrap();
    let catalog = load_catalog(
        Some(&maps),
        &read.scan,
        Some(&mut fake),
        "",
        NOW,
        &mut timings,
        &mut |_| {},
    )
    .unwrap();
    let scores = find(&read.scan, &catalog, &mut timings);
    let found = lost(&scores, &catalog);
    assert_eq!((found.total, found.ranked, found.rows.len()), (1, 1, 1));
    let mut events = Vec::new();
    let player = read.player.unwrap();
    let result = potential(
        &mut fake,
        &player,
        &scores,
        &catalog,
        &mut timings,
        &mut collect(&mut events),
    )
    .unwrap();
    assert_eq!(result.rows.len(), 3);
    assert_eq!(
        result.rows.iter().map(|row| row.lost).collect::<Vec<_>>(),
        [false, false, true]
    );
    assert_eq!(result.lost_in_top, 1);
    assert_eq!(result.online.len(), 2);
    assert_eq!(result.current_pp, 1000.0);
    assert!(result.potential_pp > 0.0);
    assert_eq!(result.global_rank, Some(1));
    assert!(fake.calls.contains(&"top 7".to_owned()));
    assert_eq!(timings.count("lost scores"), 1);
}

#[test]
fn refresh_is_a_separate_step() {
    let osu = osu_folder();
    let dir = osu.path();
    let work = tempfile::tempdir().unwrap();
    let maps = work.path().join("maps.json");
    let mut stale = MapCatalog::default();
    stale.insert(
        SAMPLE_MD5.to_owned(),
        MapInfo {
            beatmap_id: Some(5),
            status: MapStatus::Pending,
            ranked_at: None,
            outdated: false,
            checked_at: None,
        },
    );
    stale.save(&maps).unwrap();
    let mut fake = Fake::default();
    let mut timings = Timings::new();
    let scan = read(
        &source(dir, Some(maps.clone())),
        None,
        "",
        &mut timings,
        &mut |_| {},
    )
    .unwrap()
    .scan;
    let mut catalog = load_catalog(
        Some(&maps),
        &scan,
        Some(&mut fake),
        "",
        NOW,
        &mut timings,
        &mut |_| {},
    )
    .unwrap();
    assert!(fake.calls.is_empty(), "{:?}", fake.calls);
    let mut events = Vec::new();
    let changed = refresh_statuses(
        Some(&maps),
        &mut catalog,
        &mut fake,
        NOW,
        &mut timings,
        &mut collect(&mut events),
    )
    .unwrap();
    assert_eq!(fake.calls, ["batch [5]"]);
    assert_eq!(changed, 0);
    assert!(events.iter().any(|event| matches!(
        event,
        Event::Progress {
            step: Step::Refresh,
            done: 1,
            total: 1
        }
    )));
    assert!(
        log_lines(&events)
            .iter()
            .any(|line| line.starts_with("asked again about 1 maps"))
    );
}

#[test]
fn a_failed_status_refresh_warns_and_carries_on() {
    let _osu = osu_folder();
    let work = tempfile::tempdir().unwrap();
    let maps = work.path().join("maps.json");
    let mut catalog = MapCatalog::default();
    catalog.insert(
        SAMPLE_MD5.to_owned(),
        MapInfo {
            beatmap_id: Some(5),
            status: MapStatus::Pending,
            ranked_at: None,
            outdated: false,
            checked_at: None,
        },
    );
    let mut fake = Fake {
        fail_batch: true,
        ..Fake::default()
    };
    let mut events = Vec::new();
    let changed = refresh_statuses(
        Some(&maps),
        &mut catalog,
        &mut fake,
        NOW,
        &mut Timings::new(),
        &mut collect(&mut events),
    )
    .unwrap();
    assert_eq!(changed, 0);
    assert_eq!(catalog.get(SAMPLE_MD5).unwrap().status, MapStatus::Pending);
    assert!(
        log_lines(&events)
            .iter()
            .any(|line| line.starts_with("Could not refresh map statuses: ")),
        "{events:?}"
    );
}

#[test]
fn check_leaves_the_catalog_file_alone_when_nothing_changed() {
    let _osu = osu_folder();
    let cache = setup::osu_cache_dir().unwrap();
    write(&cache, &format!("{SAMPLE_MD5}.osu"), SAMPLE);
    let root = tempfile::tempdir().unwrap();
    let osu = root.path().join("osu");
    for (name, counts, score, ticks) in [
        ("a.osr", [7, 0, 0, 0], 500_000, EARLY_TICKS),
        ("b.osr", [5, 2, 0, 0], 900_000, EARLY_TICKS + 1),
    ] {
        let replay = Replay {
            mode: 0,
            player: "Lemon",
            beatmap_md5: SAMPLE_MD5,
            counts,
            score,
            max_combo: 8,
            mods: 0,
            ticks,
        };
        write(&osu, &format!("Data/r/{name}"), &replay.bytes());
    }
    write(&osu, "Songs/.keep", b"");
    let maps = root.path().join("maps.json");
    let original = format!(
        "{{\n  \"version\": 1,\n  \"maps\": {{\"{SAMPLE_MD5}\": {{\"beatmap_id\": 5, \"status\": \"ranked\", \"ranked_at\": null}}}}\n}}\n"
    );
    fs::write(&maps, &original).unwrap();
    let mut fake = Fake::default();
    let mut source = source(&osu, Some(maps.clone()));
    source.check_missing = true;
    let read = read(
        &source,
        Some(&mut fake),
        "",
        &mut Timings::new(),
        &mut |_| {},
    )
    .unwrap();
    assert!(
        read.scan
            .replays
            .iter()
            .all(|replay| replay.beatmap.is_some())
    );
    assert!(!fake.calls.iter().any(|call| call.starts_with("lookup")));
    assert_eq!(fs::read_to_string(&maps).unwrap(), original);
}

#[test]
fn check_without_the_api_gives_a_message_the_window_can_show_too() {
    let osu = osu_folder();
    let dir = osu.path();
    let mut source = source(dir, None);
    source.check_missing = true;
    let mut events = Vec::new();
    read(
        &source,
        None,
        "add your own API keys",
        &mut Timings::new(),
        &mut collect(&mut events),
    )
    .unwrap();
    let line = log_lines(&events)
        .into_iter()
        .find(|line| line.contains("needs the osu! API"))
        .unwrap_or_else(|| panic!("{events:?}"));
    assert!(!line.contains("--"), "{line}");
    assert!(line.contains("add your own API keys"), "{line}");
}

#[test]
fn potential_refuses_a_player_the_api_gave_no_statistics_for() {
    let osu = osu_folder();
    let dir = osu.path();
    let mut fake = Fake {
        bare_player: true,
        ..Fake::default()
    };
    let mut timings = Timings::new();
    let read = read(
        &source(dir, None),
        Some(&mut fake),
        "",
        &mut timings,
        &mut |_| {},
    )
    .unwrap();
    let player = read.player.unwrap();
    assert_eq!((player.pp, player.accuracy), (None, None));
    let scores = find(&read.scan, &MapCatalog::default(), &mut timings);
    let error = potential(
        &mut fake,
        &player,
        &scores,
        &MapCatalog::default(),
        &mut timings,
        &mut |_| {},
    )
    .expect_err("potential must not invent a zero");
    assert!(error.to_string().contains("no statistics"), "{error}");
}

#[test]
fn read_maps_scan_progress_to_the_songs_replays_and_pp_steps() {
    let osu = osu_folder();
    let dir = osu.path();
    let mut events = Vec::new();
    read(
        &source(dir, None),
        None,
        "",
        &mut Timings::new(),
        &mut collect(&mut events),
    )
    .unwrap();
    let last = |wanted: Step| {
        events
            .iter()
            .filter_map(|event| match event {
                Event::Progress { step, done, total } if *step == wanted => Some((*done, *total)),
                _ => None,
            })
            .next_back()
    };
    assert_eq!(last(Step::Songs), Some((1, 1)));
    assert_eq!(last(Step::Replays), Some((2, 2)));
    assert_eq!(last(Step::Pp), Some((2, 2)));
    assert_eq!(last(Step::MissingMaps), None);
}
