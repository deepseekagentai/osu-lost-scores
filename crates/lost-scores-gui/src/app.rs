use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use lost_scores_app::online::Online;
use lost_scores_app::pipeline::Api;
use lost_scores_app::pipeline::Source;
use lost_scores_app::rows::thousands;
use lost_scores_app::setup::Settings;
use lost_scores_app::{cache, history, keys, setup};
use osu_api::Credentials;
use slint::{ComponentHandle, ModelRc, VecModel, Weak};

use crate::account::{
    Field, KeysError, ProfileTexts, check_fields, profile_texts, username_from_input,
};
use crate::avatar::{avatar_pixels, fetch, render_avatar};
use crate::log::{clock_now, split, stamp};
use crate::scan::{NEEDS_KEYS, Phase, ScanState};
use crate::worker::{self, Job, Message, Sink};
use crate::{APP_ID, LogEntry, MainWindow, State, choose_folder, show_path};

const START_MESSAGE: &str =
    "Could not find your osu! folder: pick it with the button next to the path";
const AVATAR_LOGICAL: f32 = 120.0;
const ACCOUNT_SETTINGS: &str = "https://osu.ppy.sh/home/account/edit";

struct Session {
    window: MainWindow,
    weak: Weak<MainWindow>,
    log: Rc<VecModel<LogEntry>>,
    credentials: RefCell<Option<Credentials>>,
    avatar: RefCell<Option<Vec<u8>>>,
    rendered: Cell<u32>,
    scan: RefCell<ScanState>,
    shown: Cell<usize>,
    refreshing: Cell<bool>,
    counts: Cell<(usize, usize)>,
    closed: Arc<AtomicBool>,
}

struct LoopSink {
    weak: Weak<MainWindow>,
    closed: Arc<AtomicBool>,
}

impl Sink for LoopSink {
    fn is_open(&self) -> bool {
        !self.closed.load(Ordering::Relaxed)
    }

    fn send(&self, message: Message) -> bool {
        self.is_open()
            && self
                .weak
                .upgrade_in_event_loop(move |_| {
                    if let Some(app) = SESSION.with(|slot| slot.borrow().clone()) {
                        app.handle(message);
                    }
                })
                .is_ok()
    }
}

struct Profile {
    player_id: u32,
    texts: ProfileTexts,
}

pub fn run() -> Result<()> {
    let started = Instant::now();
    let window = MainWindow::new()?;
    slint::set_xdg_app_id(APP_ID)?;
    trace_first_frame(&window, started);
    let session = Rc::new(Session {
        window: window.clone_strong(),
        weak: window.as_weak(),
        log: Rc::new(VecModel::default()),
        credentials: RefCell::new(None),
        avatar: RefCell::new(None),
        rendered: Cell::new(0),
        scan: RefCell::new(ScanState::default()),
        shown: Cell::new(0),
        refreshing: Cell::new(false),
        counts: Cell::new((0, 0)),
        closed: Arc::new(AtomicBool::new(false)),
    });
    let closed = session.closed.clone();
    window.window().on_close_requested(move || {
        closed.store(true, Ordering::Relaxed);
        slint::CloseRequestResponse::HideWindow
    });
    session.setup(&window);
    SESSION.with(|slot| *slot.borrow_mut() = Some(session.clone()));
    let timer = slint::Timer::default();
    let watcher = session.clone();
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(500),
        move || watcher.refresh_avatar(),
    );
    if std::env::var_os("LOST_SCORES_GUI_AUTOSTART").is_some() {
        let starter = session.clone();
        slint::Timer::single_shot(Duration::from_secs(2), move || starter.start_scan());
    }
    window.run()?;
    Ok(())
}

fn clamped_scores(count: u16) -> u16 {
    count.clamp(*setup::SCORES_TO_SHOW.start(), *setup::SCORES_TO_SHOW.end())
}

