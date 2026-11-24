use std::cell::{Cell, RefCell};
use std::fs;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use anyhow::Result;
use lost_scores_app::history::{self, SavedScan};
use lost_scores_app::online::Player;
use lost_scores_app::pipeline::{Api, Event, Source};
use lost_scores_app::rows::{Hits, LostRow};
use lost_scores_core::grade::Grade;
use lost_scores_core::maps::{FetchedMap, MapCatalog, MapInfo, MapSource, MapStatus};
use lost_scores_core::missing::OsuFiles;
use lost_scores_gui::share::rasters::Fetch;
use lost_scores_gui::worker::{Job, Message, Outcome, Sink, create_images, render_command, run};
use osu_api::{ApiError, Score};
use tempfile::TempDir;

fn isolate_environment() -> TempDir {
    let home = tempfile::tempdir().unwrap();
    for (name, folder) in [
        ("XDG_CONFIG_HOME", "config"),
        ("XDG_DATA_HOME", "data"),
        ("XDG_CACHE_HOME", "cache"),
    ] {
        unsafe { std::env::set_var(name, home.path().join(folder)) };
    }
    home
}

struct Fake {
    known: bool,
    statistics: bool,
    failure: Option<u16>,
    expired_maps: bool,
}

impl Fake {
    fn known() -> Self {
        Self {
            known: true,
            statistics: true,
            failure: None,
            expired_maps: false,
        }
    }

    fn unknown() -> Self {
        Self {
            known: false,
            ..Self::known()
        }
    }

    fn without_statistics() -> Self {
        Self {
            statistics: false,
            ..Self::known()
        }
    }

    fn failing(status: u16) -> Self {
        Self {
            failure: Some(status),
            ..Self::known()
        }
    }

    fn expired() -> Self {
        Self::failing(1)
    }
}

impl MapSource for Fake {
    type Error = ApiError;

    fn batch(&mut self, _: &[u32]) -> Result<Vec<FetchedMap>, ApiError> {
        if self.expired_maps {
            return Err(ApiError::LoginRequired);
        }
        Ok(Vec::new())
    }

    fn lookup(&mut self, _: &str) -> Result<Option<FetchedMap>, ApiError> {
        Ok(None)
    }
}

impl OsuFiles for Fake {
    type Error = ApiError;

    fn download(&mut self, _: u32) -> Result<Vec<u8>, ApiError> {
        Ok(Vec::new())
    }
}

impl Api for Fake {
    fn player(&mut self, name: &str) -> Result<Player, ApiError> {
        match self.failure {
            Some(0) => return Err(ApiError::Network("timeout: global".into())),
            Some(1) => return Err(ApiError::LoginRequired),
            Some(status) => {
                return Err(ApiError::Status {
                    status,
                    url: "user".into(),
                });
            }
            None => {}
        }
        if self.known {
            Ok(Player {
                id: 7,
                names: vec![name.to_owned()],
                pp: self.statistics.then_some(1000.0),
                accuracy: self.statistics.then_some(98.0),
                global_rank: Some(1),
            })
        } else {
            Err(ApiError::Status {
                status: 401,
                url: "token".into(),
            })
        }
    }

    fn top(&mut self, _: u32) -> Result<Vec<Score>, ApiError> {
        Ok(Vec::new())
    }
}

struct Recorder {
    messages: RefCell<Vec<Message>>,
    open_for: Cell<usize>,
}

impl Recorder {
    fn open() -> Self {
        Self::open_for(usize::MAX)
    }

    fn open_for(messages: usize) -> Self {
        Self {
            messages: RefCell::default(),
            open_for: Cell::new(messages),
        }
    }
}

impl Sink for Recorder {
    fn is_open(&self) -> bool {
        self.open_for.get() > 0
    }

    fn send(&self, message: Message) -> bool {
        if self.open_for.get() == 0 {
            return false;
        }
        self.open_for.set(self.open_for.get() - 1);
        self.messages.borrow_mut().push(message);
        true
    }
}

struct Serial {
    _home: TempDir,
    _lock: MutexGuard<'static, ()>,
}

