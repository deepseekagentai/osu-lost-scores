mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::{Isolated, Replay, isolate_environment, write};
use lost_scores_app::history;
use lost_scores_app::online::Player;
use lost_scores_app::pipeline::{
    Api, Event, Lost, PlayerLookup, Source, Step, fill_set_ids, find, load_catalog, lost,
    potential, read, refresh_statuses, saved, unix_now,
};
use lost_scores_app::rows::{Hits, LostRow};
use lost_scores_app::setup;
use lost_scores_app::timings::Timings;
use lost_scores_core::grade::Grade;
use lost_scores_core::maps::{FetchedMap, MapCatalog, MapInfo, MapSource, MapStatus};
use lost_scores_core::missing::OsuFiles;
use osu_api::{ApiError, Score, ScoreBeatmap, ScoreBeatmapset, ScoreStatistics};
use tempfile::TempDir;

const SAMPLE: &[u8] = include_bytes!("fixtures/sample.osu");
const SAMPLE_MD5: &str = "0b577cbe9bc774600d20cfa17c0aa94c";
const UNCACHED_MD5: &str = "ffffffffffffffffffffffffffffffff";
const SAMPLE_PATH: &str = "Songs/1 Sample/sample.osu";
const EARLY_TICKS: i64 = 638_355_968_000_000_000;
const NOW: i64 = 1_800_000_000;

#[derive(Default)]
struct Fake {
    calls: Vec<String>,
    top: Vec<Score>,
    fail_batch: bool,
    bare_player: bool,
    lookup_failure: Option<u16>,
    shared: bool,
    expired: bool,
    sets: Vec<(u32, u32)>,
}

impl MapSource for Fake {
    type Error = ApiError;

    fn batch(&mut self, ids: &[u32]) -> Result<Vec<FetchedMap>, ApiError> {
        self.calls.push(format!("batch {ids:?}"));
        if self.expired {
            return Err(ApiError::LoginRequired);
        }
        if self.fail_batch {
            return Err(ApiError::Network("down".into()));
        }
        Ok(self
            .sets
            .iter()
            .filter(|(id, _)| ids.contains(id))
            .map(|&(id, set)| FetchedMap {
                checksum: None,
                info: MapInfo {
                    beatmap_id: Some(id),
                    beatmapset_id: Some(set),
                    status: MapStatus::Ranked,
                    ranked_at: None,
                    outdated: false,
                    checked_at: None,
                },
            })
            .collect())
    }