fn stored_credentials() -> Option<Credentials> {
    keys::from_env().or_else(|| setup::credentials_file().and_then(|path| keys::load(&path)))
}

fn config_player() -> Option<String> {
    let osu_dir = setup::resolve_osu_dir(None).ok()?;
    setup::player(&setup::read_user_config(&osu_dir)).ok()
}

fn saved_player() -> Option<String> {
    load_settings().player
}

fn update_settings(change: impl FnOnce(&mut Settings)) {
    if let Some(path) = setup::config_file() {
        let mut settings = setup::load_settings(&path);
        change(&mut settings);
        let _ = setup::save_settings(&path, &settings);
    }
}

fn load_settings() -> Settings {
    setup::config_file()
        .map(|path| setup::load_settings(&path))
        .unwrap_or_default()
}

fn save_player(name: &str) {
    update_settings(|settings| settings.player = Some(name.to_owned()));
}

impl Session {
    fn setup(self: &Rc<Self>, window: &MainWindow) {
        let state = window.global::<State>();
        state.set_log(ModelRc::from(self.log.clone()));
        match setup::resolve_osu_dir(None) {
            Ok(dir) => state.set_osu_dir(show_path(&dir).into()),
            Err(_) => state.set_status(START_MESSAGE.into()),
        }
        let settings = load_settings();
        state.set_include_all(settings.include_all);
        state.set_check_missing(settings.check_missing);
        state.set_show_lost(settings.show_lost_in_top_image);
        state.set_scores_to_show(i32::from(clamped_scores(settings.scores_to_show)));
        self.bind(&state);
        *self.credentials.borrow_mut() = stored_credentials();
        let name = saved_player().or_else(config_player);
        if let (Some(credentials), Some(name)) = (self.credentials.borrow().clone(), name) {
            state.set_connected(true);
            state.set_user_name(name.clone().into());
            self.fetch_profile(credentials, name);
        }
    }

    fn bind(self: &Rc<Self>, state: &State<'_>) {
        let weak = self.weak.clone();
        state.on_pick_folder(move || pick_folder(weak.clone()));
        let session = self.clone();
        state.on_open_keys(move || session.open_keys());
        let session = self.clone();
        state.on_cancel_keys(move || session.window.global::<State>().set_keys_open(false));
        let session = self.clone();
        state.on_save_keys(move || session.save_keys());
        state.on_open_account_settings(|| {
            let _ = open::that(ACCOUNT_SETTINGS);
        });
        let session = self.clone();
        state.on_ask_logout(move || session.window.global::<State>().set_confirm_open(true));
        let session = self.clone();
        state.on_cancel_logout(move || session.window.global::<State>().set_confirm_open(false));
        let session = self.clone();
        state.on_confirm_logout(move || session.log_out());
        let session = self.clone();
        state.on_set_include_all(move |value| {
            session.window.global::<State>().set_include_all(value);
            update_settings(|settings| settings.include_all = value);
        });
        let session = self.clone();
        state.on_set_check_missing(move |value| {
            session.window.global::<State>().set_check_missing(value);
            update_settings(|settings| settings.check_missing = value);
        });
        let session = self.clone();
        state.on_set_show_lost(move |value| {
            session.window.global::<State>().set_show_lost(value);
            update_settings(|settings| settings.show_lost_in_top_image = value);
        });
        let session = self.clone();
        state.on_commit_scores(move |text| {
            if let Some(count) = setup::parse_scores_to_show(&text) {
                session
                    .window
                    .global::<State>()
                    .set_scores_to_show(i32::from(count));
                update_settings(|settings| settings.scores_to_show = count);
            }
        });
        let session = self.clone();
        state.on_start_scan(move || session.start_scan());
    }

    fn log(&self, text: &str) {
        trace(&format!("log: {text}"));
        let line = stamp(clock_now(), text);
        let (time, text) = crate::log::split(&line);
        self.log.push(LogEntry {
            time: time.into(),
            text: text.into(),
        });
    }