fn serial() -> Serial {
    static LOCK: Mutex<()> = Mutex::new(());
    let lock = LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    Serial {
        _home: isolate_environment(),
        _lock: lock,
    }
}

fn job(root: &Path) -> Job {
    let osu = root.join("osu");
    fs::create_dir_all(osu.join("Data/r")).unwrap();
    fs::create_dir_all(osu.join("Songs")).unwrap();
    Job {
        source: Source {
            osu_dir: Some(osu.clone()),
            player: Some("Lemon".into()),
            songs_dir: Some(osu.join("Songs")),
            check_missing: false,
            maps: Some(root.join("maps.json")),
            stop_on_lookup_error: true,
        },
        scans_dir: root.join("scans"),
        include_all: true,
        renderer: root.join("no-such-renderer"),
        share_count: 20,
        show_lost_in_top: true,
    }
}

fn logs(sink: &Recorder) -> Vec<String> {
    sink.messages
        .borrow()
        .iter()
        .filter_map(|message| match message {
            Message::Event(Event::Log(text)) => Some(text.clone()),
            _ => None,
        })
        .collect()
}

#[cfg(unix)]
fn renderer_script(root: &Path, body: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = root.join("renderer.sh");
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[test]
fn a_scan_of_an_empty_folder_finishes_and_is_saved() {
    let _serial = serial();
    let root = tempfile::tempdir().unwrap();
    let job = job(root.path());
    let sink = Recorder::open();
    let outcome = run(&job, &mut Fake::known(), &sink);
    if let Some(Message::Failed(text)) = sink.messages.borrow().last() {
        panic!("{text}");
    }
    assert!(matches!(outcome, Outcome::Finished));
    let messages = sink.messages.borrow();
    assert!(messages.iter().any(|m| matches!(m, Message::Saving)));
    let Some(Message::Finished(done)) = messages
        .iter()
        .find(|message| matches!(message, Message::Finished(_)))
    else {
        panic!("no Finished message");
    };
    assert_eq!(history::load(&done.folder).unwrap(), done.scan);
    assert_eq!(history::list(&job.scans_dir).len(), 1);
    assert!(matches!(messages.last(), Some(Message::Refreshed(None))));
}

#[test]
fn a_rejected_player_fails_the_scan_and_saves_nothing() {
    let _serial = serial();
    let root = tempfile::tempdir().unwrap();
    let job = job(root.path());
    let sink = Recorder::open();
    let outcome = run(&job, &mut Fake::unknown(), &sink);
    assert!(matches!(outcome, Outcome::Failed));
    let messages = sink.messages.borrow();
    assert!(matches!(messages.last(), Some(Message::Failed(_))));
    assert!(history::list(&job.scans_dir).is_empty());
}

fn failure_text(api: &mut Fake) -> (String, Vec<String>) {
    let root = tempfile::tempdir().unwrap();
    let job = job(root.path());
    fs::write(
        job.source.osu_dir.as_ref().unwrap().join("Data/r/x.osr"),
        b"x",
    )
    .unwrap();
    let sink = Recorder::open();
    let outcome = run(&job, api, &sink);
    assert!(matches!(outcome, Outcome::Failed));
    let lines = logs(&sink);
    let messages = sink.messages.borrow();
    let Some(Message::Failed(text)) = messages.last() else {
        panic!("the scan did not fail");
    };
    assert!(history::list(&job.scans_dir).is_empty());
    (text.clone(), lines)
}

#[test]
fn an_expired_login_ends_the_scan_with_its_own_message() {
    let _serial = serial();
    let root = tempfile::tempdir().unwrap();
    let job = job(root.path());
    let sink = Recorder::open();
    let outcome = run(&job, &mut Fake::expired(), &sink);
    assert!(matches!(outcome, Outcome::Failed));
    assert!(matches!(
        sink.messages.borrow().last(),
        Some(Message::LoginExpired)
    ));
    assert!(history::list(&job.scans_dir).is_empty());
}

#[test]
fn an_expired_login_in_the_status_refresh_ends_with_its_own_message() {
    let _serial = serial();
    let root = tempfile::tempdir().unwrap();
    let job = job(root.path());
    let mut catalog = MapCatalog::default();
    catalog.insert(
        "a".repeat(32),
        MapInfo {
            beatmap_id: Some(5),
            beatmapset_id: None,
            status: MapStatus::Graveyard,
            ranked_at: None,
            outdated: false,
            checked_at: None,
        },
    );
    catalog.save(job.source.maps.as_ref().unwrap()).unwrap();
    let sink = Recorder::open();
    let mut api = Fake {
        expired_maps: true,
        ..Fake::known()
    };
    run(&job, &mut api, &sink);
    assert!(matches!(
        sink.messages.borrow().last(),
        Some(Message::LoginExpired)
    ));
}

#[test]
fn a_network_error_on_the_lookup_stops_the_scan_and_says_so() {
    let _serial = serial();
    let (text, lines) = failure_text(&mut Fake::failing(0));
    assert_eq!(
        text,
        "Could not reach the osu! API: timeout: global. osu! may be having problems; try again in a few minutes."
    );
    assert!(
        lines.iter().all(|line| !line.contains("replays read")),
        "{lines:?}"
    );
    assert!(lines.iter().all(|line| !line.contains("keys")), "{lines:?}");
}

#[test]
fn a_server_error_on_the_lookup_is_reported_as_unreachable() {
    let _serial = serial();
    let (text, _) = failure_text(&mut Fake::failing(503));
    assert!(text.starts_with("Could not reach the osu! API: "), "{text}");
    assert!(text.ends_with("try again in a few minutes."), "{text}");
}

#[test]
fn a_401_on_the_lookup_blames_the_keys_and_stops_early() {
    let _serial = serial();
    let (text, lines) = failure_text(&mut Fake::failing(401));
    assert_eq!(text, "User not found or API keys are invalid");
    assert!(lines.iter().all(|line| !line.contains("replays read")));
}

#[test]
fn a_404_on_the_lookup_names_the_player() {
    let _serial = serial();
    let (text, lines) = failure_text(&mut Fake::failing(404));
    assert_eq!(text, "The osu! API does not know the player Lemon");
    assert!(lines.iter().all(|line| !line.contains("replays read")));
}

#[test]
fn a_429_on_the_lookup_says_the_api_is_busy() {
    let _serial = serial();
    let (text, _) = failure_text(&mut Fake::failing(429));
    assert_eq!(text, "The osu! API is busy, try again in a minute");
}

#[test]
fn events_after_the_window_closed_are_dropped() {
    let _serial = serial();
    let root = tempfile::tempdir().unwrap();
    let job = job(root.path());
    let sink = Recorder::open_for(3);
    let outcome = run(&job, &mut Fake::known(), &sink);
    assert!(matches!(outcome, Outcome::Closed));
    assert!(sink.messages.borrow().len() <= 3);
    assert!(history::list(&job.scans_dir).is_empty());
}

#[test]
fn a_closed_window_never_starts_the_scan() {
    let _serial = serial();
    let root = tempfile::tempdir().unwrap();
    let job = job(root.path());
    let sink = Recorder::open_for(0);
    let outcome = run(&job, &mut Fake::known(), &sink);
    assert!(matches!(outcome, Outcome::Closed));
    assert!(sink.messages.borrow().is_empty());
}

#[test]
fn a_failed_render_keeps_the_scan() {
    let _serial = serial();
    let root = tempfile::tempdir().unwrap();
    let job = job(root.path());
    let sink = Recorder::open();
    let outcome = run(&job, &mut Fake::known(), &sink);
    assert!(matches!(outcome, Outcome::Finished));
    assert_eq!(history::list(&job.scans_dir).len(), 1);
    let messages = sink.messages.borrow();
    assert!(messages.iter().any(|m| matches!(m, Message::Finished(_))));
    assert!(matches!(messages.last(), Some(Message::Refreshed(None))));
    drop(messages);
    let lines = logs(&sink);
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("Could not create the images: ")),
        "{lines:?}"
    );
    assert!(!lines.iter().any(|line| line.starts_with("Images saved")));
}

