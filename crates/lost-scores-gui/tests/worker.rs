use std::cell::{Cell, RefCell};
use std::fs;
use std::path::Path;
use std::sync::{Mutex, MutexGuard, Once, OnceLock};

use lost_scores_app::history;
use lost_scores_app::online::Player;
use lost_scores_app::pipeline::{Api, Event, Source};
use lost_scores_core::maps::{FetchedMap, MapSource};
use lost_scores_core::missing::OsuFiles;
use lost_scores_gui::worker::{Job, Message, Outcome, Sink, render_command, run};
use osu_api::{ApiError, Score};
use tempfile::TempDir;

fn isolate_environment() {
    static HOME: OnceLock<TempDir> = OnceLock::new();
    static SET: Once = Once::new();
    SET.call_once(|| {
        let home = HOME.get_or_init(|| tempfile::tempdir().unwrap());
        for (name, folder) in [
            ("XDG_CONFIG_HOME", "config"),
            ("XDG_DATA_HOME", "data"),
            ("XDG_CACHE_HOME", "cache"),
        ] {
            unsafe { std::env::set_var(name, home.path().join(folder)) };
        }
    });
}

struct Fake {
    known: bool,
}

impl MapSource for Fake {
    type Error = ApiError;

    fn batch(&mut self, _: &[u32]) -> Result<Vec<FetchedMap>, ApiError> {
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
        if self.known {
            Ok(Player {
                id: 7,
                names: vec![name.to_owned()],
                pp: 1000.0,
                accuracy: 98.0,
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

fn serial() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn job(root: &Path) -> Job {
    isolate_environment();
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
    let outcome = run(&job, &mut Fake { known: true }, &sink);
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
    let outcome = run(&job, &mut Fake { known: false }, &sink);
    assert!(matches!(outcome, Outcome::Failed));
    let messages = sink.messages.borrow();
    assert!(matches!(messages.last(), Some(Message::Failed(_))));
    assert!(history::list(&job.scans_dir).is_empty());
}

#[test]
fn events_after_the_window_closed_are_dropped() {
    let _serial = serial();
    let root = tempfile::tempdir().unwrap();
    let job = job(root.path());
    let sink = Recorder::open_for(3);
    let outcome = run(&job, &mut Fake { known: true }, &sink);
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
    let outcome = run(&job, &mut Fake { known: true }, &sink);
    assert!(matches!(outcome, Outcome::Closed));
    assert!(sink.messages.borrow().is_empty());
}

#[test]
fn a_failed_render_keeps_the_scan() {
    let _serial = serial();
    let root = tempfile::tempdir().unwrap();
    let job = job(root.path());
    let sink = Recorder::open();
    let outcome = run(&job, &mut Fake { known: true }, &sink);
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
    let outcome = run(&job, &mut Fake { known: true }, &sink);
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
    job.renderer = renderer_script(
        root.path(),
        "echo '2 of 9 covers could not be downloaded' >&2",
    );
    let sink = Recorder::open();
    run(&job, &mut Fake { known: true }, &sink);
    let lines = logs(&sink);
    let folder = history::list(&job.scans_dir)[0].folder.clone();
    assert!(lines.contains(&"2 of 9 covers could not be downloaded".to_owned()));
    assert!(lines.contains(&format!("Images saved to {}", folder.display())));
}

#[test]
fn the_render_command_points_at_the_new_folder() {
    let command = render_command(
        Path::new("/opt/lost-scores-gui"),
        Path::new("/scans/2026-11-14_09-44-31Z"),
    );
    assert_eq!(command.get_program(), "/opt/lost-scores-gui");
    let args: Vec<_> = command.get_args().collect();
    assert_eq!(
        args,
        ["render-images", "--cached", "/scans/2026-11-14_09-44-31Z"]
    );
}