    fn open_keys(&self) {
        let state = self.window.global::<State>();
        let saved = saved_player();
        let from_config = saved.is_none();
        let name = saved.or_else(config_player).unwrap_or_default();
        let credentials = self.credentials.borrow().clone();
        state.set_keys_user_from_config(from_config && !name.is_empty());
        state.set_keys_user(name.into());
        state.set_keys_id(credentials.as_ref().map_or("", |c| &c.client_id).into());
        state.set_keys_secret(credentials.as_ref().map_or("", |c| &c.client_secret).into());
        self.clear_key_errors(&state);
        state.set_keys_open(true);
    }

    fn clear_key_errors(&self, state: &State<'_>) {
        state.set_keys_user_missing(false);
        state.set_keys_id_missing(false);
        state.set_keys_secret_missing(false);
        state.set_keys_error("".into());
    }

    fn save_keys(self: &Rc<Self>) {
        let state = self.window.global::<State>();
        let user = state.get_keys_user().to_string();
        let id = state.get_keys_id().trim().to_owned();
        let secret = state.get_keys_secret().trim().to_owned();
        self.clear_key_errors(&state);
        if let Err(missing) = check_fields(&user, &id, &secret) {
            state.set_keys_user_missing(missing.contains(&Field::Username));
            state.set_keys_id_missing(missing.contains(&Field::ClientId));
            state.set_keys_secret_missing(missing.contains(&Field::ClientSecret));
            return;
        }
        let name = username_from_input(&user);
        state.set_keys_busy(true);
        state.set_status(format!("Validating user '{name}' and API keys...").into());
        let credentials = Credentials {
            client_id: id,
            client_secret: secret,
        };
        let weak = self.weak.clone();
        thread::spawn(move || {
            let result = validate(&credentials, &name);
            on_ui(&weak, move |app| {
                app.window.global::<State>().set_keys_busy(false);
                app.keys_checked(credentials, name, result);
            });
        });
    }

    fn keys_checked(
        self: &Rc<Self>,
        credentials: Credentials,
        name: String,
        result: Result<Profile, KeysError>,
    ) {
        let state = self.window.global::<State>();
        state.set_status("".into());
        match result {
            Ok(profile) => {
                if let Some(path) = setup::credentials_file()
                    && let Err(error) = keys::save(&path, &credentials)
                {
                    state.set_keys_error(format!("Could not save the keys: {error:#}").into());
                    return;
                }
                save_player(&name);
                *self.credentials.borrow_mut() = Some(credentials);
                state.set_keys_open(false);
                state.set_connected(true);
                state.set_user_name(name.clone().into());
                self.show_profile(&profile);
                self.fetch_avatar(profile.player_id);
                self.log(&format!("Logged in as {name}"));
            }
            Err(error) => state.set_keys_error(error.to_string().into()),
        }
    }

    fn show_profile(&self, profile: &Profile) {
        let state = self.window.global::<State>();
        state.set_perf_now(profile.texts.performance.clone().into());
        state.set_acc_now(profile.texts.accuracy.clone().into());
        state.set_rank(profile.texts.rank.clone().into());
        state.set_perf_potential("".into());
        state.set_acc_potential("".into());
        state.set_stats_ready(true);
        trace("stats shown");
    }

    fn fetch_profile(self: &Rc<Self>, credentials: Credentials, name: String) {
        let weak = self.weak.clone();
        thread::spawn(move || {
            let result = validate(&credentials, &name);
            on_ui(&weak, move |app| app.profile_fetched(result));
        });
    }

    fn profile_fetched(self: &Rc<Self>, result: Result<Profile, KeysError>) {
        match result {
            Ok(profile) => {
                self.show_profile(&profile);
                self.fetch_avatar(profile.player_id);
            }
            Err(error) => {
                let state = self.window.global::<State>();
                state.set_perf_now("-".into());
                state.set_acc_now("-".into());
                state.set_rank("-".into());
                state.set_stats_ready(true);
                self.log(&error.to_string());
            }
        }
    }