#[cfg(unix)]
#[test]
fn a_renderer_that_exits_with_an_error_reports_its_line() {
    let _serial = serial();
    let root = tempfile::tempdir().unwrap();
    let mut job = job(root.path());
    job.renderer = renderer_script(root.path(), "echo 'cannot render: boom' >&2\nexit 1");
    let sink = Recorder::open();
    let outcome = run(&job, &mut Fake::known(), &sink);
    assert!(matches!(outcome, Outcome::Finished));
    assert_eq!(history::list(&job.scans_dir).len(), 1);
    assert!(
        logs(&sink)
            .iter()
            .any(|line| line == "Could not create the images: cannot render: boom")
    );
}

#[cfg(unix)]
#[test]
fn a_renderer_that_succeeds_is_logged_with_its_warnings() {
    let _serial = serial();
    let root = tempfile::tempdir().unwrap();
    let mut job = job(root.path());
    job.renderer = renderer_script(root.path(), "echo '2 of 9 covers could not be used' >&2");
    let sink = Recorder::open();
    run(&job, &mut Fake::known(), &sink);
    let lines = logs(&sink);
    let folder = history::list(&job.scans_dir)[0].folder.clone();
    assert!(lines.contains(&"2 of 9 covers could not be used".to_owned()));
    assert!(lines.contains(&format!("Images saved to {}", folder.display())));
}