    fn lookup(&mut self, md5: &str) -> Result<Option<FetchedMap>, ApiError> {
        self.calls.push(format!("lookup {md5}"));
        if self.expired {
            return Err(ApiError::LoginRequired);
        }
        Ok(Some(FetchedMap {
            checksum: Some(md5.to_owned()),
            info: MapInfo {
                beatmap_id: Some(11),
                beatmapset_id: None,
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
        if self.expired {
            return Err(ApiError::LoginRequired);
        }
        Ok(Vec::new())
    }
}

impl Api for Fake {
    fn shared(&self) -> bool {
        self.shared
    }

    fn player(&mut self, name: &str) -> Result<Player, ApiError> {
        self.calls.push(format!("player {name}"));
        match self.lookup_failure {
            Some(0) => return Err(ApiError::Network("timeout: global".into())),
            Some(status) => {
                return Err(ApiError::Status {
                    status,
                    url: "user".into(),
                });
            }
            None => {}
        }
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
        stop_on_lookup_error: false,
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

fn failing(status: u16) -> Fake {
    Fake {
        lookup_failure: Some(status),
        ..Fake::default()
    }
}

#[test]
fn a_failed_lookup_still_reads_the_replays_with_the_config_name() {
    let osu = osu_folder();
    let mut events = Vec::new();
    let mut api = failing(0);
    let read = read(
        &source(osu.path(), None),
        Some(&mut api),
        "",
        &mut Timings::new(),
        &mut collect(&mut events),
    )
    .unwrap();
    assert!(read.player.is_none());
    assert_eq!(read.scan.replays.len(), 2);
    let lines = log_lines(&events);
    assert!(lines[0].starts_with("could not look the player up: cannot reach the osu! API"));
    assert!(lines.iter().any(|line| line.starts_with("player: Lemon")));
    assert!(lines.iter().any(|line| line.contains("replays read")));
}

#[test]
fn a_failed_lookup_stops_before_the_replays_when_asked_to() {
    for status in [0, 401, 404, 429, 503] {
        let osu = osu_folder();
        let mut events = Vec::new();
        let mut api = failing(status);
        let mut asked = source(osu.path(), None);
        asked.stop_on_lookup_error = true;
        let error = read(
            &asked,
            Some(&mut api),
            "",
            &mut Timings::new(),
            &mut collect(&mut events),
        )
        .err()
        .unwrap();
        let lookup = error.downcast_ref::<PlayerLookup>().unwrap();
        assert_eq!(lookup.player, "Lemon");
        match (status, &lookup.error) {
            (0, ApiError::Network(_)) => {}
            (code, ApiError::Status { status, .. }) => assert_eq!(*status, code),
            other => panic!("{other:?}"),
        }
        assert!(events.is_empty(), "{status}: {events:?}");
    }
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
fn load_catalog_through_the_server_names_it() {
    let osu = osu_folder();
    let work = tempfile::tempdir().unwrap();
    let maps = work.path().join("maps.json");
    let mut fake = Fake {
        shared: true,
        ..Fake::default()
    };
    let mut timings = Timings::new();
    let mut events = Vec::new();
    let scan = read(
        &source(osu.path(), Some(maps.clone())),
        None,
        "",
        &mut timings,
        &mut |_| {},
    )
    .unwrap()
    .scan;
    load_catalog(
        Some(&maps),
        &scan,
        Some(&mut fake),
        "",
        NOW,
        &mut timings,
        &mut collect(&mut events),
    )
    .unwrap();
    assert!(
        log_lines(&events)
            .iter()
            .any(|line| line.starts_with("asking the Lost Scores server about 1 maps"))
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
            beatmapset_id: None,
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
            beatmapset_id: None,
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

fn expired_login(error: &anyhow::Error) -> bool {
    error
        .chain()
        .any(|cause| matches!(cause.downcast_ref(), Some(ApiError::LoginRequired)))
}

#[test]
fn an_expired_login_stops_the_status_refresh_instead_of_a_warning() {
    let _osu = osu_folder();
    let mut catalog = MapCatalog::default();
    catalog.insert(
        SAMPLE_MD5.to_owned(),
        MapInfo {
            beatmap_id: Some(5),
            beatmapset_id: None,
            status: MapStatus::Pending,
            ranked_at: None,
            outdated: false,
            checked_at: None,
        },
    );
    let mut fake = Fake {
        expired: true,
        ..Fake::default()
    };
    let error = refresh_statuses(
        None,
        &mut catalog,
        &mut fake,
        NOW,
        &mut Timings::new(),
        &mut |_| {},
    )
    .unwrap_err();
    assert!(expired_login(&error), "{error:#}");
}

#[test]
fn an_expired_login_while_checking_missing_maps_stays_an_expired_login() {
    let _osu = osu_folder();
    let root = tempfile::tempdir().unwrap();
    let osu = root.path().join("osu");
    for (name, counts, score, ticks) in [
        ("a.osr", [7, 0, 0, 0], 500_000, EARLY_TICKS),
        ("b.osr", [5, 2, 0, 0], 900_000, EARLY_TICKS + 1),
    ] {
        let replay = Replay {
            mode: 0,
            player: "Lemon",
            beatmap_md5: UNCACHED_MD5,
            counts,
            score,
            max_combo: 8,
            mods: 0,
            ticks,
        };
        write(&osu, &format!("Data/r/{name}"), &replay.bytes());
    }
    write(&osu, "Songs/.keep", b"");
    let mut fake = Fake {
        expired: true,
        ..Fake::default()
    };
    let mut source = source(&osu, Some(root.path().join("maps.json")));
    source.check_missing = true;
    let result = read(
        &source,
        Some(&mut fake),
        "",
        &mut Timings::new(),
        &mut |_| {},
    );
    let error = result
        .err()
        .unwrap_or_else(|| panic!("no error after {:?}", fake.calls));
    assert!(expired_login(&error), "{error:#}");
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
fn the_window_and_the_command_line_build_the_same_saved_scan() {
    let player = Player {
        id: 7,
        names: vec!["Lemon".into(), "OldName".into()],
        pp: Some(1000.0),
        accuracy: Some(98.5),
        global_rank: Some(12),
    };
    let found = Lost {
        rows: Vec::new(),
        total: 3,
        ranked: 2,
    };
    let mut from = source(Path::new("/osu"), None);
    from.check_missing = true;
    let before = unix_now();
    let scan = saved(&from, true, &player, found, None);
    assert_eq!(scan.version, history::VERSION);
    assert!((before..=unix_now()).contains(&scan.made_at));
    assert_eq!((scan.player_id, scan.player_name.as_str()), (7, "Lemon"));
    assert!(scan.include_all && scan.check_missing);
    assert_eq!((scan.lost_total, scan.lost_ranked), (3, 2));
    assert!(scan.lost.is_empty() && scan.potential.is_none());
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

fn lost_row(md5: &str, beatmap_id: u32, beatmapset_id: Option<u32>) -> LostRow {
    LostRow {
        pp: 300.0,
        kept_pp: 200.0,
        score: 1_000_000,
        kept_score: 900_000,
        mods: String::new(),
        unix_time: 1_700_000_000,
        played: "2023-11-14 22:13".into(),
        status: MapStatus::Ranked,
        beatmap_md5: md5.into(),
        beatmap_id: Some(beatmap_id),
        beatmapset_id,
        artist: "A".into(),
        title: "T".into(),
        version: "V".into(),
        file: format!("{md5}.osr"),
        creator: "M".into(),
        hits: Hits {
            n300: 300,
            n100: 2,
            n50: 0,
            misses: 0,
        },
        accuracy: 99.5,
        grade: Grade::S,
        max_combo: 500,
    }
}

#[test]
fn lost_plays_without_a_set_number_get_one_in_a_single_request() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("maps.json");
    let mut fake = Fake {
        sets: vec![(11, 44), (13, 46)],
        ..Fake::default()
    };
    let known = |beatmap_id: u32, status: MapStatus| MapInfo {
        beatmap_id: Some(beatmap_id),
        beatmapset_id: None,
        status,
        ranked_at: None,
        outdated: false,
        checked_at: None,
    };
    let mut catalog = MapCatalog::default();
    catalog.insert("a".into(), known(11, MapStatus::Ranked));
    catalog.insert("b".into(), known(12, MapStatus::Ranked));
    catalog.insert("c".into(), known(13, MapStatus::Unknown));
    let mut found = Lost {
        rows: vec![
            lost_row("a", 11, None),
            lost_row("b", 12, Some(7)),
            lost_row("c", 13, None),
        ],
        total: 3,
        ranked: 2,
    };
    let mut events = Vec::new();
    fill_set_ids(
        Some(&path),
        &mut found,
        &mut catalog,
        &mut fake,
        &mut collect(&mut events),
    )
    .unwrap();
    assert_eq!(fake.calls, ["batch [11]"]);
    assert_eq!(found.rows[0].beatmapset_id, Some(44));
    assert_eq!(found.rows[1].beatmapset_id, Some(7));
    assert_eq!(found.rows[2].beatmapset_id, None);
    assert_eq!(
        MapCatalog::load(&path)
            .unwrap()
            .get("a")
            .unwrap()
            .beatmapset_id,
        Some(44)
    );
    fake.calls.clear();
    fill_set_ids(
        Some(&path),
        &mut found,
        &mut catalog,
        &mut fake,
        &mut collect(&mut events),
    )
    .unwrap();
    assert!(fake.calls.is_empty(), "{:?}", fake.calls);
}