    fn fetch_avatar(self: &Rc<Self>, player_id: u32) {
        let weak = self.weak.clone();
        thread::spawn(move || {
            let cache = cache::avatars_dir();
            let bytes = fetch(player_id, cache.as_deref()).ok();
            on_ui(&weak, move |app| {
                if let Some(bytes) = bytes {
                    *app.avatar.borrow_mut() = Some(bytes);
                    app.rendered.set(0);
                    app.refresh_avatar();
                }
            });
        });
    }

    fn refresh_avatar(&self) {
        let pixels = avatar_pixels(AVATAR_LOGICAL, self.window.window().scale_factor());
        if pixels == self.rendered.get() {
            return;
        }
        let bytes = self.avatar.borrow();
        let Some(bytes) = bytes.as_ref() else {
            return;
        };
        if let Ok(buffer) = render_avatar(bytes, pixels) {
            let state = self.window.global::<State>();
            state.set_avatar(slint::Image::from_rgba8(buffer));
            state.set_has_avatar(true);
            self.rendered.set(pixels);
            trace(&format!("avatar rendered at {pixels} px"));
        }
    }

    fn busy(&self) -> bool {
        self.scan.borrow().phase == Phase::Scanning || self.refreshing.get()
    }

    fn start_scan(self: &Rc<Self>) {
        let state = self.window.global::<State>();
        let credentials = self.credentials.borrow().clone();
        let connected = credentials.is_some() && state.get_connected();
        if !connected {
            state.set_status(NEEDS_KEYS.into());
            return;
        }
        if self.refreshing.get() || !self.scan.borrow().can_start(connected) {
            return;
        }
        let Some(credentials) = credentials else {
            return;
        };
        let Some(scans_dir) = history::scans_dir() else {
            state.set_status("Operation failed".into());
            self.log("Error: cannot find a folder for the scan history");
            return;
        };
        let settings = load_settings();
        let osu_dir = state.get_osu_dir().to_string();
        let job = Job {
            source: Source {
                osu_dir: (!osu_dir.is_empty()).then(|| PathBuf::from(osu_dir)),
                player: Some(state.get_user_name().to_string()),
                songs_dir: None,
                check_missing: settings.check_missing,
                maps: None,
            },
            scans_dir,
            include_all: settings.include_all,
        };
        self.scan.borrow_mut().start();
        self.sync();
        let sink = LoopSink {
            weak: self.weak.clone(),
            closed: self.closed.clone(),
        };
        thread::spawn(move || {
            let mut api = Online::new(credentials);
            worker::run(&job, &mut api, &sink);
        });
    }

    fn sync(&self) {
        let state = self.window.global::<State>();
        let scan = self.scan.borrow();
        state.set_status(scan.status.clone().into());
        state.set_progress(scan.progress);
        state.set_busy(self.busy());
        for line in &scan.log[self.shown.get()..] {
            trace(&format!("log: {line}"));
            let (time, text) = split(line);
            self.log.push(LogEntry {
                time: time.into(),
                text: text.into(),
            });
        }
        self.shown.set(scan.log.len());
        if scan.phase != Phase::Scanning {
            trace(&format!("status: {}", scan.status));
        }
    }