#[test]
fn the_render_command_carries_the_image_options_of_the_window() {
    let command = render_command(
        Path::new("/opt/lost-scores-gui"),
        Path::new("/scans/2026-11-14_09-44-31Z"),
        3,
        true,
    );
    assert_eq!(command.get_program(), "/opt/lost-scores-gui");
    let args: Vec<_> = command.get_args().collect();
    assert_eq!(
        args,
        [
            "render-images",
            "--cached",
            "--count",
            "3",
            "--show-lost-in-top",
            "/scans/2026-11-14_09-44-31Z"
        ]
    );
}

#[test]
fn the_render_command_leaves_out_the_top_flag_when_it_is_off() {
    let command = render_command(Path::new("/r"), Path::new("/scans/x"), 10, false);
    let args: Vec<_> = command.get_args().collect();
    assert_eq!(
        args,
        ["render-images", "--cached", "--count", "10", "/scans/x"]
    );
}

#[cfg(unix)]
#[test]
fn a_panicking_renderer_is_reported_by_its_message_line() {
    let _serial = serial();
    let root = tempfile::tempdir().unwrap();
    let mut job = job(root.path());
    job.renderer = renderer_script(
        root.path(),
        "echo \"thread 'main' panicked at src/x.rs:1:1:\" >&2\necho 'boom' >&2\necho 'cannot render the share images: boom' >&2\necho 'note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace' >&2\nexit 101",
    );
    let sink = Recorder::open();
    let outcome = run(&job, &mut Fake::known(), &sink);
    assert!(matches!(outcome, Outcome::Finished));
    let lines = logs(&sink);
    assert!(
        lines.contains(
            &"Could not create the images: cannot render the share images: boom".to_owned()
        ),
        "{lines:?}"
    );
}

#[cfg(unix)]
struct Panicking;

#[cfg(unix)]
impl Fetch for Panicking {
    fn get(&mut self, _url: &str) -> Result<Option<Vec<u8>>> {
        panic!("the cover server blew up");
    }
}

fn one_lost_play() -> SavedScan {
    SavedScan {
        version: history::VERSION,
        made_at: 1_794_649_471,
        player_id: 7,
        player_name: "Synthetic".into(),
        include_all: true,
        check_missing: false,
        lost_total: 1,
        lost_ranked: 1,
        before_cutoff: None,
        lost: vec![LostRow {
            pp: 400.0,
            kept_pp: 380.0,
            counted_pp: None,
            score: 900_000,
            kept_score: 800_000,
            mods: String::new(),
            unix_time: 1_794_600_000,
            played: "2026-11-13".into(),
            status: MapStatus::Ranked,
            beatmap_md5: format!("{:032x}", 1),
            beatmap_id: Some(1),
            beatmapset_id: Some(101),
            artist: "Synthetic Artist".into(),
            title: "Synthetic Title".into(),
            version: "Insane".into(),
            file: "x.osr".into(),
            creator: "Synthetic Mapper".into(),
            hits: Hits {
                n300: 400,
                n100: 6,
                n50: 1,
                misses: 2,
            },
            accuracy: 97.25,
            grade: Grade::S,
            max_combo: 321,
        }],
        potential: None,
    }
}