    fn handle(&self, message: Message) {
        match message {
            Message::Event(event) => self.scan.borrow_mut().apply(&event, clock_now()),
            Message::Saving => self.scan.borrow_mut().saving(),
            Message::Finished(done) => {
                self.refreshing.set(true);
                self.counts
                    .set((done.scan.lost_total, done.scan.lost_ranked));
                self.scan
                    .borrow_mut()
                    .finish(done.scan.lost_total, done.scan.lost_ranked);
                self.show_results(&done.scan);
            }
            Message::Refreshed(updated) => {
                self.refreshing.set(false);
                if let Some(done) = updated {
                    self.counts
                        .set((done.scan.lost_total, done.scan.lost_ranked));
                    self.show_results(&done.scan);
                }
                let (total, ranked) = self.counts.get();
                self.scan.borrow_mut().finish(total, ranked);
            }
            Message::Failed(error) => {
                self.refreshing.set(false);
                self.scan.borrow_mut().fail(&error);
            }
        }
        self.sync();
    }

    fn show_results(&self, scan: &history::SavedScan) {
        let Some(potential) = &scan.potential else {
            return;
        };
        let state = self.window.global::<State>();
        state.set_perf_now(thousands(potential.current_pp).into());
        state.set_perf_potential(thousands(potential.potential_pp).into());
        state.set_acc_now(format!("{:.2}%", potential.current_accuracy).into());
        state.set_acc_potential(format!("{:.2}%", potential.potential_accuracy).into());
        state.set_rank(
            potential
                .global_rank
                .map_or_else(
                    || "unranked".to_owned(),
                    |rank| format!("#{}", thousands(f64::from(rank))),
                )
                .into(),
        );
        state.set_stats_ready(true);
    }

    fn log_out(&self) {
        let state = self.window.global::<State>();
        state.set_confirm_open(false);
        if let Some(path) = setup::credentials_file()
            && let Err(error) = keys::delete(&path)
        {
            self.log(&format!("Could not delete the saved keys: {error:#}"));
            return;
        }
        *self.credentials.borrow_mut() = None;
        *self.avatar.borrow_mut() = None;
        self.rendered.set(0);
        state.set_connected(false);
        state.set_has_avatar(false);
        state.set_stats_ready(false);
        state.set_user_name("".into());
        self.log("Logged out");
    }
}

thread_local! {
    static SESSION: RefCell<Option<Rc<Session>>> = const { RefCell::new(None) };
}

fn on_ui(weak: &Weak<MainWindow>, action: impl FnOnce(&Rc<Session>) + Send + 'static) {
    let _ = weak.upgrade_in_event_loop(move |_| {
        if let Some(app) = SESSION.with(|slot| slot.borrow().clone()) {
            action(&app);
        }
    });
}

fn validate(credentials: &Credentials, name: &str) -> Result<Profile, KeysError> {
    let mut online = Online::new(credentials.clone());
    online.check().map_err(|_| KeysError::Rejected)?;
    let player = online.player(name).map_err(|_| KeysError::Rejected)?;
    Ok(Profile {
        player_id: player.id,
        texts: profile_texts(&player),
    })
}

fn pick_folder(weak: Weak<MainWindow>) {
    thread::spawn(move || {
        let Some(dir) = rfd::FileDialog::new()
            .set_title("Select osu! Game Directory")
            .pick_folder()
        else {
            return;
        };
        let result = choose_folder(Path::new(&dir)).map_err(|error| error.to_string());
        let _ = weak.upgrade_in_event_loop(move |window| {
            let state = window.global::<State>();
            match result {
                Ok(path) => {
                    state.set_osu_dir(path.into());
                    state.set_status("".into());
                }
                Err(message) => state.set_status(message.into()),
            }
        });
    });
}

fn tracing() -> bool {
    std::env::var_os("LOST_SCORES_GUI_TRACE").is_some()
}

fn trace(message: &str) {
    if tracing() {
        eprintln!("{message}");
    }
}

fn trace_first_frame(window: &MainWindow, started: Instant) {
    if !tracing() {
        return;
    }
    let mut reported = false;
    let _ = window.window().set_rendering_notifier(move |state, _| {
        if !reported && matches!(state, slint::RenderingState::AfterRendering) {
            reported = true;
            eprintln!("first frame after {} ms", started.elapsed().as_millis());
        }
    });
}