#[cfg(unix)]
#[test]
fn a_panic_while_fetching_covers_is_logged_and_the_renderer_still_runs() {
    let _serial = serial();
    let root = tempfile::tempdir().unwrap();
    let mut job = job(root.path());
    job.renderer = renderer_script(root.path(), "exit 0");
    let mut lines = Vec::new();
    let mut events = |event: Event| {
        if let Event::Log(text) = event {
            lines.push(text);
        }
    };
    create_images(
        &job,
        &one_lost_play(),
        root.path(),
        None,
        &mut events,
        &mut Panicking,
    );
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert_eq!(
        lines[0],
        "Could not fetch the covers: the cover server blew up"
    );
    assert_eq!(
        lines[1],
        format!("Images saved to {}", root.path().display())
    );
}

#[cfg(unix)]
#[test]
fn the_renderer_binary_does_not_panic_on_a_folder_that_is_not_utf8() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    use std::process::Command;

    let root = tempfile::tempdir().unwrap();
    let folder = root.path().join(OsStr::from_bytes(b"scan-\xff\xfe"));
    let output = Command::new(env!("CARGO_BIN_EXE_lost-scores-gui"))
        .arg("render-images")
        .arg(&folder)
        .env("XDG_CONFIG_HOME", root.path().join("config"))
        .env("XDG_DATA_HOME", root.path().join("data"))
        .env("XDG_CACHE_HOME", root.path().join("cache"))
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.starts_with("cannot render the share images: "),
        "{stderr}"
    );
    assert!(!stderr.contains("panicked"), "{stderr}");
}

struct NotAnImage;

impl Fetch for NotAnImage {
    fn get(&mut self, _url: &str) -> Result<Option<Vec<u8>>> {
        Ok(Some(b"<html>not a picture</html>".to_vec()))
    }
}

#[test]
fn a_cover_that_is_not_a_picture_is_reported_as_unusable() {
    let _serial = serial();
    let root = tempfile::tempdir().unwrap();
    let job = job(root.path());
    let mut lines = Vec::new();
    let mut events = |event: Event| {
        if let Event::Log(text) = event {
            lines.push(text);
        }
    };
    create_images(
        &job,
        &one_lost_play(),
        root.path(),
        None,
        &mut events,
        &mut NotAnImage,
    );
    assert!(
        lines.contains(&"1 of 1 covers could not be used".to_owned()),
        "{lines:?}"
    );
}

#[test]
fn the_isolated_home_is_removed_when_the_test_ends() {
    let home = {
        let _serial = serial();
        let config = std::env::var_os("XDG_CONFIG_HOME").unwrap();
        let home = Path::new(&config).parent().unwrap().to_path_buf();
        assert!(home.is_dir());
        home
    };
    assert!(!home.exists(), "{}", home.display());
}

#[test]
fn a_player_without_statistics_still_gets_a_scan_without_the_potential_top() {
    let _serial = serial();
    let root = tempfile::tempdir().unwrap();
    let job = job(root.path());
    let sink = Recorder::open();
    let outcome = run(&job, &mut Fake::without_statistics(), &sink);
    assert!(matches!(outcome, Outcome::Finished));
    let listed = history::list(&job.scans_dir);
    assert_eq!(listed.len(), 1);
    assert_eq!(history::load(&listed[0].folder).unwrap().potential, None);
    let lines = logs(&sink);
    assert_eq!(
        lines
            .iter()
            .filter(|line| line.as_str()
                == "The osu! API sent no statistics for this player; the Potential Top is left out")
            .count(),
        1,
        "{lines:?}"
    );
    assert!(
        !sink
            .messages
            .borrow()
            .iter()
            .any(|message| matches!(message, Message::Failed(_)))
    );
}
