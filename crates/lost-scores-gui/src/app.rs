use std::cell::{Cell, RefCell};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use anyhow::Result;
use jiff::tz::TimeZone;
use lost_scores_app::login::{self as loopback, LoginError, PRIVACY_URL, TERMS_URL};
use lost_scores_app::online::{Online, Player, USER_AGENT};
use lost_scores_app::pipeline::{self, PotentialResult, Source};
use lost_scores_app::rows::thousands;
use lost_scores_app::session::{self as stored, Session as Login};
use lost_scores_app::setup::Settings;
use lost_scores_app::shared::{self, Shared};
use lost_scores_app::updates::latest_release;
use lost_scores_app::{cache, history, keys, replays, setup, submission};
use osu_api::{ApiError, Credentials, Http, Server, UserKey};
use slint::{ComponentHandle, Model, ModelRc, VecModel, Weak};

use crate::account::{
    Account, Field, KeysError, Profile, check_fields, keys_error, lookup_failure, parse_account,
    sentence,
};
use crate::avatar::{GUEST, avatar_pixels, fetch, render_avatar};
use crate::clipboard::{Clipboard, Request};
use crate::log::{clock_now, stamp};
use crate::newest::{load_newest, newest_of_player};
use crate::previews::{Wanted, picture_tops, plan, render_all, step_target};
use crate::scan::{LOGIN_EXPIRED, NEEDS_ACCOUNT, NEEDS_FOLDER, Phase, ScanState, check_folders};
use crate::scans::{delete_folders, older_done, older_prompt, reread};
use crate::send::{self, Call, Courier, Reply};
use crate::share::render::{ShareImage, images_in};
use crate::share::upload::{self, Choices, Consent};
use crate::table::{Column, Tab, Table, anchor_index, tone, when, when_short};
use crate::update::{self, Outcome};
use crate::view::{RowsModel, foot, mods_width};
use crate::worker::{self, Job, Message, Sink};
use crate::{APP_ID, LogEntry, MainWindow, Preview, State, choose_folder, show_path};

const AVATAR_LOGICAL: f32 = 120.0;
const ROW_HEIGHT: f32 = 64.0;
const ACCOUNT_SETTINGS: &str = "https://osu.ppy.sh/home/account/edit";
const BEATMAP_PAGE: &str = "https://osu.ppy.sh/b/";
const DEVELOPER_SITE: &str = "https://lemon4ik.kz/";
const SITE: &str = "https://lost.lemon4ik.kz";
const UPDATE_TIMEOUT: Duration = Duration::from_secs(15);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(600);
const COPIED_FOR: Duration = Duration::from_secs(2);
const CATBOX_DONE_FOR: Duration = Duration::from_secs(4);
const CATBOX_FAILED_FOR: Duration = Duration::from_secs(6);
const PREVIEW_DEBOUNCE: Duration = Duration::from_millis(150);

struct Session {
    window: MainWindow,
    weak: Weak<MainWindow>,
    log: Rc<VecModel<LogEntry>>,
    log_file: Option<PathBuf>,
    updated: RefCell<Option<String>>,
    renderer: RefCell<Option<PathBuf>>,
    credentials: RefCell<Option<Credentials>>,
    login: RefCell<Option<Login>>,
    login_epoch: Cell<u64>,
    avatar: RefCell<Option<Vec<u8>>>,
    rendered: Cell<u32>,
    scan: RefCell<ScanState>,
    shown: Cell<usize>,
    refreshing: Cell<bool>,
    counts: Cell<(usize, usize)>,
    closed: Arc<AtomicBool>,
    settings: RefCell<Settings>,
    player: Cell<Option<u32>>,
    epoch: Cell<u64>,
    dialog: Arc<AtomicBool>,
    table: Rc<RefCell<Table>>,
    rows: Rc<RowsModel>,
    tab: Cell<Tab>,
    query: RefCell<String>,
    entries: RefCell<Vec<history::ScanEntry>>,
    folder: RefCell<Option<PathBuf>>,
    clipboard: RefCell<Clipboard>,
    copied: slint::Timer,
    catbox_reset: slint::Timer,
    consent: Cell<Consent>,
    choices: RefCell<Choices>,
    uploading: Cell<bool>,
    pending_delete: RefCell<Option<Removal>>,
    opens_results: Cell<bool>,
    opener: RefCell<Opener>,
    revoker: RefCell<Revoker>,
    courier: RefCell<Courier>,
    sent_epoch: Cell<u64>,
    offered: RefCell<Option<PathBuf>>,
    declined: RefCell<Option<PathBuf>>,
    publish_next: Cell<bool>,
    quiet_next: Cell<bool>,
    replays_next: RefCell<Option<PathBuf>>,
    replays_running: RefCell<Option<(PathBuf, Arc<AtomicBool>)>>,
    replays_turned_away: RefCell<Vec<PathBuf>>,
    top_rows: RefCell<Vec<usize>>,
    marks: RefCell<Vec<(usize, i32)>>,
    images_view: Cell<bool>,
    picked_images: Cell<bool>,
    picture_tops: RefCell<Vec<f32>>,
    images_scroll: Cell<(f32, f32, f32)>,
    preview_room: Cell<f32>,
    preview_shown: RefCell<Vec<Wanted>>,
    preview_wanted: RefCell<Vec<Wanted>>,
    preview_epoch: Cell<u64>,
    preview_timer: slint::Timer,
}

type Opener = Arc<dyn Fn(&OsStr) -> std::io::Result<()> + Send + Sync>;
type Revoker = Arc<dyn Fn(Login) + Send + Sync>;

enum Access {
    Keys(Credentials),
    Login(String),
}

enum Lookup {
    Expired,
    Failed(String),
}

struct Removal {
    folders: Vec<PathBuf>,
    done: String,
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

pub fn run() -> Result<()> {
    run_with(|_| {})
}

impl Session {
    fn create(window: &MainWindow) -> Rc<Self> {
        let table = Rc::new(RefCell::new(Table::default()));
        Rc::new(Session {
            rows: Rc::new(RowsModel::new(table.clone())),
            table,
            tab: Cell::new(Tab::Lost),
            query: RefCell::new(String::new()),
            entries: RefCell::new(Vec::new()),
            folder: RefCell::new(None),
            clipboard: RefCell::new(Clipboard::system()),
            copied: slint::Timer::default(),
            catbox_reset: slint::Timer::default(),
            consent: Cell::new(Consent::new(false)),
            choices: RefCell::new(Choices::default()),
            uploading: Cell::new(false),
            pending_delete: RefCell::new(None),
            opens_results: Cell::new(false),
            opener: RefCell::new(Arc::new(|target| open::that(target))),
            revoker: RefCell::new(Arc::new(|login: Login| {
                thread::spawn(move || {
                    let _ = Server::new(Http::new(USER_AGENT), &login.server)
                        .with_session(&login.token)
                        .logout();
                });
            })),
            courier: RefCell::new(send::over_http()),
            sent_epoch: Cell::new(0),
            offered: RefCell::new(None),
            declined: RefCell::new(None),
            publish_next: Cell::new(false),
            quiet_next: Cell::new(false),
            replays_next: RefCell::new(None),
            replays_running: RefCell::new(None),
            replays_turned_away: RefCell::new(Vec::new()),
            top_rows: RefCell::new(Vec::new()),
            marks: RefCell::new(Vec::new()),
            images_view: Cell::new(true),
            picked_images: Cell::new(true),
            picture_tops: RefCell::new(Vec::new()),
            images_scroll: Cell::new((0.0, 0.0, 0.0)),
            preview_room: Cell::new(0.0),
            preview_shown: RefCell::new(Vec::new()),
            preview_wanted: RefCell::new(Vec::new()),
            preview_epoch: Cell::new(0),
            preview_timer: slint::Timer::default(),
            window: window.clone_strong(),
            weak: window.as_weak(),
            log: Rc::new(VecModel::default()),
            updated: RefCell::new(None),
            renderer: RefCell::new(None),
            log_file: setup::data_root()
                .map(|dir| dir.join("log.txt"))
                .inspect(|file| crate::log::rotate(file)),
            credentials: RefCell::new(None),
            login: RefCell::new(None),
            login_epoch: Cell::new(0),
            avatar: RefCell::new(None),
            rendered: Cell::new(0),
            scan: RefCell::new(ScanState::default()),
            shown: Cell::new(0),
            refreshing: Cell::new(false),
            counts: Cell::new((0, 0)),
            closed: Arc::new(AtomicBool::new(false)),
            settings: RefCell::new(Settings::default()),
            player: Cell::new(None),
            epoch: Cell::new(0),
            dialog: Arc::new(AtomicBool::new(false)),
        })
    }
}

pub fn run_with(prepare: impl FnOnce(&MainWindow)) -> Result<()> {
    let window = MainWindow::new()?;
    slint::set_xdg_app_id(APP_ID)?;
    prepare(&window);
    let session = Session::create(&window);
    let closed = session.closed.clone();
    window.window().on_close_requested(move || {
        closed.store(true, Ordering::Relaxed);
        slint::CloseRequestResponse::HideWindow
    });
    crate::layout::bind(&window);
    session.setup(&window);
    SESSION.with(|slot| *slot.borrow_mut() = Some(session.clone()));
    let timer = slint::Timer::default();
    let watcher = session.clone();
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(500),
        move || {
            watcher.refresh_avatar();
            watcher.refresh_previews();
        },
    );
    window.run()?;
    Ok(())
}

fn clamped_scores(count: u16) -> u16 {
    count.clamp(*setup::SCORES_TO_SHOW.start(), *setup::SCORES_TO_SHOW.end())
}

fn stored_credentials() -> Option<Credentials> {
    keys::from_env().or_else(|| setup::credentials_file().and_then(|path| keys::load(&path)))
}

fn stored_login() -> Option<Login> {
    setup::session_file().and_then(|path| stored::load_for(&path, &shared::server_url()))
}

fn load_settings() -> Settings {
    setup::config_file()
        .map(|path| setup::load_settings(&path))
        .unwrap_or_default()
}

impl Session {
    fn setup(self: &Rc<Self>, window: &MainWindow) {
        let state = window.global::<State>();
        state.set_log(ModelRc::from(self.log.clone()));
        state.set_rows(ModelRc::from(self.rows.clone()));
        state.set_version(env!("CARGO_PKG_VERSION").into());
        match setup::resolve_osu_dir(None) {
            Ok(dir) => state.set_osu_dir(show_path(&dir).into()),
            Err(_) => state.set_status(NEEDS_FOLDER.into()),
        }
        *self.settings.borrow_mut() = load_settings();
        {
            let settings = self.settings.borrow();
            state.set_include_all(settings.include_all);
            state.set_check_missing(settings.check_missing);
            state.set_show_lost(settings.show_lost_in_top_image);
            state.set_scores_to_show(i32::from(clamped_scores(settings.scores_to_show)));
            self.consent.set(Consent::new(settings.catbox_confirmed));
        }
        self.init_mode();
        self.bind(&state);
        self.bind_extras(&state);
        self.load_newest();
        self.refresh_avatar();
        self.restore(stored_login(), stored_credentials());
        let leftovers = std::mem::take(&mut self.settings.borrow_mut().update_leftovers);
        if !leftovers.is_empty() {
            for leftover in &leftovers {
                lost_scores_app::updates::remove_leftover(leftover);
            }
            self.change_settings(|settings| settings.update_leftovers.clear());
        }
        if update::due(
            self.settings.borrow().update_checked_at,
            pipeline::unix_now(),
        ) {
            self.check_updates(false);
        }
    }

    fn restore(self: &Rc<Self>, login: Option<Login>, credentials: Option<Credentials>) {
        let keys = credentials.is_some();
        *self.credentials.borrow_mut() = credentials;
        self.window.global::<State>().set_own_keys(keys);
        match login {
            Some(login) if !keys => self.adopt_login(login),
            _ => self.connect_with_keys(),
        }
    }

    fn connect_with_keys(self: &Rc<Self>) {
        if self.credentials.borrow().is_none() {
            return;
        }
        let Some(name) = self.saved_player().or_else(|| self.config_name()) else {
            return;
        };
        let state = self.window.global::<State>();
        state.set_connected(true);
        state.set_user_name(name.clone().into());
        self.fetch_profile(Account::Name(name));
    }

    fn adopt_login(self: &Rc<Self>, login: Login) {
        let state = self.window.global::<State>();
        self.epoch.set(self.epoch.get() + 1);
        self.show_guest();
        state.set_logged_in(true);
        state.set_connected(true);
        state.set_editing_name(false);
        state.set_stats_ready(false);
        state.set_user_name(login.username.clone().into());
        let account = Account::Id(login.osu_id);
        *self.login.borrow_mut() = Some(login);
        self.show_sent();
        self.refresh_sent();
        self.fetch_profile(account);
    }

    fn log_in(self: &Rc<Self>) {
        let state = self.window.global::<State>();
        let mut server = Server::new(Http::new(USER_AGENT), &shared::server_url());
        let pending = match loopback::begin(&server) {
            Ok(pending) => pending,
            Err(error) => {
                state.set_status(format!("Could not start the login: {error}").into());
                return;
            }
        };
        self.login_epoch.set(self.login_epoch.get() + 1);
        let epoch = self.login_epoch.get();
        state.set_logging_in(true);
        state.set_status("Log in with osu! in the browser".into());
        self.launch_or(
            "the browser",
            pending.url.clone().into(),
            Some(login_fallback(&pending.url)),
        );
        let weak = self.weak.clone();
        thread::spawn(move || {
            let result = loopback::wait(&mut server, pending, loopback::LIMIT);
            on_ui(&weak, move |app| app.login_finished(epoch, result));
        });
    }

    fn login_finished(self: &Rc<Self>, epoch: u64, result: Result<Login, LoginError>) {
        if self.login_epoch.get() != epoch {
            return;
        }
        let state = self.window.global::<State>();
        state.set_logging_in(false);
        match result {
            Ok(login) => {
                if let Some(path) = setup::session_file()
                    && let Err(error) = stored::save(&path, &login)
                {
                    state.set_status("Could not save the login: see the log".into());
                    self.log(&format!("Could not save the login: {error:#}"));
                    return;
                }
                state.set_status("".into());
                self.log(&format!("Logged in as {}", login.username));
                self.adopt_login(login);
            }
            Err(error) => {
                let text = sentence(&error.to_string());
                self.log(&format!("Login failed: {text}"));
                state.set_status(text.into());
            }
        }
    }

    fn login_expired(&self) {
        *self.login.borrow_mut() = None;
        if let Some(path) = setup::session_file()
            && let Err(error) = stored::delete(&path)
        {
            self.log(&format!("Could not delete the saved login: {error:#}"));
        }
        self.disconnect();
        self.window
            .global::<State>()
            .set_status(LOGIN_EXPIRED.into());
    }

    fn saved_player(&self) -> Option<String> {
        self.settings.borrow().player.clone()
    }

    fn config_name(&self) -> Option<String> {
        let dir = self.window.global::<State>().get_osu_dir().to_string();
        if dir.is_empty() {
            return None;
        }
        setup::player(&setup::read_user_config(Path::new(&dir))).ok()
    }

    fn change_settings(&self, change: impl FnOnce(&mut Settings)) {
        let saved = {
            let mut settings = self.settings.borrow_mut();
            change(&mut settings);
            setup::config_file().map(|path| setup::save_settings(&path, &settings))
        };
        if let Some(Err(error)) = saved {
            self.log(&format!("Could not save the settings: {error:#}"));
        }
    }

    fn commit_scores(&self, text: &str) {
        if let Some(count) = setup::scores_to_show_typed(text) {
            self.window
                .global::<State>()
                .set_scores_to_show(i32::from(count));
            self.change_settings(|settings| settings.scores_to_show = count);
        }
    }

    fn commit_pending_scores(&self) {
        let state = self.window.global::<State>();
        let pending = state.get_scores_pending();
        state.set_scores_pending("".into());
        self.commit_scores(&pending);
        state.set_outside_clicks(state.get_outside_clicks() + 1);
    }

    fn bind(self: &Rc<Self>, state: &State<'_>) {
        let session = self.clone();
        state.on_pick_folder(move || session.pick_folder());
        let session = self.clone();
        state.on_open_keys(move || session.open_keys());
        let session = self.clone();
        state.on_log_in(move || session.log_in());
        let session = self.clone();
        state.on_change_player(move |text| session.change_player(&text));
        let session = self.clone();
        state.on_cancel_keys(move || session.window.global::<State>().set_keys_open(false));
        let session = self.clone();
        state.on_save_keys(move || session.save_keys());
        let session = self.clone();
        state.on_open_account_settings(move || session.launch("the page", ACCOUNT_SETTINGS.into()));
        let session = self.clone();
        state.on_ask_logout(move || {
            session.close_popups();
            session.window.global::<State>().set_confirm_open(true);
        });
        let session = self.clone();
        state.on_cancel_logout(move || session.window.global::<State>().set_confirm_open(false));
        let session = self.clone();
        state.on_confirm_logout(move || session.log_out());
        let session = self.clone();
        state.on_set_include_all(move |value| {
            session.window.global::<State>().set_include_all(value);
            session.change_settings(|settings| settings.include_all = value);
            session.include_in_place(value);
        });
        let session = self.clone();
        state.on_set_check_missing(move |value| {
            session.window.global::<State>().set_check_missing(value);
            session.change_settings(|settings| settings.check_missing = value);
        });
        let session = self.clone();
        state.on_set_show_lost(move |value| {
            session.window.global::<State>().set_show_lost(value);
            session.change_settings(|settings| settings.show_lost_in_top_image = value);
        });
        let session = self.clone();
        state.on_commit_scores(move |text| session.commit_scores(&text));
        let session = self.clone();
        state.on_start_scan(move || session.start_scan());
        let session = self.clone();
        state.on_set_tab(move |index| session.pick_table(Tab::from_index(index)));
        let session = self.clone();
        state.on_sort_by(move |index| session.sort_by(Column::from_index(index)));
        let session = self.clone();
        state.on_search_changed(move |text| session.search_changed(&text));
        let session = self.clone();
        state.on_select_scan(move |index| session.select_scan(index as usize));
        let session = self.clone();
        state.on_open_scan_folder(move || session.open_scan_folder());
        let session = self.clone();
        state.on_open_beatmap(move |id| {
            if id > 0 {
                session.launch("the page", format!("{BEATMAP_PAGE}{id}").into());
            }
        });
        let session = self.clone();
        state.on_copy_row(move |index| session.copy_row(index as usize));
        let session = self.clone();
        state.on_delete_scan(move |index| session.ask_delete_scan(index as usize));
        let session = self.clone();
        state.on_delete_older_scans(move || session.ask_delete_older());
        let session = self.clone();
        state.on_confirm_delete(move || session.confirm_delete());
        let session = self.clone();
        state.on_cancel_delete(move || session.cancel_delete());
        let session = self.clone();
        state.on_show_images(move || session.pick_images());
        let session = self.clone();
        state.on_results_opening(move || session.show_picked_mode());
        let session = self.clone();
        state.on_images_scrolled(move |top, visible, content| {
            session.images_scrolled(top, visible, content)
        });
        let session = self.clone();
        state.on_step_picture(move |direction| session.step_picture(direction));
        let session = self.clone();
        state.on_preview_room_changed(move |room| session.preview_room_changed(room));
        let session = self.clone();
        state.on_open_image(move |index| session.open_image(index as usize));
        let session = self.clone();
        state.on_copy_image(move |index| session.copy_image(index as usize));
        let session = self.clone();
        state.on_upload_all(move || session.ask_upload_all());
        let session = self.clone();
        state.on_confirm_upload(move |remember| session.confirm_upload(remember));
        let session = self.clone();
        state.on_cancel_upload(move || session.cancel_upload());
        let session = self.clone();
        state.on_publish_scan(move || session.send_about(Call::Publish));
        let session = self.clone();
        state.on_unpublish_scan(move || session.send_about(Call::Unpublish));
        let session = self.clone();
        state.on_ask_unsend(move || session.ask_unsend());
        let session = self.clone();
        state.on_confirm_unsend(move || session.confirm_unsend());
        let session = self.clone();
        state.on_publish_offer(move || session.publish_offer());
        let session = self.clone();
        state.on_share(move || session.share());
        let session = self.clone();
        state.on_dismiss_offer(move || session.dismiss_offer());
        let session = self.clone();
        state.on_cancel_unsend(move || {
            session
                .window
                .global::<State>()
                .set_unsend_confirm_open(false);
        });
    }

    fn bind_extras(self: &Rc<Self>, state: &State<'_>) {
        let session = self.clone();
        state.on_open_developer_site(move || session.launch("the page", DEVELOPER_SITE.into()));
        let session = self.clone();
        state.on_open_terms(move || session.launch("the page", TERMS_URL.into()));
        let session = self.clone();
        state.on_open_privacy(move || session.launch("the page", PRIVACY_URL.into()));
        let session = self.clone();
        state.on_open_hall(move || {
            let page = match session.login.borrow().as_ref() {
                Some(login) => format!("{SITE}/rankings?me={}", login.osu_id),
                None => format!("{SITE}/rankings"),
            };
            session.launch("the page", page.into());
        });
        let session = self.clone();
        state.on_open_profile(move || {
            let page = session
                .login
                .borrow()
                .as_ref()
                .map(|login| format!("{SITE}/u/{}", login.osu_id));
            if let Some(page) = page {
                session.launch("the page", page.into());
            }
        });
        let session = self.clone();
        state.on_open_about(move || {
            session.close_popups();
            session.window.global::<State>().set_about_open(true);
        });
        let session = self.clone();
        state.on_close_about(move || session.window.global::<State>().set_about_open(false));
        let session = self.clone();
        state.on_ask_clear_cache(move || {
            session.close_popups();
            session
                .window
                .global::<State>()
                .set_cache_confirm_open(true);
        });
        let session = self.clone();
        state.on_cancel_clear_cache(move || {
            session
                .window
                .global::<State>()
                .set_cache_confirm_open(false);
        });
        let session = self.clone();
        state.on_confirm_clear_cache(move || session.clear_cache());
        let session = self.clone();
        state.on_check_updates(move || session.check_updates(true));
    }

    fn clear_cache(&self) {
        self.window.global::<State>().set_cache_confirm_open(false);
        let Some(root) = setup::cache_root() else {
            self.log("Could not find the cache folder");
            return;
        };
        match cache::clear(&root) {
            Ok(removed) => self.log(&format!("Cache cleared: {removed} files removed")),
            Err(error) => self.log(&format!("Could not clear the cache: {error:#}")),
        }
    }

    fn check_updates(self: &Rc<Self>, asked: bool) {
        let state = self.window.global::<State>();
        if state.get_checking_updates() {
            return;
        }
        if let Some(version) = self.updated.borrow().clone() {
            if asked {
                self.log(&format!(
                    "Version {version} is installed; restart the program to use it"
                ));
            }
            return;
        }
        state.set_checking_updates(true);
        let weak = self.weak.clone();
        thread::spawn(move || {
            let agent = |timeout| -> ureq::Agent {
                ureq::Agent::config_builder()
                    .timeout_global(Some(timeout))
                    .user_agent(lost_scores_app::online::USER_AGENT)
                    .build()
                    .into()
            };
            let outcome = latest_release(&agent(UPDATE_TIMEOUT)).map(|release| {
                update::update(
                    release,
                    env!("CARGO_PKG_VERSION"),
                    update::official(),
                    &std::env::current_exe().unwrap_or_default(),
                    lost_scores_app::updates::PUBLIC_KEY,
                    &mut agent(DOWNLOAD_TIMEOUT),
                )
            });
            on_ui(&weak, move |app| {
                app.window.global::<State>().set_checking_updates(false);
                match outcome {
                    Ok(outcome) => {
                        if !matches!(outcome, Outcome::Failed(_)) {
                            app.change_settings(|settings| {
                                settings.update_checked_at = Some(pipeline::unix_now())
                            });
                        }
                        if asked || !outcome.quiet() {
                            app.log(&outcome.text());
                        }
                        match outcome {
                            Outcome::Updated { version, installed } => {
                                *app.updated.borrow_mut() = Some(version);
                                *app.renderer.borrow_mut() = Some(installed.old_program);
                                app.change_settings(|settings| {
                                    settings.update_leftovers = installed.leftovers
                                });
                            }
                            Outcome::Manual {
                                page: Some(page), ..
                            } if asked => {
                                app.launch("the page", page.into());
                            }
                            _ => {}
                        }
                    }
                    Err(error) if asked => {
                        app.log(&format!("Could not check for updates: {error:#}"))
                    }
                    Err(_) => {}
                }
            });
        });
    }

    fn load_newest(self: &Rc<Self>) {
        let Some(dir) = history::scans_dir() else {
            return;
        };
        let weak = self.weak.clone();
        thread::spawn(move || {
            let entries = history::list(&dir);
            let newest = load_newest(&entries, history::load);
            on_ui(&weak, move |app| {
                let zone = TimeZone::system();
                for (made_at, error) in &newest.failures {
                    app.log(&format!(
                        "Could not load the scan from {}: {error}",
                        when(*made_at, &zone)
                    ));
                }
                let folder = newest
                    .loaded
                    .as_ref()
                    .map(|(index, _)| entries[*index].folder.clone());
                *app.entries.borrow_mut() = entries;
                if let Some((index, scan)) = newest.loaded {
                    app.show_scan(index, folder, &scan);
                }
            });
        });
    }

    fn select_scan(self: &Rc<Self>, index: usize) {
        let state = self.window.global::<State>();
        state.set_scans_open(false);
        let Some(entry) = self.entries.borrow().get(index).cloned() else {
            return;
        };
        let weak = self.weak.clone();
        thread::spawn(move || {
            let loaded = history::load(&entry.folder);
            on_ui(&weak, move |app| match loaded {
                Ok(scan) => app.show_scan(index, Some(entry.folder), &scan),
                Err(error) => app.log(&format!("Could not open the scan: {error:#}")),
            });
        });
    }

    fn register(&self, folder: &Path, scan: &history::SavedScan) {
        if let Some(dir) = history::scans_dir() {
            *self.entries.borrow_mut() = history::list(&dir);
        }
        self.show_scan(0, Some(folder.to_path_buf()), scan);
    }

    fn show_scan(&self, index: usize, folder: Option<PathBuf>, scan: &history::SavedScan) {
        let zone = TimeZone::system();
        let mut table = Table::from_scan(scan);
        table.set_include_all(self.window.global::<State>().get_include_all());
        *self.table.borrow_mut() = table;
        *self.folder.borrow_mut() = folder;
        *self.top_rows.borrow_mut() = replays::in_top(scan);
        self.marks.borrow_mut().clear();
        let state = self.window.global::<State>();
        self.relist(index);
        state.set_scan_date(when_short(scan.made_at, &zone).into());
        state.set_has_results(true);
        self.refresh_shared();
        let images = self.images_view.get();
        self.show_tab(self.tab.get());
        if images {
            self.show_images();
        }
        self.show_sent();
        self.refresh_sent();
        let waiting = self
            .shown_sent()
            .is_some_and(|sent| !sent.queued.is_empty());
        if waiting && let Some(folder) = self.folder.borrow().clone() {
            self.send_replays(&folder);
        }
    }

    fn relist(&self, fallback: usize) {
        let zone = TimeZone::system();
        let folder = self.folder.borrow().clone();
        let index = folder
            .as_ref()
            .and_then(|folder| {
                self.entries
                    .borrow()
                    .iter()
                    .position(|entry| &entry.folder == folder)
            })
            .unwrap_or(fallback);
        let labels: Vec<slint::SharedString> = self
            .entries
            .borrow()
            .iter()
            .map(|entry| when(entry.made_at, &zone).into())
            .collect();
        let state = self.window.global::<State>();
        state.set_scans(ModelRc::new(VecModel::from(labels)));
        state.set_scan_selected(index as i32);
        self.mark_scans();
    }

    fn mark_scans(&self) {
        let server = self.sending_login().map(|login| login.server);
        let marks: Vec<i32> = self
            .entries
            .borrow()
            .iter()
            .map(|entry| {
                let sent = server
                    .as_deref()
                    .and_then(|server| history::submission(&entry.folder, server));
                match sent {
                    Some(sent) if send::published(&sent) => match sent.status.as_str() {
                        "verified" => 2,
                        "hidden" | "rejected" => 3,
                        _ => 1,
                    },
                    _ => 0,
                }
            })
            .collect();
        self.window
            .global::<State>()
            .set_scan_marks(ModelRc::new(VecModel::from(marks)));
    }

    fn ask_delete_scan(&self, index: usize) {
        let entry = match self.entries.borrow().get(index) {
            Some(entry) if index > 0 => entry.clone(),
            _ => return,
        };
        let date = when(entry.made_at, &TimeZone::system());
        let stays = if history::submission(&entry.folder, &shared::server_url()).is_some() {
            send::SERVER_COPY_STAYS
        } else {
            ""
        };
        self.ask_delete(
            vec![entry.folder],
            format!("Delete the scan from {date}? This can't be undone.{stays}"),
            format!("Deleted the scan from {date}"),
        );
    }

    fn ask_delete_older(self: &Rc<Self>) {
        if let Some(dir) = history::scans_dir() {
            self.reload_scans(&dir);
        }
        let folders: Vec<PathBuf> = self
            .entries
            .borrow()
            .iter()
            .skip(1)
            .map(|entry| entry.folder.clone())
            .collect();
        if folders.is_empty() {
            return;
        }
        let bytes: u64 = folders.iter().map(|folder| history::size(folder)).sum();
        let count = folders.len();
        let server = shared::server_url();
        let sent = folders
            .iter()
            .filter(|folder| history::submission(folder, &server).is_some())
            .count();
        let prompt = format!(
            "{}{}",
            older_prompt(count, bytes),
            send::server_copies(sent)
        );
        self.ask_delete(folders, prompt, older_done(count));
    }

    fn ask_delete(&self, folders: Vec<PathBuf>, message: String, done: String) {
        let state = self.window.global::<State>();
        state.set_scans_open(false);
        *self.pending_delete.borrow_mut() = Some(Removal { folders, done });
        state.set_delete_message(message.into());
        state.set_delete_confirm_open(true);
    }

    fn cancel_delete(&self) {
        *self.pending_delete.borrow_mut() = None;
        self.window.global::<State>().set_delete_confirm_open(false);
    }

    fn confirm_delete(self: &Rc<Self>) {
        self.window.global::<State>().set_delete_confirm_open(false);
        let Some(removal) = self.pending_delete.borrow_mut().take() else {
            return;
        };
        let Some(dir) = history::scans_dir() else {
            self.fail_removal("cannot find the scan history");
            return;
        };
        let weak = self.weak.clone();
        thread::spawn(move || {
            let result = history::remove(&dir, &removal.folders, delete_folders);
            on_ui(&weak, move |app| {
                match result {
                    Ok(()) => {
                        app.log(&removal.done);
                        app.toast("Deleted");
                    }
                    Err(error) => app.fail_removal(&format!("{error:#}")),
                }
                app.reload_scans(&dir);
            });
        });
    }

    fn fail_removal(&self, reason: &str) {
        self.log(&format!("Could not delete the scans: {reason}"));
        self.toast("Could not delete: see the log");
    }

    fn reload_scans(self: &Rc<Self>, dir: &Path) {
        let shown = self.folder.borrow().clone();
        let read = reread(dir, shown.as_deref());
        *self.entries.borrow_mut() = read.entries;
        self.relist(read.index);
        if read.shown_gone {
            self.select_scan(0);
        }
    }

    fn refresh_shared(&self) {
        let found = self
            .folder
            .borrow()
            .as_deref()
            .map(images_in)
            .unwrap_or_default();
        let state = self.window.global::<State>();
        state.set_has_images(!found.is_empty());
        self.choices.borrow_mut().replace(found);
        self.refresh_previews();
    }

    fn sending_login(&self) -> Option<Login> {
        if self.credentials.borrow().is_some() {
            return None;
        }
        self.login.borrow().clone()
    }

    fn shown_sent(&self) -> Option<history::Sent> {
        let login = self.sending_login()?;
        history::submission(self.folder.borrow().as_deref()?, &login.server)
    }

    fn show_sent(&self) {
        self.mark_scans();
        let sent = self.shown_sent();
        let state = self.window.global::<State>();
        state.set_sent_state(sent.as_ref().map(send::label).unwrap_or_default().into());
        state.set_sent_public(sent.as_ref().is_some_and(send::published));
        state.set_sent_verified(sent.as_ref().is_some_and(|sent| sent.status == "verified"));
        state.set_sent_blocked(
            sent.as_ref()
                .is_some_and(|sent| matches!(sent.status.as_str(), "hidden" | "rejected")),
        );
        state.set_replays_in_top(self.top_rows.borrow().len() as i32);
        let marks: Vec<(usize, i32)> = sent
            .iter()
            .flat_map(|sent| {
                let shared = sent.replays.keys().map(|&row| (row, 4));
                shared.chain(sent.queued.iter().map(|&row| (row, 3)))
            })
            .collect();
        if *self.marks.borrow() != marks {
            self.table.borrow_mut().set_replays(&marks);
            self.rows.refresh();
            *self.marks.borrow_mut() = marks;
        }
        let offered = self.offered.borrow().clone();
        state.set_offer_shown(
            !sent.as_ref().is_some_and(send::published)
                && self.sending_login().is_some()
                && offered.is_some()
                && offered == *self.folder.borrow(),
        );
    }

    fn offer(&self, folder: &Path, scan: &history::SavedScan) {
        let declined = self.declined.borrow().as_deref() == Some(folder);
        let offer = scan
            .potential
            .as_ref()
            .and_then(send::offer)
            .filter(|_| !declined);
        *self.offered.borrow_mut() = offer.as_ref().map(|_| folder.to_path_buf());
        let state = self.window.global::<State>();
        state.set_offer_with_replays(false);
        if let Some(offer) = offer {
            state.set_offer_lost(offer.lost.into());
            state.set_offer_before(offer.before.into());
            state.set_offer_after(offer.after.into());
        }
        self.show_sent();
    }

    fn dismiss_offer(&self) {
        let offered = self.offered.borrow_mut().take();
        *self.declined.borrow_mut() = offered;
        self.show_sent();
    }

    fn share(&self) {
        let Some(folder) = self.folder.borrow().clone() else {
            return;
        };
        let offered = history::load(&folder)
            .ok()
            .filter(|scan| scan.potential.as_ref().and_then(send::offer).is_some());
        match offered {
            Some(scan) => {
                if self.declined.borrow().as_deref() == Some(folder.as_path()) {
                    *self.declined.borrow_mut() = None;
                }
                self.offer(&folder, &scan);
            }
            None => self.publish_offer(),
        }
    }

    fn save_privately(&self) {
        if self.sending_login().is_none() || self.shown_sent().is_some() {
            return;
        }
        self.quiet_next.set(true);
        self.submit_scan();
        if !self.window.global::<State>().get_sending() {
            self.quiet_next.set(false);
        }
    }

    fn publish_offer(&self) {
        let state = self.window.global::<State>();
        let wanted = state.get_offer_with_replays() && !self.top_rows.borrow().is_empty();
        *self.replays_next.borrow_mut() = self.folder.borrow().clone().filter(|_| wanted);
        match self.shown_sent() {
            Some(sent) if !send::published(&sent) => self.send_about(Call::Publish),
            Some(_) => {}
            None => {
                self.publish_next.set(true);
                self.submit_scan();
            }
        }
        if state.get_sending() {
            self.dismiss_offer();
        } else {
            self.publish_next.set(false);
            self.replays_next.borrow_mut().take();
        }
    }

    fn queue_top_replays(&self, folder: &Path) {
        let Some(login) = self.sending_login() else {
            return;
        };
        let Some(mut sent) = history::submission(folder, &login.server) else {
            return;
        };
        let shown = self.folder.borrow().as_deref() == Some(folder);
        let rows = if shown {
            self.top_rows.borrow().clone()
        } else {
            history::load(folder)
                .map(|scan| replays::in_top(&scan))
                .unwrap_or_default()
        };
        let fresh: Vec<usize> = rows
            .into_iter()
            .filter(|row| !sent.replays.contains_key(row))
            .collect();
        sent.queued.extend(fresh);
        match history::remember(folder, &sent) {
            Ok(()) => self.send_replays(folder),
            Err(error) => self.log(&format!("Could not keep the replays to share: {error:#}")),
        }
        if shown {
            self.show_sent();
        }
    }

    fn send_replays(&self, folder: &Path) {
        let Some(login) = self.sending_login() else {
            return;
        };
        let osu_dir = self.window.global::<State>().get_osu_dir().to_string();
        if osu_dir.is_empty() {
            return;
        }
        if self.replays_running.borrow().is_some() {
            let mut turned_away = self.replays_turned_away.borrow_mut();
            if !turned_away.iter().any(|waiting| waiting == folder) {
                turned_away.push(folder.to_path_buf());
            }
            return;
        }
        let Some(id) = history::submission(folder, &login.server).map(|sent| sent.id) else {
            return;
        };
        let stop = Arc::new(AtomicBool::new(false));
        *self.replays_running.borrow_mut() = Some((folder.to_path_buf(), stop.clone()));
        let dir = PathBuf::from(osu_dir).join("Data").join("r");
        let courier = self.courier.borrow().clone();
        let weak = self.weak.clone();
        let folder = folder.to_path_buf();
        let now = pipeline::unix_now();
        thread::spawn(move || {
            let mut report = |change: &replays::Change| {
                let (folder, id, change) = (folder.clone(), id.clone(), change.clone());
                on_ui(&weak, move |app| app.replay_changed(&folder, &id, &change));
                !stop.load(Ordering::Relaxed)
            };
            let result = send::share_replays(&courier, &login, &folder, &dir, now, &mut report);
            on_ui(&weak, move |app| app.replayed(&folder, result));
        });
    }

    fn replay_changed(&self, folder: &Path, id: &str, change: &replays::Change) {
        let Some(login) = self.sending_login() else {
            return;
        };
        let Some(mut sent) =
            history::submission(folder, &login.server).filter(|sent| sent.id == id)
        else {
            return;
        };
        replays::apply(&mut sent, change);
        if let Err(error) = history::remember(folder, &sent) {
            self.log(&format!("Could not keep the shared replays: {error:#}"));
            return;
        }
        if self.folder.borrow().as_deref() == Some(folder) {
            self.show_sent();
        }
    }

    fn stop_share(&self, folder: &Path) {
        if let Some((running, stop)) = &*self.replays_running.borrow()
            && running == folder
        {
            stop.store(true, Ordering::Relaxed);
        }
        self.replays_turned_away
            .borrow_mut()
            .retain(|waiting| waiting != folder);
    }

    fn replayed(&self, folder: &Path, result: Result<send::Shared>) {
        self.replays_running.borrow_mut().take();
        match result {
            Ok(shared) => {
                for line in &shared.lines {
                    self.log(line);
                }
                if let Some(text) = shared.summary() {
                    self.toast(&text);
                }
            }
            Err(error) => self.log(&format!("Could not share the replays: {error:#}")),
        }
        if self.folder.borrow().as_deref() == Some(folder) {
            self.show_sent();
        }
        let next = {
            let mut turned_away = self.replays_turned_away.borrow_mut();
            (!turned_away.is_empty()).then(|| turned_away.remove(0))
        };
        if let Some(next) = next {
            self.send_replays(&next);
        }
    }

    fn submit_scan(&self) {
        if self.shown_sent().is_none() {
            self.send(|folder| Ok(Call::Submit(submission::body(&history::load(folder)?)?)));
        }
    }

    fn send_about(&self, call: fn(String) -> Call) {
        if let Some(sent) = self.shown_sent() {
            self.send(move |_| Ok(call(sent.id)));
        }
    }

    fn ask_unsend(&self) {
        if self.shown_sent().is_some() {
            self.close_popups();
            self.window.global::<State>().set_unsend_confirm_open(true);
        }
    }

    fn confirm_unsend(&self) {
        self.window.global::<State>().set_unsend_confirm_open(false);
        self.send_about(Call::Delete);
    }

    fn send(&self, call: impl FnOnce(&Path) -> Result<Call> + Send + 'static) {
        let state = self.window.global::<State>();
        let folder = self.folder.borrow().clone();
        let (Some(login), Some(folder)) = (self.sending_login(), folder) else {
            return;
        };
        if self.busy() || state.get_sending() {
            return;
        }
        state.set_sending(true);
        self.sent_epoch.set(self.sent_epoch.get() + 1);
        let courier = self.courier.borrow().clone();
        let weak = self.weak.clone();
        thread::spawn(move || {
            let result = call(&folder).and_then(|call| Ok(courier(&login, call)?));
            on_ui(&weak, move |app| app.answered(&folder, result));
        });
    }

    fn answered(&self, folder: &Path, result: Result<Reply>) {
        self.window.global::<State>().set_sending(false);
        let quiet = self.quiet_next.replace(false);
        let replays = self.replays_next.borrow_mut().take().as_deref() == Some(folder);
        let publish_next =
            self.publish_next.replace(false) && self.folder.borrow().as_deref() == Some(folder);
        let Some(login) = self.sending_login() else {
            return;
        };
        let before = history::submission(folder, &login.server);
        match result {
            Ok(Reply::Sent(answer)) => {
                let mut sent = before
                    .as_ref()
                    .filter(|old| old.id == answer.id)
                    .map_or_else(
                        || history::Sent::new(&answer, &login.server),
                        |old| old.answered(&answer),
                    );
                if !send::published(&sent) {
                    sent.queued.clear();
                    sent.retry_at = None;
                    self.stop_share(folder);
                }
                let done = send::done(before.as_ref(), &sent);
                match history::remember(folder, &sent) {
                    Ok(()) if publish_next && !send::published(&sent) => {
                        self.log(&format!("{done}: {}", sent.id));
                        if replays {
                            *self.replays_next.borrow_mut() = Some(folder.to_path_buf());
                        }
                        self.send_about(Call::Publish);
                    }
                    Ok(()) => {
                        self.log(&format!("{done}: {}", sent.id));
                        if !quiet {
                            self.toast(done);
                        }
                        if send::published(&sent) && replays {
                            self.queue_top_replays(folder);
                        }
                    }
                    Err(error) => {
                        self.send_failed(&format!("Could not keep the answer: {error:#}"))
                    }
                }
            }
            Ok(Reply::Deleted) => {
                self.stop_share(folder);
                self.forget_sent(folder);
                self.log("Deleted the scan from the Lost Scores server");
                self.toast("Deleted from the server");
            }
            Ok(Reply::Listed(_) | Reply::Replayed(_)) => {}
            Err(error) => match error.downcast_ref::<ApiError>() {
                Some(ApiError::LoginRequired) => {
                    self.login_expired();
                    self.toast(LOGIN_EXPIRED);
                }
                Some(ApiError::NotOnServer) => {
                    self.forget_sent(folder);
                    self.send_failed(&sentence(&error.to_string()));
                }
                _ if quiet => self.log(&format!(
                    "Could not save the scan to your profile: {}",
                    sentence(&format!("{error:#}"))
                )),
                _ => self.send_failed(&sentence(&format!("{error:#}"))),
            },
        }
        if self.folder.borrow().as_deref() == Some(folder) {
            self.show_sent();
        }
    }

    fn refresh_sent(&self) {
        let folder = self.folder.borrow().clone();
        let (Some(login), Some(folder)) = (self.sending_login(), folder) else {
            return;
        };
        if history::submission(&folder, &login.server).is_none() {
            return;
        }
        let epoch = self.sent_epoch.get();
        let courier = self.courier.borrow().clone();
        let weak = self.weak.clone();
        thread::spawn(move || {
            let result = courier(&login, Call::List).map_err(anyhow::Error::from);
            on_ui(&weak, move |app| app.listed(&folder, epoch, result));
        });
    }

    fn listed(&self, folder: &Path, epoch: u64, result: Result<Reply>) {
        if self.sent_epoch.get() != epoch {
            return;
        }
        let Some(login) = self.sending_login() else {
            return;
        };
        let Some(sent) = history::submission(folder, &login.server) else {
            return;
        };
        match result {
            Ok(Reply::Listed(listed)) => match send::reconcile(&sent, &listed) {
                Some(fresh) if fresh == sent => {}
                Some(fresh) => {
                    if let Err(error) = history::remember(folder, &fresh) {
                        self.log(&format!("Could not keep the scan's state: {error:#}"));
                    }
                }
                None => {
                    self.forget_sent(folder);
                    self.log("The sent scan is no longer on the Lost Scores server");
                }
            },
            Ok(_) => {}
            Err(error) if matches!(error.downcast_ref(), Some(ApiError::LoginRequired)) => {
                self.login_expired();
                return;
            }
            Err(error) => self.log(&format!("Could not check the sent scan: {error:#}")),
        }
        if self.folder.borrow().as_deref() == Some(folder) {
            self.show_sent();
        }
    }

    fn forget_sent(&self, folder: &Path) {
        if let Err(error) = history::forget(folder) {
            self.log(&format!("Could not forget the sent scan: {error:#}"));
        }
    }

    fn send_failed(&self, text: &str) {
        self.log(&format!("Sending failed: {text}"));
        self.window.global::<State>().set_status(text.into());
        self.toast_for(text, CATBOX_FAILED_FOR);
    }

    fn toast(&self, text: &str) {
        self.toast_for(text, COPIED_FOR);
    }

    fn toast_for(&self, text: &str, hold: Duration) {
        let state = self.window.global::<State>();
        state.set_copied_text(text.into());
        state.set_copied(true);
        let weak = self.weak.clone();
        self.copied
            .start(slint::TimerMode::SingleShot, hold, move || {
                if let Some(window) = weak.upgrade() {
                    window.global::<State>().set_copied(false);
                }
            });
    }

    fn put_on_clipboard(
        &self,
        text: String,
        done: impl FnOnce(&Rc<Session>, Result<(), String>) + Send + 'static,
    ) {
        let weak = self.weak.clone();
        self.clipboard
            .borrow()
            .send(Request::Text(text), move |result| {
                on_ui(&weak, move |app| done(app, result));
            });
    }

    fn shared_image(&self, index: usize) -> Option<ShareImage> {
        self.choices.borrow().get(index)
    }

    fn copy_image(&self, index: usize) {
        let Some(image) = self.shared_image(index) else {
            return;
        };
        let weak = self.weak.clone();
        self.clipboard
            .borrow()
            .send(Request::Image(image.path), move |result| {
                on_ui(&weak, move |app| match result {
                    Ok(()) => app.toast("Image copied"),
                    Err(reason) => {
                        app.log(&format!("Could not copy the image: {reason}"));
                        app.toast("Could not copy the image: see the log");
                    }
                });
            });
    }

    fn open_image(&self, index: usize) {
        let Some(image) = self.shared_image(index) else {
            return;
        };
        self.launch("the image", image.path.into_os_string());
    }

    fn launch(&self, what: &'static str, target: OsString) {
        self.launch_or(what, target, None);
    }

    fn launch_or(&self, what: &'static str, target: OsString, fallback: Option<String>) {
        let opener = self.opener.borrow().clone();
        let weak = self.weak.clone();
        thread::spawn(move || {
            if let Err(error) = opener(&target) {
                let reason = error.to_string();
                on_ui(&weak, move |app| {
                    app.open_failed(what, &reason, fallback.as_deref());
                });
            }
        });
    }

    fn open_failed(&self, what: &str, reason: &str, fallback: Option<&str>) {
        self.log(&format!("Could not open {what}: {reason}"));
        if let Some(line) = fallback {
            self.log(line);
        }
        self.toast(&format!("Could not open {what}: see the log"));
    }

    fn ask_upload_all(self: &Rc<Self>) {
        let state = self.window.global::<State>();
        if self.uploading.get() {
            self.toast("An upload is already running");
            return;
        }
        if self.consent.get().must_ask() {
            if self.choices.borrow_mut().hold_all() {
                state.set_upload_confirm_open(true);
            }
        } else {
            let images = self.choices.borrow().all().to_vec();
            if !images.is_empty() {
                self.upload_all(images);
            }
        }
    }

    fn confirm_upload(self: &Rc<Self>, remember: bool) {
        self.window.global::<State>().set_upload_confirm_open(false);
        let mut consent = self.consent.get();
        consent.accept(remember);
        self.consent.set(consent);
        if remember {
            self.change_settings(|settings| settings.catbox_confirmed = true);
        }
        let held = self.choices.borrow_mut().release();
        if !held.is_empty() {
            self.upload_all(held);
        }
    }

    fn cancel_upload(&self) {
        self.choices.borrow_mut().release();
        self.window.global::<State>().set_upload_confirm_open(false);
    }

    fn upload_all(self: &Rc<Self>, images: Vec<ShareImage>) {
        self.uploading.set(true);
        let total = images.len();
        let count = match total {
            1 => "1 picture".to_owned(),
            many => format!("{many} pictures"),
        };
        self.log(&format!("Uploading {count} to catbox..."));
        self.catbox_reset.stop();
        self.catbox_progress(&upload::progress_label(1, total));
        let weak = self.weak.clone();
        thread::spawn(move || {
            let agent = upload::agent();
            let mut results = Vec::with_capacity(total);
            for (index, image) in images.iter().enumerate() {
                let label = upload::progress_label(index + 1, total);
                on_ui(&weak, move |app| app.catbox_progress(&label));
                let result =
                    upload::upload(&agent, &image.path).map_err(|error| format!("{error:#}"));
                results.push((image.label, result));
            }
            on_ui(&weak, move |app| {
                app.uploading.set(false);
                app.uploaded_all(&results);
            });
        });
    }

    fn uploaded_all(&self, results: &[(&'static str, Result<String, String>)]) {
        for (label, result) in results {
            match result {
                Ok(link) => self.log(&format!("{label}: {link}")),
                Err(error) => self.log(&format!("Could not upload {label}: {error}")),
            }
        }
        let upload::Summary {
            links,
            message,
            label,
            tone,
            first_error,
        } = upload::links_summary(results);
        if links.is_empty() {
            self.finish_catbox(&label, tone, &first_error);
            self.toast(&message);
            return;
        }
        let shown = links.replace('\n', " ");
        self.put_on_clipboard(links, move |app, copied| match copied {
            Ok(()) => {
                app.finish_catbox(&label, tone, &first_error);
                app.toast(&message);
            }
            Err(error) => {
                app.log(&format!(
                    "Uploaded, but the links could not be copied: {error}. Links: {shown}"
                ));
                app.finish_catbox(upload::LINKS_IN_THE_LOG, -1, &error);
                app.toast("Uploaded: the links are in the log");
            }
        });
    }

    fn catbox_progress(&self, label: &str) {
        set_catbox(&self.window.global::<State>(), label, 0, true, "");
    }

    fn finish_catbox(&self, label: &str, tone: i32, error: &str) {
        set_catbox(&self.window.global::<State>(), label, tone, false, error);
        let hold = if tone > 0 {
            CATBOX_DONE_FOR
        } else {
            CATBOX_FAILED_FOR
        };
        let weak = self.weak.clone();
        self.catbox_reset
            .start(slint::TimerMode::SingleShot, hold, move || {
                if let Some(window) = weak.upgrade() {
                    set_catbox(&window.global::<State>(), "", 0, false, "");
                }
            });
    }

    fn include_in_place(&self, value: bool) {
        let tab = self.tab.get();
        let top = (-self.window.global::<State>().get_scroll_y()).max(0.0);
        let first = (top / ROW_HEIGHT) as usize;
        let offset = top - first as f32 * ROW_HEIGHT;
        let before = self.table.borrow().rows(tab).to_vec();
        self.table.borrow_mut().set_include_all(value);
        self.show_rows(tab);
        let anchor = anchor_index(&before, self.table.borrow().rows(tab), first);
        match anchor {
            Some(index) => self.jump_to(-(index as f32 * ROW_HEIGHT + offset), false),
            None => self.jump(0, false),
        }
    }

    fn init_mode(&self) {
        self.show_picked_mode();
    }

    fn pick_images(&self) {
        self.picked_images.set(true);
        self.show_images();
    }

    fn pick_table(&self, tab: Tab) {
        self.picked_images.set(false);
        self.show_tab(tab);
    }

    fn show_picked_mode(&self) {
        if self.picked_images.get() {
            self.show_images();
        } else {
            self.show_tab(self.tab.get());
        }
    }

    fn show_images(&self) {
        self.images_view.set(true);
        self.window.global::<State>().set_images_shown(true);
        self.refresh_previews();
    }

    fn show_tab(&self, tab: Tab) {
        self.images_view.set(false);
        self.window.global::<State>().set_images_shown(false);
        self.table.borrow_mut().clear_fresh();
        self.show_rows(tab);
        self.jump(0, false);
    }

    fn show_rows(&self, tab: Tab) {
        self.tab.set(tab);
        let state = self.window.global::<State>();
        state.set_tab(tab.index() as i32);
        state.set_foot(foot(&self.table.borrow(), tab));
        state.set_lost_count(self.table.borrow().rows(Tab::Lost).len() as i32);
        self.rows.show(tab);
        self.show_sorting();
        state.set_mods_width(mods_width(self.table.borrow().rows(tab)));
        self.reset_search();
    }

    fn reset_search(&self) {
        self.query.borrow_mut().clear();
        self.window.global::<State>().set_search_text("".into());
        self.rerun_search(false);
    }

    fn show_sorting(&self) {
        let (column, descending) = self.table.borrow().sorting(self.tab.get());
        let state = self.window.global::<State>();
        state.set_sort_column(column.index());
        state.set_sort_desc(descending);
    }

    fn sort_by(&self, column: Column) {
        let tab = self.tab.get();
        self.table.borrow_mut().sort(tab, column);
        self.rows.refresh();
        self.show_sorting();
        self.rerun_search(false);
        self.jump(0, false);
    }

    fn jump(&self, row: i32, animated: bool) {
        self.jump_to(-(row as f32 * ROW_HEIGHT), animated);
    }

    fn jump_to(&self, y: f32, animated: bool) {
        let state = self.window.global::<State>();
        state.set_jump_y(y);
        state.set_jump_animated(animated);
        state.set_jump_serial(state.get_jump_serial() + 1);
    }

    fn search_changed(&self, text: &str) {
        *self.query.borrow_mut() = text.to_owned();
        self.rerun_search(true);
    }

    fn rerun_search(&self, jump: bool) {
        let query = self.query.borrow().clone();
        let found =
            (!query.trim().is_empty()).then(|| self.table.borrow().search(self.tab.get(), &query));
        let label = match &found {
            None => String::new(),
            Some(rows) if rows.is_empty() => "nothing found".to_owned(),
            Some(rows) => format!("{} found", rows.len()),
        };
        self.rows.narrow(found);
        self.window.global::<State>().set_search_label(label.into());
        if jump {
            self.jump(0, false);
        }
    }

    fn copy_row(&self, index: usize) {
        let Some(source) = self.rows.source(index) else {
            return;
        };
        let text = self.table.borrow().copy_text(self.tab.get(), source);
        self.put_on_clipboard(text, |app, result| match result {
            Ok(()) => app.toast("Copied to clipboard"),
            Err(error) => app.log(&format!("Could not copy the row: {error}")),
        });
    }

    fn open_scan_folder(&self) {
        let folder = self.folder.borrow().clone();
        if let Some(folder) = folder {
            self.launch("the folder", folder.into_os_string());
        }
    }

    fn log(&self, text: &str) {
        self.add_log_line(&stamp(clock_now(), text));
    }

    fn add_log_line(&self, line: &str) {
        let (time, text) = crate::log::split(line);
        self.log.push(LogEntry {
            time: time.into(),
            text: text.into(),
        });
        while self.log.row_count() > crate::log::KEEP_LINES {
            self.log.remove(0);
        }
        let joined: Vec<String> = self
            .log
            .iter()
            .map(|entry| format!("{} {}", entry.time, entry.text).trim().to_owned())
            .collect();
        self.window
            .global::<State>()
            .set_log_text(joined.join("\n").into());
        if let Some(file) = &self.log_file {
            crate::log::append(file, line);
        }
    }

    fn close_popups(&self) {
        let state = self.window.global::<State>();
        state.set_scans_open(false);
    }

    fn open_keys(&self) {
        let state = self.window.global::<State>();
        let saved = self.saved_player();
        let from_config = saved.is_none();
        let name = saved.or_else(|| self.config_name()).unwrap_or_default();
        let credentials = self.credentials.borrow().clone();
        state.set_keys_user_from_config(from_config && !name.is_empty());
        state.set_keys_user(name.into());
        state.set_keys_id(credentials.as_ref().map_or("", |c| &c.client_id).into());
        state.set_keys_secret(credentials.as_ref().map_or("", |c| &c.client_secret).into());
        self.clear_key_errors(&state);
        self.close_popups();
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
        let account = parse_account(&user);
        let (_, shown) = account_key(&account);
        state.set_keys_busy(true);
        state.set_status(format!("Validating user '{shown}' and API keys...").into());
        let credentials = Credentials {
            client_id: id,
            client_secret: secret,
        };
        let weak = self.weak.clone();
        thread::spawn(move || {
            let result = validate(&credentials, &account);
            on_ui(&weak, move |app| {
                app.window.global::<State>().set_keys_busy(false);
                app.keys_checked(credentials, result);
            });
        });
    }

    fn keys_checked(self: &Rc<Self>, credentials: Credentials, result: Result<Profile, KeysError>) {
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
                *self.credentials.borrow_mut() = Some(credentials);
                self.login_epoch.set(self.login_epoch.get() + 1);
                state.set_logging_in(false);
                state.set_own_keys(true);
                state.set_keys_open(false);
                self.adopt_profile(&profile);
                self.log(&format!("Logged in as {}", profile.username));
            }
            Err(error) => state.set_keys_error(error.to_string().into()),
        }
    }

    fn change_player(self: &Rc<Self>, text: &str) {
        let state = self.window.global::<State>();
        let wanted = text.trim();
        if wanted.is_empty() || wanted == state.get_user_name().as_str() {
            state.set_editing_name(false);
            return;
        }
        let account = parse_account(wanted);
        let (_, shown) = account_key(&account);
        state.set_name_busy(true);
        state.set_status(format!("Looking up the player '{shown}'...").into());
        let credentials = self.credentials.borrow().clone();
        let token = self
            .login
            .borrow()
            .as_ref()
            .map(|login| login.token.clone());
        let weak = self.weak.clone();
        thread::spawn(move || {
            let result = look_up(credentials.as_ref(), token.as_deref(), &account);
            on_ui(&weak, move |app| app.player_found(result));
        });
    }

    fn player_found(self: &Rc<Self>, result: Result<Profile, Lookup>) {
        let state = self.window.global::<State>();
        state.set_name_busy(false);
        state.set_editing_name(false);
        match result {
            Ok(profile) => {
                state.set_status("".into());
                self.adopt_profile(&profile);
                self.log(&format!("Player: {}", profile.username));
            }
            Err(Lookup::Failed(error)) => state.set_status(error.into()),
            Err(Lookup::Expired) => {
                self.log(LOGIN_EXPIRED);
                self.login_expired();
            }
        }
    }

    fn adopt_profile(self: &Rc<Self>, profile: &Profile) {
        let state = self.window.global::<State>();
        let name = profile.username.clone();
        self.change_settings(|settings| settings.player = Some(name.clone()));
        self.epoch.set(self.epoch.get() + 1);
        self.show_guest();
        state.set_connected(true);
        state.set_user_name(name.into());
        self.show_profile(profile);
        self.fetch_avatar(profile.player_id);
    }

    fn show_profile(&self, profile: &Profile) {
        self.player.set(Some(profile.player_id));
        let state = self.window.global::<State>();
        state.set_perf_now(profile.texts.performance.clone().into());
        state.set_acc_now(profile.texts.accuracy.clone().into());
        state.set_rank(profile.texts.rank.clone().into());
        state.set_perf_potential("".into());
        state.set_acc_potential("".into());
        state.set_perf_tone(0);
        state.set_acc_tone(0);
        state.set_stats_ready(true);
        self.load_card(profile.player_id);
    }

    fn load_card(&self, player_id: u32) {
        let Some(dir) = history::scans_dir() else {
            return;
        };
        let weak = self.weak.clone();
        thread::spawn(move || {
            let entries = history::list(&dir);
            let saved = newest_of_player(&entries, player_id, history::load);
            on_ui(&weak, move |app| {
                let Some(potential) = saved.and_then(|scan| scan.potential) else {
                    return;
                };
                let state = app.window.global::<State>();
                if app.player.get() == Some(player_id) && state.get_perf_potential().is_empty() {
                    app.show_card(&potential);
                }
            });
        });
    }

    fn fetch_profile(self: &Rc<Self>, account: Account) {
        let weak = self.weak.clone();
        let epoch = self.epoch.get();
        let credentials = self.credentials.borrow().clone();
        let token = self
            .login
            .borrow()
            .as_ref()
            .map(|login| login.token.clone());
        thread::spawn(move || {
            let result = look_up(credentials.as_ref(), token.as_deref(), &account);
            on_ui(&weak, move |app| app.profile_arrived(epoch, result));
        });
    }

    fn profile_arrived(self: &Rc<Self>, epoch: u64, result: Result<Profile, Lookup>) {
        if self.epoch.get() == epoch {
            self.profile_fetched(result);
        }
    }

    fn profile_fetched(self: &Rc<Self>, result: Result<Profile, Lookup>) {
        match result {
            Ok(profile) => {
                let state = self.window.global::<State>();
                if state.get_logged_in() {
                    state.set_user_name(profile.username.clone().into());
                }
                self.show_profile(&profile);
                self.fetch_avatar(profile.player_id);
            }
            Err(Lookup::Expired) => {
                self.log(LOGIN_EXPIRED);
                self.login_expired();
            }
            Err(Lookup::Failed(error)) => {
                let state = self.window.global::<State>();
                state.set_perf_now("-".into());
                state.set_acc_now("-".into());
                state.set_rank("-".into());
                state.set_stats_ready(true);
                self.log(&error);
            }
        }
    }

    fn fetch_avatar(self: &Rc<Self>, player_id: u32) {
        let weak = self.weak.clone();
        thread::spawn(move || {
            let cache = cache::avatars_dir();
            let bytes = fetch(player_id, cache.as_deref()).ok();
            on_ui(&weak, move |app| app.avatar_arrived(player_id, bytes));
        });
    }

    fn avatar_arrived(&self, player_id: u32, bytes: Option<Vec<u8>>) {
        if let Some(bytes) = bytes
            && self.player.get() == Some(player_id)
        {
            *self.avatar.borrow_mut() = Some(bytes);
            self.rendered.set(0);
            self.refresh_avatar();
        }
    }

    fn show_guest(&self) {
        *self.avatar.borrow_mut() = None;
        self.rendered.set(0);
        self.refresh_avatar();
    }

    fn refresh_avatar(&self) {
        let pixels = avatar_pixels(AVATAR_LOGICAL, self.window.window().scale_factor());
        if pixels == self.rendered.get() {
            return;
        }
        let own = self
            .avatar
            .borrow()
            .as_deref()
            .map(|bytes| render_avatar(bytes, pixels));
        let rendered = match own {
            Some(Ok(buffer)) => Ok(buffer),
            Some(Err(error)) => {
                *self.avatar.borrow_mut() = None;
                self.log(&format!("Could not show the avatar: {error:#}"));
                render_avatar(GUEST, pixels)
            }
            None => render_avatar(GUEST, pixels),
        };
        if let Ok(buffer) = rendered {
            let state = self.window.global::<State>();
            state.set_avatar(slint::Image::from_rgba8(buffer));
            state.set_has_avatar(true);
            self.rendered.set(pixels);
        }
    }

    fn pick_folder(self: &Rc<Self>) {
        if self.dialog.swap(true, Ordering::Relaxed) {
            return;
        }
        let weak = self.weak.clone();
        let dialog = self.dialog.clone();
        let spawned = thread::Builder::new().spawn(move || {
            let picked = std::panic::catch_unwind(|| {
                rfd::FileDialog::new()
                    .set_title("Select osu! Game Directory")
                    .pick_folder()
            });
            dialog.store(false, Ordering::Relaxed);
            on_ui(&weak, move |app| match picked {
                Ok(Some(dir)) => app.folder_picked(&dir),
                Ok(None) => {}
                Err(_) => app.log("Could not open the folder dialog"),
            });
        });
        if spawned.is_err() {
            self.dialog.store(false, Ordering::Relaxed);
            self.log("Could not open the folder dialog");
        }
    }

    fn folder_picked(self: &Rc<Self>, dir: &Path) {
        let state = self.window.global::<State>();
        match choose_folder(dir) {
            Ok(path) => {
                self.settings.borrow_mut().osu_dir = Some(PathBuf::from(&path));
                state.set_osu_dir(path.into());
                state.set_status("".into());
                if !state.get_connected() {
                    self.connect_with_keys();
                }
            }
            Err(error) => state.set_status(error.to_string().into()),
        }
    }

    fn busy(&self) -> bool {
        self.scan.borrow().phase == Phase::Scanning || self.refreshing.get()
    }

    fn start_scan(self: &Rc<Self>) {
        let Some((job, access)) = self.prepare_scan() else {
            return;
        };
        self.begin_scan();
        let sink = self.sink();
        let queue = self.sink();
        thread::spawn(move || match access {
            Access::Keys(credentials) => {
                worker::run(&job, &mut Online::new(credentials), &sink);
            }
            Access::Login(token) => {
                let mut api = Shared::new()
                    .with_session(&token)
                    .on_wait(move |seconds, maps| {
                        queue.send(Message::Queue(seconds, maps));
                    });
                worker::run(&job, &mut api, &sink);
            }
        });
    }

    fn sink(&self) -> LoopSink {
        LoopSink {
            weak: self.weak.clone(),
            closed: self.closed.clone(),
        }
    }

    fn begin_scan(&self) {
        let check_missing = self.settings.borrow().check_missing;
        self.scan.borrow_mut().start(check_missing);
        let showing = self.window.global::<State>().get_show_results();
        self.opens_results
            .set(!self.window.get_two_columns() && !showing);
        self.sync();
    }

    fn prepare_scan(&self) -> Option<(Job, Access)> {
        self.commit_pending_scores();
        let state = self.window.global::<State>();
        let connected = state.get_connected();
        if !connected {
            state.set_status(NEEDS_ACCOUNT.into());
            return None;
        }
        if self.refreshing.get() || !self.scan.borrow().can_start(connected) {
            return None;
        }
        let access = match (
            self.credentials.borrow().clone(),
            self.login.borrow().as_ref(),
        ) {
            (Some(credentials), _) => Access::Keys(credentials),
            (None, Some(login)) => Access::Login(login.token.clone()),
            (None, None) => {
                state.set_status(NEEDS_ACCOUNT.into());
                return None;
            }
        };
        let osu_dir = state.get_osu_dir().to_string();
        if let Err(message) = check_folders(&osu_dir) {
            state.set_status(message.into());
            return None;
        }
        let Some(scans_dir) = history::scans_dir() else {
            state.set_status("Operation failed".into());
            self.log("Error: cannot find a folder for the scan history");
            return None;
        };
        let settings = self.settings.borrow();
        let job = Job {
            source: Source {
                osu_dir: Some(PathBuf::from(osu_dir)),
                player: Some(state.get_user_name().to_string()),
                songs_dir: None,
                check_missing: settings.check_missing,
                maps: None,
                stop_on_lookup_error: true,
            },
            scans_dir,
            include_all: settings.include_all,
            renderer: self
                .renderer
                .borrow()
                .clone()
                .unwrap_or_else(|| std::env::current_exe().unwrap_or_default()),
            share_count: usize::from(clamped_scores(settings.scores_to_show)),
            show_lost_in_top: settings.show_lost_in_top_image,
        };
        Some((job, access))
    }

    fn sync(&self) {
        let state = self.window.global::<State>();
        let scan = self.scan.borrow();
        state.set_status(scan.status.clone().into());
        state.set_progress(scan.progress);
        state.set_busy(self.busy());
        let fresh: Vec<String> = scan.log[self.shown.get()..].to_vec();
        self.shown.set(scan.log.len());
        drop(scan);
        for line in &fresh {
            self.add_log_line(line);
        }
    }

    fn handle(&self, message: Message) {
        match message {
            Message::Event(event) => self.scan.borrow_mut().apply(&event, clock_now()),
            Message::Queue(seconds, maps) => self.scan.borrow_mut().queued(seconds, maps),
            Message::Saving => self.scan.borrow_mut().saving(),
            Message::Finished(done) => {
                self.refreshing.set(true);
                self.counts
                    .set((done.scan.lost_total, done.scan.lost_ranked));
                self.show_results(&done.scan);
                self.images_view.set(true);
                self.register(&done.folder, &done.scan);
                self.offer(&done.folder, &done.scan);
            }
            Message::Refreshed(updated) => {
                self.refreshing.set(false);
                if let Some(done) = updated {
                    self.counts
                        .set((done.scan.lost_total, done.scan.lost_ranked));
                    self.show_results(&done.scan);
                    self.register(&done.folder, &done.scan);
                    self.offer(&done.folder, &done.scan);
                }
                self.refresh_shared();
                self.show_images();
                let (total, ranked) = self.counts.get();
                let images = self.window.global::<State>().get_has_images();
                self.scan.borrow_mut().finish(total, ranked, images);
                self.open_results_if_narrow();
                self.save_privately();
            }
            Message::Failed(error) => {
                self.refreshing.set(false);
                self.scan.borrow_mut().fail(&error, clock_now());
            }
            Message::LoginExpired => {
                self.refreshing.set(false);
                self.scan.borrow_mut().expired(clock_now());
                self.login_expired();
            }
        }
        self.sync();
    }

    fn open_results_if_narrow(&self) {
        let narrow = !self.window.get_two_columns();
        if self.opens_results.replace(false) && narrow {
            self.window.global::<State>().set_show_results(true);
        }
    }

    fn images_scrolled(&self, top: f32, visible: f32, content: f32) {
        self.images_scroll.set((top, visible, content));
        self.update_arrows();
    }

    fn update_arrows(&self) {
        let (top, visible, content) = self.images_scroll.get();
        let tops = self.picture_tops.borrow();
        let state = self.window.global::<State>();
        state.set_prev_picture_ok(step_target(&tops, top, visible, content, false).is_some());
        state.set_next_picture_ok(step_target(&tops, top, visible, content, true).is_some());
    }

    fn step_picture(&self, direction: i32) {
        let (top, visible, content) = self.images_scroll.get();
        let target = step_target(
            &self.picture_tops.borrow(),
            top,
            visible,
            content,
            direction > 0,
        );
        if let Some(target) = target {
            self.jump_to(-target, true);
        }
    }

    fn preview_room_changed(&self, room: f32) {
        self.preview_room.set(room);
        self.refresh_previews();
    }

    fn refresh_previews(&self) {
        let wanted = plan(
            self.choices.borrow().all(),
            self.preview_room.get(),
            self.window.window().scale_factor(),
        );
        if wanted == *self.preview_wanted.borrow() {
            return;
        }
        self.preview_epoch.set(self.preview_epoch.get() + 1);
        *self.preview_wanted.borrow_mut() = wanted.clone();
        if wanted == *self.preview_shown.borrow() {
            self.preview_timer.stop();
            return;
        }
        if wanted.is_empty() {
            self.preview_timer.stop();
            self.previews_ready(Vec::new());
            return;
        }
        self.preview_timer
            .start(slint::TimerMode::SingleShot, PREVIEW_DEBOUNCE, || {
                if let Some(app) = SESSION.with(|slot| slot.borrow().clone()) {
                    app.start_previews();
                }
            });
    }

    fn start_previews(&self) {
        let wanted = self.preview_wanted.borrow().clone();
        let epoch = self.preview_epoch.get();
        let weak = self.weak.clone();
        thread::spawn(move || {
            let made = render_all(&wanted);
            on_ui(&weak, move |app| app.previews_arrived(epoch, made));
        });
    }

    fn previews_arrived(
        &self,
        epoch: u64,
        made: Vec<(Wanted, Result<slint::SharedPixelBuffer<slint::Rgba8Pixel>>)>,
    ) {
        if epoch != self.preview_epoch.get() {
            return;
        }
        for (item, result) in &made {
            if let Err(error) = result {
                self.log(&format!(
                    "Could not show the {} preview: {error:#}",
                    item.label
                ));
            }
        }
        self.previews_ready(made);
    }

    fn previews_ready(
        &self,
        made: Vec<(Wanted, Result<slint::SharedPixelBuffer<slint::Rgba8Pixel>>)>,
    ) {
        let scale = self.window.window().scale_factor();
        let mut shown = Vec::new();
        let mut items = Vec::new();
        for (item, result) in made {
            let preview = match result {
                Ok(buffer) => Preview {
                    label: item.label.into(),
                    width: buffer.width() as f32 / scale,
                    height: buffer.height() as f32 / scale,
                    image: slint::Image::from_rgba8(buffer),
                    ready: true,
                },
                Err(_) => Preview {
                    label: item.label.into(),
                    image: slint::Image::default(),
                    width: 0.0,
                    height: 0.0,
                    ready: false,
                },
            };
            items.push(preview);
            shown.push(item);
        }
        let heights: Vec<f32> = items.iter().map(|item| item.height).collect();
        *self.picture_tops.borrow_mut() = picture_tops(&heights);
        *self.preview_shown.borrow_mut() = shown;
        self.window
            .global::<State>()
            .set_previews(ModelRc::new(VecModel::from(items)));
        self.update_arrows();
    }

    fn show_card(&self, potential: &PotentialResult) {
        let state = self.window.global::<State>();
        state.set_perf_now(thousands(potential.current_pp).into());
        state.set_perf_potential(thousands(potential.potential_pp).into());
        state.set_acc_now(format!("{:.2}%", potential.current_accuracy).into());
        state.set_acc_potential(format!("{:.2}%", potential.potential_accuracy).into());
        state.set_perf_tone(tone(potential.current_pp, potential.potential_pp, 0));
        state.set_acc_tone(tone(
            potential.current_accuracy,
            potential.potential_accuracy,
            2,
        ));
    }

    fn show_results(&self, scan: &history::SavedScan) {
        let Some(potential) = &scan.potential else {
            return;
        };
        self.show_card(potential);
        let state = self.window.global::<State>();
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
        let login = self
            .login
            .borrow()
            .clone()
            .or_else(|| setup::session_file().and_then(|path| stored::load(&path)));
        if let Some(path) = setup::credentials_file()
            && let Err(error) = keys::delete(&path)
        {
            self.log(&format!("Could not delete the saved keys: {error:#}"));
            return;
        }
        if let Some(path) = setup::session_file()
            && let Err(error) = stored::delete(&path)
        {
            self.log(&format!("Could not delete the saved login: {error:#}"));
            return;
        }
        *self.login.borrow_mut() = None;
        if let Some(login) = login {
            let revoke = self.revoker.borrow().clone();
            revoke(login);
        }
        self.disconnect();
        self.log("Logged out");
    }

    fn disconnect(&self) {
        let state = self.window.global::<State>();
        *self.credentials.borrow_mut() = None;
        self.change_settings(|settings| settings.player = None);
        state.set_own_keys(false);
        state.set_logged_in(false);
        self.epoch.set(self.epoch.get() + 1);
        self.player.set(None);
        self.show_guest();
        state.set_editing_name(false);
        state.set_stats_ready(false);
        state.set_connected(false);
        state.set_user_name("".into());
        self.show_sent();
    }
}

thread_local! {
    static SESSION: RefCell<Option<Rc<Session>>> = const { RefCell::new(None) };
}

fn set_catbox(state: &State, label: &str, tone: i32, busy: bool, error: &str) {
    state.set_catbox_label(label.into());
    state.set_catbox_tone(tone);
    state.set_catbox_busy(busy);
    state.set_catbox_error(error.into());
}

fn login_fallback(url: &str) -> String {
    format!("Open this address in a browser to log in: {url}")
}

fn on_ui(weak: &Weak<MainWindow>, action: impl FnOnce(&Rc<Session>) + Send + 'static) {
    let _ = weak.upgrade_in_event_loop(move |_| {
        if let Some(app) = SESSION.with(|slot| slot.borrow().clone()) {
            action(&app);
        }
    });
}

fn account_key(account: &Account) -> (UserKey, String) {
    match account {
        Account::Name(name) => (UserKey::Username, name.clone()),
        Account::Id(id) => (UserKey::Id, id.to_string()),
    }
}

fn validate(credentials: &Credentials, account: &Account) -> Result<Profile, KeysError> {
    let mut online = Online::new(credentials.clone());
    online.check().map_err(|error| keys_error(&error))?;
    let (key, value) = account_key(account);
    let player = online
        .account(key, &value)
        .map_err(|error| keys_error(&error))?;
    Ok(Profile::from_player(&player))
}

fn look_up(
    credentials: Option<&Credentials>,
    token: Option<&str>,
    account: &Account,
) -> Result<Profile, Lookup> {
    let (key, value) = account_key(account);
    let mut ask = |key: UserKey, value: &str| match (credentials, token) {
        (Some(credentials), _) => Online::new(credentials.clone()).account(key, value),
        (None, Some(token)) => Shared::new().with_session(token).account(key, value),
        (None, None) => Err(ApiError::LoginRequired),
    };
    by_id_then_name(key, &value, &mut ask)
        .map(|player| Profile::from_player(&player))
        .map_err(|error| match error {
            ApiError::LoginRequired => Lookup::Expired,
            error => Lookup::Failed(lookup_failure(&value, &error)),
        })
}

fn by_id_then_name(
    key: UserKey,
    value: &str,
    ask: &mut impl FnMut(UserKey, &str) -> Result<Player, ApiError>,
) -> Result<Player, ApiError> {
    match ask(key, value) {
        Err(ApiError::Status { status: 404, .. }) if key == UserKey::Id => {
            ask(UserKey::Username, value)
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    use lost_scores_app::history::{self, Sent};
    use lost_scores_app::setup;
    use osu_api::{ApiError, Credentials, Submission, SubmissionInfo, UserKey};
    use slint::{ComponentHandle, LogicalSize, Model};
    use tempfile::TempDir;

    use lost_scores_app::login::LoginError;
    use lost_scores_app::online::Player;
    use lost_scores_app::pipeline::PotentialResult;
    use lost_scores_app::session::{self as stored, Session as Login};

    use super::{Access, Lookup, Session, login_fallback};
    use crate::account::Profile;
    use crate::scan::{LOGIN_EXPIRED, NEEDS_ACCOUNT};
    use crate::send::{Call, Reply};
    use crate::share::render::ShareImage;
    use crate::table::Tab;
    use crate::worker::{Finished, Message};
    use crate::{MainWindow, State};

    static HOME: Mutex<(usize, Option<TempDir>)> = Mutex::new((0, None));
    static FILES: Mutex<()> = Mutex::new(());

    fn files() -> std::sync::MutexGuard<'static, ()> {
        FILES
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    struct Home;

    fn isolate_environment() -> Home {
        let mut state = HOME.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.0 == 0 {
            let home = tempfile::tempdir().unwrap();
            for (name, folder) in [
                ("XDG_CONFIG_HOME", "config"),
                ("XDG_DATA_HOME", "data"),
                ("XDG_CACHE_HOME", "cache"),
            ] {
                unsafe { std::env::set_var(name, home.path().join(folder)) };
            }
            unsafe { std::env::set_var("LOST_SCORES_SERVER", "http://127.0.0.1:9") };
            state.1 = Some(home);
        }
        state.0 += 1;
        Home
    }

    impl Drop for Home {
        fn drop(&mut self) {
            let mut state = HOME.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            state.0 -= 1;
            if state.0 == 0 {
                state.1 = None;
            }
        }
    }

    struct Fixture {
        dir: TempDir,
        _home: Home,
    }

    impl Fixture {
        fn path(&self) -> &Path {
            self.dir.path()
        }
    }

    fn osu_folder(root: &Path) -> String {
        let osu = root.join("osu");
        std::fs::create_dir_all(osu.join("Data/r")).unwrap();
        std::fs::create_dir_all(osu.join("Songs")).unwrap();
        osu.display().to_string()
    }

    fn session(width: f32) -> (MainWindow, std::rc::Rc<Session>, Fixture) {
        let home = isolate_environment();
        i_slint_backend_testing::init_no_event_loop();
        let window = MainWindow::new().unwrap();
        window.show().unwrap();
        window.window().set_size(LogicalSize::new(width, 800.0));
        slint::platform::update_timers_and_animations();
        let root = tempfile::tempdir().unwrap();
        let session = Session::create(&window);
        session.bind(&window.global::<State>());
        *session.credentials.borrow_mut() = Some(Credentials {
            client_id: "1".into(),
            client_secret: "x".into(),
        });
        let state = window.global::<State>();
        state.set_connected(true);
        state.set_user_name("Synthetic".into());
        state.set_osu_dir(osu_folder(root.path()).into());
        let root = Fixture {
            dir: root,
            _home: home,
        };
        (window, session, root)
    }

    fn png() -> Vec<u8> {
        let image = image::RgbaImage::from_pixel(16, 16, image::Rgba([200, 120, 40, 255]));
        let mut bytes = Vec::new();
        image
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        bytes
    }

    #[test]
    fn a_profile_fetched_before_a_login_or_logout_is_dropped() {
        let (window, session, _root) = session(650.0);
        let state = window.global::<State>();
        let stale = session.epoch.get();
        session.epoch.set(stale + 1);
        session.profile_arrived(
            stale,
            Err(Lookup::Failed("Could not reach the osu! API".into())),
        );
        assert_eq!(state.get_perf_now(), "");
        assert!(!state.get_stats_ready());
        session.profile_arrived(
            stale + 1,
            Err(Lookup::Failed("Could not reach the osu! API".into())),
        );
        assert_eq!(state.get_perf_now(), "-");
        assert!(state.get_stats_ready());
    }

    fn keyless(width: f32) -> (MainWindow, std::rc::Rc<Session>, Fixture) {
        let (window, session, root) = session(width);
        *session.credentials.borrow_mut() = None;
        *session.login.borrow_mut() = Some(login());
        let state = window.global::<State>();
        state.set_own_keys(false);
        state.set_logged_in(true);
        (window, session, root)
    }

    fn nobody(width: f32) -> (MainWindow, std::rc::Rc<Session>, Fixture) {
        let (window, session, root) = keyless(width);
        *session.login.borrow_mut() = None;
        let state = window.global::<State>();
        state.set_logged_in(false);
        state.set_connected(false);
        state.set_user_name("".into());
        (window, session, root)
    }

    fn login() -> Login {
        Login {
            token: "tok".into(),
            osu_id: 9,
            username: "Logged".into(),
            server: "http://127.0.0.1:9".into(),
        }
    }

    #[test]
    fn a_saved_login_for_another_server_is_not_restored() {
        let _files = files();
        let path = setup::session_file().unwrap();
        stored::save(
            &path,
            &Login {
                server: "https://elsewhere.test".into(),
                ..login()
            },
        )
        .unwrap();
        assert_eq!(super::stored_login(), None);
        stored::save(&path, &login()).unwrap();
        assert_eq!(super::stored_login(), Some(login()));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn a_login_that_finishes_after_keys_were_saved_is_dropped() {
        let _files = files();
        let (window, session, _root) = nobody(650.0);
        let state = window.global::<State>();
        state.set_logging_in(true);
        let epoch = session.login_epoch.get();
        let keys = Credentials {
            client_id: "1".into(),
            client_secret: "x".into(),
        };
        session.keys_checked(keys, Ok(profile("Keyed", 3)));
        assert!(!state.get_logging_in());
        session.login_finished(epoch, Ok(login()));
        assert!(state.get_own_keys());
        assert!(!state.get_logged_in());
        assert_eq!(state.get_user_name(), "Keyed");
        assert!(!setup::session_file().unwrap().exists());
        lost_scores_app::keys::delete(&setup::credentials_file().unwrap()).unwrap();
    }

    #[test]
    fn logging_out_revokes_a_session_left_by_the_command_line() {
        let _files = files();
        let (window, session, _root) = session(650.0);
        let path = setup::session_file().unwrap();
        stored::save(&path, &login()).unwrap();
        let revoked = std::sync::Arc::new(Mutex::new(Vec::new()));
        let seen = revoked.clone();
        *session.revoker.borrow_mut() =
            std::sync::Arc::new(move |login: Login| seen.lock().unwrap().push(login));
        session.log_out();
        assert_eq!(*revoked.lock().unwrap(), vec![login()]);
        assert!(!path.exists());
        assert!(!window.global::<State>().get_connected());
    }

    fn write_config(state: &State<'_>) {
        let osu = PathBuf::from(state.get_osu_dir().to_string());
        std::fs::write(osu.join("osu!.Someone.cfg"), "Username = FromConfig\r\n").unwrap();
    }

    fn profile(name: &str, id: u32) -> Profile {
        let avatars = lost_scores_app::cache::avatars_dir().unwrap();
        std::fs::create_dir_all(&avatars).unwrap();
        std::fs::write(avatars.join(id.to_string()), png()).unwrap();
        Profile::from_player(&Player {
            id,
            names: vec![name.into()],
            pp: Some(1.0),
            accuracy: Some(99.0),
            global_rank: Some(1),
        })
    }

    #[test]
    fn after_an_update_the_window_keeps_to_its_own_version_until_restart() {
        let (window, session, _root) = keyless(650.0);
        let state = window.global::<State>();
        let old = PathBuf::from("/programs/lost-scores-gui.old");
        *session.updated.borrow_mut() = Some("9.9.9".to_owned());
        *session.renderer.borrow_mut() = Some(old.clone());
        session.check_updates(true);
        assert!(!state.get_checking_updates());
        assert_eq!(
            last_log(&session),
            "Version 9.9.9 is installed; restart the program to use it"
        );
        let (job, _) = session.prepare_scan().unwrap();
        assert_eq!(job.renderer, old);
    }

    #[test]
    fn a_scan_with_a_session_goes_through_the_server() {
        let (_window, session, _root) = keyless(650.0);
        let (job, access) = session.prepare_scan().unwrap();
        assert!(matches!(access, Access::Login(token) if token == "tok"));
        assert_eq!(job.source.player.as_deref(), Some("Synthetic"));
        *session.login.borrow_mut() = None;
        assert!(session.prepare_scan().is_none());
    }

    #[test]
    fn a_scan_with_keys_asks_osu_itself() {
        let (_window, session, _root) = session(650.0);
        let (_, access) = session.prepare_scan().unwrap();
        assert!(matches!(access, Access::Keys(_)));
    }

    #[test]
    fn without_a_player_a_scan_asks_for_one() {
        let (window, session, _root) = keyless(650.0);
        let state = window.global::<State>();
        state.set_connected(false);
        assert!(session.prepare_scan().is_none());
        assert_eq!(state.get_status(), NEEDS_ACCOUNT);
    }

    #[test]
    fn a_saved_session_starts_logged_in_without_the_pen() {
        let (window, session, _root) = nobody(650.0);
        let state = window.global::<State>();
        session.restore(Some(login()), None);
        assert!(state.get_connected());
        assert!(state.get_logged_in());
        assert!(!state.get_own_keys());
        assert_eq!(state.get_user_name(), "Logged");
        assert_eq!(
            session.login.borrow().as_ref().map(|login| login.osu_id),
            Some(9)
        );
    }

    #[test]
    fn without_session_or_keys_the_window_shows_connect_even_with_an_osu_config() {
        let (window, session, _root) = nobody(650.0);
        let state = window.global::<State>();
        write_config(&state);
        session.restore(None, None);
        assert!(!state.get_connected());
        assert_eq!(state.get_user_name(), "");
        session.open_keys();
        assert_eq!(state.get_keys_user(), "FromConfig");
    }

    #[test]
    fn with_keys_the_player_comes_from_the_osu_config_and_wins_over_a_session() {
        let (window, session, _root) = nobody(650.0);
        let state = window.global::<State>();
        write_config(&state);
        let keys = Credentials {
            client_id: "1".into(),
            client_secret: "x".into(),
        };
        session.restore(Some(login()), Some(keys));
        assert!(state.get_connected());
        assert!(state.get_own_keys());
        assert!(!state.get_logged_in());
        assert_eq!(state.get_user_name(), "FromConfig");
    }

    #[test]
    fn a_finished_login_saves_the_session_and_shows_the_profile() {
        let _files = files();
        let (window, session, _root) = nobody(650.0);
        let state = window.global::<State>();
        state.set_logging_in(true);
        session.login_epoch.set(2);
        session.login_finished(1, Ok(login()));
        assert!(!state.get_connected());
        assert!(state.get_logging_in());
        session.login_finished(2, Ok(login()));
        assert!(!state.get_logging_in());
        assert!(state.get_connected());
        assert!(state.get_logged_in());
        assert_eq!(state.get_user_name(), "Logged");
        assert!(!state.get_stats_ready());
        let path = setup::session_file().unwrap();
        assert_eq!(stored::load(&path), Some(login()));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn a_login_keeps_its_address_out_of_the_log_when_the_browser_opens() {
        let (window, session, _root) = nobody(650.0);
        let (sent, received) = std::sync::mpsc::channel();
        *session.opener.borrow_mut() = std::sync::Arc::new(move |target| {
            sent.send(target.to_owned()).unwrap();
            Ok(())
        });
        session.log_in();
        let opened = received
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        assert!(opened.to_string_lossy().contains("/api/v1/auth/start"));
        let lines: Vec<String> = (0..session.log.row_count())
            .map(|i| session.log.row_data(i).unwrap().text.to_string())
            .collect();
        assert!(
            !lines.iter().any(|line| line.contains("/api/v1/auth/start")),
            "{lines:?}"
        );
        assert_eq!(
            window.global::<State>().get_status(),
            "Log in with osu! in the browser"
        );
    }

    #[test]
    fn a_browser_that_did_not_open_gets_the_login_address_in_the_log() {
        let (window, session, _root) = nobody(650.0);
        let address = "https://lost.test/api/v1/auth/start?challenge=00&port=1";
        session.open_failed(
            "the browser",
            "no browser installed",
            Some(&login_fallback(address)),
        );
        let lines: Vec<String> = (0..session.log.row_count())
            .map(|i| session.log.row_data(i).unwrap().text.to_string())
            .collect();
        let reason = lines
            .iter()
            .position(|line| line.contains("Could not open the browser: no browser installed"))
            .unwrap();
        assert!(lines[reason + 1].contains(address), "{lines:?}");
        assert_eq!(
            window.global::<State>().get_copied_text(),
            "Could not open the browser: see the log"
        );
    }

    #[test]
    fn a_failed_login_says_why_and_stays_on_connect() {
        let (window, session, _root) = nobody(650.0);
        let state = window.global::<State>();
        state.set_logging_in(true);
        session.login_finished(session.login_epoch.get(), Err(LoginError::Cancelled));
        assert!(!state.get_logging_in());
        assert!(!state.get_connected());
        assert_eq!(state.get_status(), "The login was cancelled on osu!");
    }

    #[test]
    fn logging_out_deletes_the_session_and_returns_to_connect() {
        let _files = files();
        let (window, session, _root) = nobody(650.0);
        let state = window.global::<State>();
        let path = setup::session_file().unwrap();
        stored::save(&path, &login()).unwrap();
        session.restore(Some(login()), None);
        session.log_out();
        assert!(!path.exists());
        assert!(session.login.borrow().is_none());
        assert!(!state.get_logged_in());
        assert!(!state.get_connected());
        assert_eq!(state.get_user_name(), "");
    }

    #[test]
    fn an_expired_session_returns_to_connect_with_a_message() {
        let _files = files();
        let (window, session, _root) = keyless(650.0);
        let state = window.global::<State>();
        let path = setup::session_file().unwrap();
        stored::save(&path, &login()).unwrap();
        begin(&session);
        session.handle(Message::LoginExpired);
        assert!(!state.get_connected());
        assert!(!state.get_logged_in());
        assert!(session.login.borrow().is_none());
        assert!(!path.exists());
        assert_eq!(state.get_status(), LOGIN_EXPIRED);
        stored::save(&path, &login()).unwrap();
        session.restore(Some(login()), None);
        state.set_status("".into());
        session.profile_arrived(session.epoch.get(), Err(Lookup::Expired));
        assert!(!state.get_connected());
        assert!(!path.exists());
        assert_eq!(state.get_status(), LOGIN_EXPIRED);
    }

    #[test]
    fn a_found_player_replaces_the_name_and_is_remembered() {
        let (window, session, _root) = keyless(650.0);
        let state = window.global::<State>();
        state.set_editing_name(true);
        session.player_found(Ok(profile("Other", 9)));
        assert!(!state.get_editing_name());
        assert_eq!(state.get_user_name(), "Other");
        assert_eq!(session.settings.borrow().player.as_deref(), Some("Other"));
    }

    #[test]
    fn a_player_that_is_not_found_leaves_the_previous_name() {
        let (window, session, _root) = keyless(650.0);
        let state = window.global::<State>();
        state.set_editing_name(true);
        session.player_found(Err(Lookup::Failed(
            "The osu! API does not know the player Nobody".into(),
        )));
        assert!(!state.get_editing_name());
        assert_eq!(state.get_user_name(), "Synthetic");
        assert_eq!(
            state.get_status(),
            "The osu! API does not know the player Nobody"
        );
    }

    #[test]
    fn an_empty_or_unchanged_name_closes_the_field_without_a_lookup() {
        let (window, _session, _root) = keyless(650.0);
        let state = window.global::<State>();
        for text in ["   ", "Synthetic"] {
            state.set_editing_name(true);
            state.invoke_change_player(text.into());
            assert!(!state.get_editing_name(), "{text}");
            assert!(!state.get_name_busy(), "{text}");
        }
    }

    #[test]
    fn digits_are_tried_as_an_id_first_then_as_a_name() {
        let player = |id| Player {
            id,
            names: vec!["Found".into()],
            pp: None,
            accuracy: None,
            global_rank: None,
        };
        let mut asked = Vec::new();
        let found = super::by_id_then_name(UserKey::Id, "727", &mut |key, value: &str| {
            asked.push((key, value.to_owned()));
            match key {
                UserKey::Id => Err(ApiError::Status {
                    status: 404,
                    url: "u".into(),
                }),
                UserKey::Username => Ok(player(5)),
            }
        });
        assert_eq!(found.unwrap().id, 5);
        assert_eq!(
            asked,
            [
                (UserKey::Id, "727".to_owned()),
                (UserKey::Username, "727".to_owned())
            ]
        );
        let mut calls = 0;
        let by_id = super::by_id_then_name(UserKey::Id, "727", &mut |_, _: &str| {
            calls += 1;
            Ok(player(727))
        });
        assert_eq!((by_id.unwrap().id, calls), (727, 1));
    }

    #[test]
    fn logging_out_with_keys_removes_them_and_returns_to_the_connect_card() {
        let _files = files();
        let (window, session, _root) = session(650.0);
        let state = window.global::<State>();
        state.set_own_keys(true);
        session.settings.borrow_mut().player = Some("Synthetic".into());
        session.log_out();
        assert!(session.credentials.borrow().is_none());
        assert!(!state.get_own_keys());
        assert!(!state.get_connected());
        assert_eq!(state.get_user_name(), "");
        assert_eq!(session.settings.borrow().player, None);
    }

    #[test]
    fn logging_out_without_keys_returns_to_the_connect_card_too() {
        let _files = files();
        let (window, session, _root) = keyless(650.0);
        let state = window.global::<State>();
        session.log_out();
        assert!(!state.get_connected());
        assert_eq!(state.get_user_name(), "");
    }

    #[test]
    fn a_wait_in_the_shared_queue_reaches_the_status_line() {
        let (window, session, _root) = keyless(650.0);
        begin(&session);
        session.handle(Message::Queue(42, None));
        assert_eq!(
            window.global::<State>().get_status(),
            "Waiting in the shared queue: 42 s"
        );
    }

    fn middle_pixel(state: &State<'_>) -> (u8, u8, u8) {
        let image = state.get_avatar();
        let buffer = image.to_rgba8().unwrap();
        let size = buffer.width() as usize;
        let pixel = buffer.as_slice()[size / 2 * size + size / 2];
        (pixel.r, pixel.g, pixel.b)
    }

    #[test]
    fn the_guest_avatar_stands_in_until_the_player_avatar_arrives() {
        let _files = files();
        let (window, session, _root) = session(650.0);
        let state = window.global::<State>();
        session.refresh_avatar();
        assert!(state.get_has_avatar());
        let (r, g, b) = middle_pixel(&state);
        assert!(r.abs_diff(g) < 12 && g.abs_diff(b) < 12, "{r} {g} {b}");
        session.player.set(Some(2));
        session.avatar_arrived(2, Some(png()));
        assert_eq!(middle_pixel(&state), (200, 120, 40));
        session.log_out();
        let (r, g, b) = middle_pixel(&state);
        assert!(r.abs_diff(g) < 12 && g.abs_diff(b) < 12, "{r} {g} {b}");
    }

    #[test]
    fn an_avatar_for_another_player_is_dropped() {
        let (window, session, _root) = session(650.0);
        let state = window.global::<State>();
        session.player.set(Some(2));
        session.avatar_arrived(1, Some(png()));
        assert!(!state.get_has_avatar());
        assert!(session.avatar.borrow().is_none());
        session.avatar_arrived(2, Some(png()));
        assert!(state.get_has_avatar());
        *session.avatar.borrow_mut() = None;
        session.player.set(None);
        session.avatar_arrived(2, Some(png()));
        assert!(session.avatar.borrow().is_none());
    }

    #[test]
    fn scores_to_show_typed_above_the_limit_becomes_the_limit() {
        let _files = files();
        let (window, session, _root) = nobody(650.0);
        let state = window.global::<State>();
        state.set_scores_pending("999".into());
        session.commit_pending_scores();
        assert_eq!(state.get_scores_to_show(), 250);
        assert_eq!(session.settings.borrow().scores_to_show, 250);
    }

    #[test]
    fn start_scan_applies_the_pending_scores_to_show() {
        let (window, session, _root) = session(650.0);
        let state = window.global::<State>();
        state.set_scores_pending("3".into());
        let (job, _) = session.prepare_scan().unwrap();
        assert_eq!(job.share_count, 3);
        assert_eq!(session.settings.borrow().scores_to_show, 3);
        assert_eq!(state.get_scores_to_show(), 3);
        let saved = setup::load_settings(&setup::config_file().unwrap());
        assert_eq!(saved.scores_to_show, 3);
        assert_eq!(state.get_scores_pending(), "");
    }

    #[test]
    fn start_scan_resets_a_pending_value_that_is_not_valid() {
        let (window, session, _root) = session(650.0);
        let state = window.global::<State>();
        session.settings.borrow_mut().scores_to_show = 7;
        state.set_scores_pending("abc".into());
        let (job, _) = session.prepare_scan().unwrap();
        assert_eq!(job.share_count, 7);
        assert_eq!(state.get_scores_pending(), "");
    }

    fn begin(session: &Session) {
        session.prepare_scan().unwrap();
        session.begin_scan();
    }

    fn finish(session: &Session) {
        session.handle(Message::Refreshed(None));
    }

    #[test]
    fn a_scan_started_in_one_column_opens_the_results_when_it_ends() {
        let (window, session, _root) = session(650.0);
        assert!(!window.get_two_columns());
        begin(&session);
        assert!(!window.global::<State>().get_show_results());
        finish(&session);
        assert!(window.global::<State>().get_show_results());
        assert!(!window.window().is_maximized());
    }

    #[test]
    fn two_columns_do_not_open_the_results_view() {
        let (window, session, _root) = session(1600.0);
        assert!(window.get_two_columns());
        begin(&session);
        finish(&session);
        assert!(!window.global::<State>().get_show_results());
    }

    fn shared(session: &Session, root: &Path) -> Vec<PathBuf> {
        let images: Vec<ShareImage> = ["lost_scores.png", "badge.png"]
            .into_iter()
            .map(|file| ShareImage {
                label: "x",
                path: root.join(file),
            })
            .collect();
        let paths = images.iter().map(|image| image.path.clone()).collect();
        session.choices.borrow_mut().replace(images);
        paths
    }

    #[test]
    fn open_hands_the_image_path_to_the_opener_off_the_ui_thread() {
        let (_window, session, root) = session(650.0);
        let paths = shared(&session, root.path());
        let (sent, received) = std::sync::mpsc::channel();
        let ui = std::thread::current().id();
        *session.opener.borrow_mut() = std::sync::Arc::new(move |target| {
            sent.send((target.to_owned(), std::thread::current().id()))
                .unwrap();
            Ok(())
        });
        session.open_image(1);
        session.open_image(0);
        session.open_image(9);
        let timeout = std::time::Duration::from_secs(5);
        let mut seen = vec![received.recv_timeout(timeout).unwrap()];
        seen.push(received.recv_timeout(timeout).unwrap());
        seen.sort_by(|a, b| a.0.cmp(&b.0));
        assert!(seen.iter().all(|(_, thread)| *thread != ui));
        let mut expected = vec![paths[0].clone(), paths[1].clone()];
        expected.sort();
        let targets: Vec<_> = seen.into_iter().map(|(target, _)| target).collect();
        assert_eq!(targets, expected);
        assert!(
            received
                .recv_timeout(std::time::Duration::from_millis(200))
                .is_err()
        );
    }

    #[test]
    fn the_footer_opens_the_hall_and_the_profile_on_the_site() {
        let (window, session, _root) = keyless(650.0);
        let (sent, received) = std::sync::mpsc::channel();
        *session.opener.borrow_mut() = std::sync::Arc::new(move |target| {
            sent.send(target.to_owned()).unwrap();
            Ok(())
        });
        let state = window.global::<State>();
        session.bind_extras(&state);
        state.invoke_open_hall();
        state.invoke_open_profile();
        let timeout = std::time::Duration::from_secs(5);
        let mut opened: Vec<String> = (0..2)
            .map(|_| {
                received
                    .recv_timeout(timeout)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        opened.sort();
        assert_eq!(
            opened,
            [
                format!("https://lost.lemon4ik.kz/rankings?me={}", login().osu_id),
                format!("https://lost.lemon4ik.kz/u/{}", login().osu_id),
            ]
        );
    }

    #[test]
    fn a_slow_opener_does_not_hold_the_window() {
        let (_window, session, root) = session(650.0);
        shared(&session, root.path());
        let (release, wait) = std::sync::mpsc::channel::<()>();
        let wait = std::sync::Mutex::new(wait);
        *session.opener.borrow_mut() = std::sync::Arc::new(move |_| {
            let _ = wait
                .lock()
                .unwrap()
                .recv_timeout(std::time::Duration::from_secs(10));
            Ok(())
        });
        let started = std::time::Instant::now();
        session.open_image(0);
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        release.send(()).unwrap();
    }

    #[test]
    fn a_sheet_that_opens_closes_the_popups_under_it() {
        let (window, session, _root) = session(1600.0);
        let state = window.global::<State>();
        session.bind_extras(&state);
        let asks: [fn(&State); 4] = [
            |state| state.invoke_ask_logout(),
            |state| state.invoke_ask_clear_cache(),
            |state| state.invoke_open_about(),
            |state| state.invoke_open_keys(),
        ];
        for (index, ask) in asks.into_iter().enumerate() {
            state.set_scans_open(true);
            ask(&state);
            assert!(!state.get_scans_open(), "ask {index}");
        }
        assert!(state.get_confirm_open() && state.get_cache_confirm_open());
        assert!(state.get_about_open() && state.get_keys_open());
    }

    #[test]
    fn a_failed_upload_marks_the_button_with_the_reason_until_it_resets() {
        let (window, session, _root) = session(650.0);
        let state = window.global::<State>();
        state.set_catbox_busy(true);
        session.uploaded_all(&[("Badge", Err("cannot reach catbox".to_owned()))]);
        assert_eq!(state.get_catbox_label(), "Upload failed");
        assert_eq!(state.get_catbox_tone(), -1);
        assert!(!state.get_catbox_busy());
        assert_eq!(state.get_catbox_error(), "Badge: cannot reach catbox");
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_secs(5));
        assert_eq!(state.get_catbox_label(), "Upload failed");
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_secs(2));
        assert_eq!(state.get_catbox_label(), "");
        assert_eq!(state.get_catbox_tone(), 0);
        assert_eq!(state.get_catbox_error(), "");
    }

    #[test]
    fn a_failed_open_is_logged_and_toasted() {
        let (window, session, _root) = session(650.0);
        session.open_failed("the image", "no viewer installed", None);
        let state = window.global::<State>();
        assert!(state.get_copied());
        assert_eq!(
            state.get_copied_text(),
            "Could not open the image: see the log"
        );
        let last = session.log.row_data(session.log.row_count() - 1).unwrap();
        assert!(
            last.text
                .contains("Could not open the image: no viewer installed")
        );
    }

    #[test]
    fn a_finished_scan_reports_ready_images_only_when_the_folder_has_them() {
        let (window, session, root) = session(650.0);
        let state = window.global::<State>();
        let folder = root.path().join("scan");
        std::fs::create_dir_all(&folder).unwrap();
        *session.folder.borrow_mut() = Some(folder.clone());
        begin(&session);
        finish(&session);
        assert!(!state.get_status().contains("images ready"));
        std::fs::write(folder.join("lost_scores.png"), png()).unwrap();
        begin(&session);
        finish(&session);
        assert!(
            state
                .get_status()
                .ends_with("on ranked maps \u{b7} images ready"),
            "{}",
            state.get_status()
        );
    }

    fn let_the_panel_open() {
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(300));
        slint::platform::update_timers_and_animations();
    }

    fn nudge_the_width(window: &MainWindow, width: f32) {
        window
            .window()
            .set_size(LogicalSize::new(width + 10.0, 800.0));
        let_the_panel_open();
        window.window().set_size(LogicalSize::new(width, 800.0));
        let_the_panel_open();
    }

    fn picture(path: &Path, width: u32, height: u32) {
        image::RgbaImage::from_fn(width, height, |x, y| {
            image::Rgba([(x % 251) as u8, (y % 241) as u8, 60, 255])
        })
        .save(path)
        .unwrap();
    }

    fn scan_folder(root: &Path, with_images: bool) -> PathBuf {
        let folder = root.join("scan");
        std::fs::create_dir_all(&folder).unwrap();
        if with_images {
            picture(&folder.join("lost_scores.png"), 1960, 1200);
            picture(&folder.join("potential_top.png"), 1960, 900);
            picture(&folder.join("summary_badge.png"), 1040, 280);
        }
        folder
    }

    fn open_scan(session: &Session, folder: &Path) {
        *session.folder.borrow_mut() = Some(folder.to_path_buf());
        session.refresh_shared();
    }

    fn make_previews(session: &Session) {
        let wanted = session.preview_wanted.borrow().clone();
        let epoch = session.preview_epoch.get();
        session.previews_arrived(epoch, crate::previews::render_all(&wanted));
    }

    fn saved_scan() -> lost_scores_app::history::SavedScan {
        lost_scores_app::history::SavedScan {
            version: lost_scores_app::history::VERSION,
            made_at: 1_794_649_471,
            player_id: 1,
            player_name: "Player".into(),
            include_all: false,
            check_missing: false,
            lost: Vec::new(),
            lost_total: 0,
            lost_ranked: 0,
            potential: None,
        }
    }

    #[test]
    fn a_scan_started_in_one_column_opens_the_results_on_the_images() {
        let (window, session, root) = session(650.0);
        let state = window.global::<State>();
        open_scan(&session, &scan_folder(root.path(), true));
        begin(&session);
        finish(&session);
        assert!(state.get_show_results());
        assert!(state.get_images_shown());
        assert!(session.images_view.get());
    }

    #[test]
    fn two_columns_switch_the_right_panel_to_the_images() {
        let (window, session, root) = session(1600.0);
        let state = window.global::<State>();
        open_scan(&session, &scan_folder(root.path(), true));
        begin(&session);
        finish(&session);
        assert!(!state.get_show_results());
        assert!(state.get_images_shown());
    }

    fn with_layout(width: f32) -> (MainWindow, std::rc::Rc<Session>, Fixture) {
        let (window, session, root) = session(width);
        crate::layout::bind(&window);
        (window, session, root)
    }

    #[test]
    fn the_default_mode_is_images_in_one_column() {
        let (window, session, _root) = session(650.0);
        session.init_mode();
        assert!(window.global::<State>().get_images_shown());
        assert!(session.picked_images.get());
    }

    #[test]
    fn the_default_mode_is_images_in_two_columns() {
        let (window, session, _root) = session(1600.0);
        session.init_mode();
        assert!(window.global::<State>().get_images_shown());
        assert!(session.picked_images.get());
    }

    #[test]
    fn a_picked_table_is_kept_by_back_and_see_full_results() {
        let (window, _session, _root) = with_layout(650.0);
        let state = window.global::<State>();
        state.invoke_set_tab(0);
        assert!(!state.get_images_shown());
        state.invoke_see_full_results();
        assert!(state.get_show_results());
        assert!(!state.get_images_shown());
        state.invoke_back();
        state.invoke_see_full_results();
        assert!(!state.get_images_shown());
    }

    #[test]
    fn a_picked_images_is_kept_by_back_and_see_full_results() {
        let (window, session, _root) = with_layout(650.0);
        let state = window.global::<State>();
        state.invoke_set_tab(1);
        state.invoke_show_images();
        assert!(session.picked_images.get());
        state.invoke_see_full_results();
        state.invoke_back();
        state.invoke_see_full_results();
        assert!(state.get_images_shown());
    }

    #[test]
    fn a_scan_that_ends_shows_images_without_changing_the_pick() {
        let (window, session, root) = with_layout(650.0);
        let state = window.global::<State>();
        state.invoke_set_tab(2);
        assert!(!session.picked_images.get());
        begin(&session);
        scan_ends(&session, &scan_folder(root.path(), true));
        assert!(state.get_images_shown());
        assert!(!session.picked_images.get());
        state.invoke_back();
        state.invoke_see_full_results();
        assert!(!state.get_images_shown());
    }

    #[test]
    fn after_a_scan_back_and_see_full_results_open_the_images_by_default() {
        let (window, session, root) = with_layout(650.0);
        let state = window.global::<State>();
        begin(&session);
        scan_ends(&session, &scan_folder(root.path(), true));
        state.invoke_back();
        state.invoke_see_full_results();
        assert!(state.get_images_shown());
    }

    fn scan_ends(session: &Session, folder: &Path) {
        session.handle(Message::Finished(Box::new(Finished {
            scan: saved_scan(),
            folder: folder.to_path_buf(),
        })));
        session.handle(Message::Refreshed(None));
    }

    #[test]
    fn a_scan_that_ends_leaves_a_table_mode_for_the_images_in_one_column() {
        let (window, session, root) = session(650.0);
        let state = window.global::<State>();
        session.show_tab(Tab::Potential);
        assert!(!state.get_images_shown());
        begin(&session);
        scan_ends(&session, &scan_folder(root.path(), true));
        assert!(state.get_show_results());
        assert!(state.get_images_shown());
    }

    #[test]
    fn a_scan_that_ends_with_the_overlay_already_open_switches_it_to_the_images() {
        let (window, session, root) = session(650.0);
        let state = window.global::<State>();
        session.show_tab(Tab::Online);
        state.set_show_results(true);
        begin(&session);
        scan_ends(&session, &scan_folder(root.path(), true));
        assert!(state.get_show_results());
        assert!(state.get_images_shown());
    }

    #[test]
    fn a_scan_that_ends_in_two_columns_switches_the_right_panel_to_the_images() {
        let (window, session, root) = session(1600.0);
        let state = window.global::<State>();
        session.show_tab(Tab::Lost);
        begin(&session);
        scan_ends(&session, &scan_folder(root.path(), true));
        assert!(!state.get_show_results());
        assert!(state.get_images_shown());
    }

    #[test]
    fn a_scan_that_ends_without_pictures_still_opens_the_images_mode() {
        let (window, session, root) = session(650.0);
        let state = window.global::<State>();
        session.show_tab(Tab::Lost);
        begin(&session);
        scan_ends(&session, &scan_folder(root.path(), false));
        assert!(state.get_images_shown());
        assert!(!state.get_has_images());
    }

    #[test]
    fn a_scan_that_fails_leaves_the_mode_as_it_was() {
        let (window, session, _root) = session(1600.0);
        let state = window.global::<State>();
        session.show_tab(Tab::Lost);
        begin(&session);
        session.handle(Message::Failed("the scan failed".into()));
        assert!(!state.get_images_shown());
    }

    #[test]
    fn the_previews_come_in_the_order_badge_lost_scores_potential_top() {
        let (window, session, root) = session(650.0);
        let state = window.global::<State>();
        open_scan(&session, &scan_folder(root.path(), true));
        session.preview_room_changed(584.0);
        make_previews(&session);
        let labels: Vec<_> = (0..state.get_previews().row_count())
            .map(|index| {
                state
                    .get_previews()
                    .row_data(index)
                    .unwrap()
                    .label
                    .to_string()
            })
            .collect();
        assert_eq!(labels, ["Badge", "Lost Scores", "Potential Top"]);
    }

    #[test]
    fn next_from_the_top_lands_on_the_second_picture_and_previous_is_off_there() {
        let (window, session, root) = session(650.0);
        window
            .window()
            .dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged {
                scale_factor: 1.25,
            });
        window.window().set_size(LogicalSize::new(650.0, 740.0));
        let state = window.global::<State>();
        state.set_has_results(true);
        state.set_show_results(true);
        state.set_images_shown(true);
        let_the_panel_open();
        open_scan(&session, &scan_folder(root.path(), true));
        make_previews(&session);
        let_the_panel_open();
        let tops = session.picture_tops.borrow().clone();
        assert_eq!(tops.len(), 3);
        let (top, visible, content) = session.images_scroll.get();
        assert_eq!(top, 0.0);
        assert!(content > visible, "{content} {visible}");
        assert!(!state.get_prev_picture_ok());
        assert!(state.get_next_picture_ok());

        session.step_picture(-1);
        assert_eq!(state.get_jump_y(), 0.0);
        session.step_picture(1);
        assert_eq!(state.get_jump_y(), -tops[1]);
        assert!(state.get_jump_animated());
        let_the_panel_open();
        let_the_panel_open();
        let (top, _, _) = session.images_scroll.get();
        assert_eq!(top, tops[1]);
        assert!(state.get_prev_picture_ok());
    }

    #[test]
    fn choosing_another_scan_keeps_the_tab_the_user_is_on() {
        let (window, session, root) = session(1600.0);
        let state = window.global::<State>();
        let folder = scan_folder(root.path(), true);
        session.show_images();
        session.show_scan(0, Some(folder.clone()), &saved_scan());
        assert!(state.get_images_shown());
        session.show_tab(Tab::Online);
        session.show_scan(0, Some(folder), &saved_scan());
        assert!(!state.get_images_shown());
        assert_eq!(state.get_tab(), Tab::Online.index() as i32);
    }

    fn drawn_previews(window: &MainWindow, scale: f32) -> Vec<(f32, f32, (u32, u32))> {
        let state = window.global::<State>();
        let mut seen = Vec::new();
        for index in 0..state.get_previews().row_count() {
            let item = state.get_previews().row_data(index).unwrap();
            let size = item.image.size();
            assert!(item.ready);
            let pixels = (size.width, size.height);
            assert_eq!(
                (item.width * scale).round() as u32,
                pixels.0,
                "{}",
                item.label
            );
            assert_eq!(
                (item.height * scale).round() as u32,
                pixels.1,
                "{}",
                item.label
            );
            seen.push((item.width, item.height, pixels));
        }
        seen
    }

    #[test]
    fn each_preview_is_as_many_pixels_as_it_is_drawn_at_in_one_column() {
        each_preview_is_as_many_pixels_as_it_is_drawn_at(650.0);
    }

    #[test]
    fn each_preview_is_as_many_pixels_as_it_is_drawn_at_in_two_columns() {
        each_preview_is_as_many_pixels_as_it_is_drawn_at(1600.0);
    }

    fn each_preview_is_as_many_pixels_as_it_is_drawn_at(width: f32) {
        {
            let (window, session, root) = session(width);
            window
                .window()
                .dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged {
                    scale_factor: 1.25,
                });
            window.window().set_size(LogicalSize::new(width, 800.0));
            let state = window.global::<State>();
            state.set_has_results(true);
            state.set_show_results(true);
            state.set_images_shown(true);
            nudge_the_width(&window, width);
            let room = session.preview_room.get();
            let panel = i_slint_backend_testing::ElementHandle::find_by_element_type_name(
                &window,
                "ResultsPanel",
            )
            .next()
            .unwrap();
            assert_eq!(room, panel.size().width - 66.0, "{width}");
            open_scan(&session, &scan_folder(root.path(), true));
            make_previews(&session);

            let big = (980.0_f32.min(room) * 1.25).round() as u32;
            let badge = (520.0_f32.min(room) * 1.25).round() as u32;
            let seen = drawn_previews(&window, 1.25);
            assert_eq!(seen.len(), 3, "{width}");
            assert_eq!(seen[0].2.0, badge, "{width} badge");
            assert_eq!(seen[1].2.0, big, "{width} lost");
            assert_eq!(seen[2].2.0, big, "{width} potential");
            assert_eq!(
                seen[1].2.1,
                (1200.0 * f64::from(big) / 1960.0).round() as u32
            );
            assert_eq!(
                seen[0].2.1,
                (280.0 * f64::from(badge) / 1040.0).round() as u32
            );
        }
    }

    #[test]
    fn the_images_are_drawn_at_their_own_logical_size() {
        let (window, session, root) = session(650.0);
        window
            .window()
            .dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged {
                scale_factor: 1.25,
            });
        window.window().set_size(LogicalSize::new(650.0, 800.0));
        let state = window.global::<State>();
        state.set_has_results(true);
        state.set_show_results(true);
        state.set_images_shown(true);
        let_the_panel_open();
        open_scan(&session, &scan_folder(root.path(), true));
        make_previews(&session);
        let_the_panel_open();
        let expected = drawn_previews(&window, 1.25);
        let handles = i_slint_backend_testing::ElementHandle::find_by_element_type_name;
        let tops: Vec<f32> = handles(&window, "PreviewCard")
            .map(|card| card.absolute_position().y + crate::previews::CARD_HEAD)
            .collect();
        assert!(!tops.is_empty());
        let images: Vec<_> = handles(&window, "Image").collect();
        let mut checked = 0;
        for (index, top) in tops.iter().enumerate() {
            let Some(drawn) = images.iter().find(|image| {
                (image.absolute_position().y - top).abs() < 0.5 && image.size().width > 100.0
            }) else {
                continue;
            };
            let size = drawn.size();
            assert_eq!(
                (size.width, size.height),
                (expected[index].0, expected[index].1),
                "picture {index}"
            );
            checked += 1;
        }
        assert!(checked >= 2);
    }

    #[test]
    fn a_new_width_asks_for_new_previews_after_a_pause_and_drops_the_old_ones() {
        let (window, session, root) = session(650.0);
        window
            .window()
            .dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged {
                scale_factor: 1.25,
            });
        let state = window.global::<State>();
        open_scan(&session, &scan_folder(root.path(), true));
        session.preview_room_changed(584.0);
        let first_epoch = session.preview_epoch.get();
        let first = crate::previews::render_all(&session.preview_wanted.borrow());
        assert!(session.preview_timer.running());
        assert_eq!(state.get_previews().row_count(), 0);

        session.preview_room_changed(500.0);
        assert!(session.preview_epoch.get() > first_epoch);
        session.previews_arrived(first_epoch, first);
        assert_eq!(state.get_previews().row_count(), 0);
        make_previews(&session);
        let widths: Vec<_> = drawn_previews(&window, 1.25)
            .iter()
            .map(|seen| seen.2.0)
            .collect();
        assert_eq!(widths, [625, 625, 625]);

        window
            .window()
            .dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged { scale_factor: 1.5 });
        session.refresh_previews();
        assert!(session.preview_timer.running());
        make_previews(&session);
        let widths: Vec<_> = drawn_previews(&window, 1.5)
            .iter()
            .map(|seen| seen.2.0)
            .collect();
        assert_eq!(widths, [750, 750, 750]);
    }

    #[test]
    fn a_picture_that_cannot_be_read_gets_a_notice_and_does_not_stop_the_rest() {
        let (window, session, root) = session(650.0);
        let state = window.global::<State>();
        let folder = scan_folder(root.path(), true);
        std::fs::write(folder.join("potential_top.png"), b"broken").unwrap();
        open_scan(&session, &folder);
        session.preview_room_changed(584.0);
        make_previews(&session);
        let ready: Vec<_> = (0..state.get_previews().row_count())
            .map(|index| state.get_previews().row_data(index).unwrap().ready)
            .collect();
        assert_eq!(ready, [true, true, false]);
        let last = session.log.row_data(session.log.row_count() - 1).unwrap();
        assert!(
            last.text
                .contains("Could not show the Potential Top preview")
        );
    }

    #[test]
    fn a_scan_without_pictures_clears_the_previews() {
        let (window, session, root) = session(650.0);
        let state = window.global::<State>();
        open_scan(&session, &scan_folder(root.path(), true));
        session.preview_room_changed(584.0);
        make_previews(&session);
        assert_eq!(state.get_previews().row_count(), 3);
        open_scan(&session, &scan_folder(&root.path().join("other"), false));
        assert_eq!(state.get_previews().row_count(), 0);
    }

    #[test]
    fn a_failed_scan_says_why_and_lets_the_next_one_start() {
        let (window, session, _root) = session(650.0);
        let state = window.global::<State>();
        begin(&session);
        assert!(state.get_busy());
        session.handle(Message::Failed(
            "Could not reach the osu! API: timeout: global. osu! may be having problems; try again in a few minutes.".into(),
        ));
        assert_eq!(state.get_status(), "Operation failed");
        assert!(!state.get_busy());
        let last = session.log.row_data(session.log.row_count() - 1).unwrap();
        assert!(last.text.starts_with("Error: Could not reach the osu! API"));
        assert!(session.prepare_scan().is_some());
    }

    #[test]
    fn a_results_view_that_is_already_shown_stays_as_it_is() {
        let (window, session, _root) = session(650.0);
        window.global::<State>().set_show_results(true);
        begin(&session);
        finish(&session);
        assert!(window.global::<State>().get_show_results());
    }

    fn record_calls(session: &Session) -> std::sync::mpsc::Receiver<String> {
        let (sent, received) = std::sync::mpsc::channel();
        let sent = Mutex::new(sent);
        *session.courier.borrow_mut() = std::sync::Arc::new(move |login: &Login, call: Call| {
            let call = match call {
                Call::Submit(body) => {
                    let body = String::from_utf8_lossy(&body);
                    let shown = body.contains(r#""made_at":1794649471"#);
                    format!(
                        "submit {}",
                        if shown {
                            "the shown scan"
                        } else {
                            "another body"
                        }
                    )
                }
                Call::Publish(id) => format!("publish {id}"),
                Call::Unpublish(id) => format!("unpublish {id}"),
                Call::Delete(id) => format!("delete {id}"),
                Call::List => "list".to_owned(),
                Call::Replay { id, row, .. } => format!("replay {id} {row}"),
            };
            sent.lock()
                .unwrap()
                .send(format!("{} {call}", login.token))
                .unwrap();
            Err(ApiError::Network("not in tests".into()))
        });
        received
    }

    fn calls(received: &std::sync::mpsc::Receiver<String>, count: usize) -> Vec<String> {
        let timeout = std::time::Duration::from_secs(5);
        let mut seen: Vec<String> = (0..count)
            .map(|_| received.recv_timeout(timeout).unwrap())
            .collect();
        seen.sort();
        assert!(
            received
                .recv_timeout(std::time::Duration::from_millis(200))
                .is_err()
        );
        seen
    }

    fn answer(visibility: &str, status: &str) -> Submission {
        Submission {
            id: "abc".into(),
            visibility: visibility.into(),
            status: status.into(),
        }
    }

    fn replay_bytes() -> Vec<u8> {
        let mut out = vec![0];
        out.extend(20210520i32.to_le_bytes());
        for text in [
            "0b577cbe9bc774600d20cfa17c0aa94c",
            "Lemon",
            "fedcba9876543210fedcba9876543210",
        ] {
            if text.is_empty() {
                out.push(0x00);
            } else {
                out.push(0x0b);
                out.push(u8::try_from(text.len()).unwrap());
                out.extend(text.as_bytes());
            }
        }
        for count in [300u16, 20, 1, 0, 0, 2] {
            out.extend(count.to_le_bytes());
        }
        out.extend(1_234_567u32.to_le_bytes());
        out.extend(500u16.to_le_bytes());
        out.push(0);
        out.extend(72u32.to_le_bytes());
        out.push(0x00);
        out.extend(638_357_968_000_000_000i64.to_le_bytes());
        let frames = include_bytes!("../../osu-formats/testdata/frames.lzma");
        out.extend(i32::try_from(frames.len()).unwrap().to_le_bytes());
        out.extend(frames);
        out.extend(1i64.to_le_bytes());
        out
    }

    fn replay_scan() -> lost_scores_app::history::SavedScan {
        use lost_scores_app::rows::{Hits, LostRow, PotentialRow};
        use lost_scores_core::grade::Grade;
        use lost_scores_core::maps::MapStatus;
        let hits = Hits {
            n300: 300,
            n100: 20,
            n50: 1,
            misses: 2,
        };
        let row = |file: &str, beatmap_id: u32, pp: f64| LostRow {
            pp,
            kept_pp: 150.0,
            score: 1_234_567,
            kept_score: 1_300_000,
            mods: "HDDT".into(),
            unix_time: 1_700_000_000,
            played: "2023-11-14 22:13".into(),
            status: MapStatus::Ranked,
            beatmap_md5: "0b577cbe9bc774600d20cfa17c0aa94c".into(),
            beatmap_id: Some(beatmap_id),
            beatmapset_id: None,
            artist: "A".into(),
            title: format!("Map {beatmap_id}"),
            version: "x".into(),
            file: file.into(),
            creator: "m".into(),
            hits,
            accuracy: 96.5,
            grade: Grade::S,
            max_combo: 500,
        };
        let place = |pp: f64, beatmap_id: u32| PotentialRow {
            place: 1,
            pp,
            weight: 1.0,
            weighted: pp,
            lost: true,
            beatmap_id,
            beatmapset_id: None,
            artist: "A".into(),
            title: format!("Map {beatmap_id}"),
            version: "x".into(),
            creator: "m".into(),
            mods: "HDDT".into(),
            accuracy: 96.5,
            unix_time: 1_700_000_000,
            played: "2023-11-14 22:13".into(),
            hits,
            score: 1_234_567,
            grade: Grade::S,
            status: MapStatus::Ranked,
            kept_pp: Some(150.0),
        };
        let mut scan = saved_scan();
        scan.lost = vec![row("a.osr", 11, 321.5), row("b.osr", 12, 200.0)];
        scan.potential = Some(PotentialResult {
            current_pp: 1000.0,
            potential_pp: 1100.0,
            current_accuracy: 98.0,
            potential_accuracy: 98.0,
            global_rank: None,
            lost_in_top: 2,
            rows: vec![place(321.5, 11), place(200.0, 12)],
            online: Vec::new(),
        });
        scan
    }

    fn shown_replay_scan(session: &Session, root: &Path, sent: Option<Sent>) -> PathBuf {
        let scan = replay_scan();
        let folder = history::save(root, &scan).unwrap();
        for name in ["a.osr", "b.osr"] {
            std::fs::write(root.join("osu/Data/r").join(name), replay_bytes()).unwrap();
        }
        if let Some(sent) = sent {
            history::remember(&folder, &sent).unwrap();
        }
        session.show_scan(0, Some(folder.clone()), &scan);
        folder
    }

    fn marks(session: &Session) -> Vec<i32> {
        use slint::Model;
        session.show_tab(Tab::Lost);
        (0..session.rows.row_count())
            .map(|i| session.rows.row_data(i).unwrap().replay)
            .collect()
    }

    fn titles(session: &Session) -> Vec<String> {
        use slint::Model;
        (0..session.rows.row_count())
            .map(|i| session.rows.row_data(i).unwrap().title.to_string())
            .collect()
    }

    #[test]
    fn the_scan_button_shows_a_short_date() {
        use crate::table::when_short;
        use jiff::tz::TimeZone;
        let (window, session, root) = keyless(650.0);
        let state = window.global::<State>();
        shown_replay_scan(&session, root.path(), None);
        assert_eq!(
            state.get_scan_date(),
            when_short(replay_scan().made_at, &TimeZone::system())
        );
    }

    #[test]
    fn searching_shows_only_the_matching_rows() {
        let (window, session, root) = keyless(650.0);
        let state = window.global::<State>();
        shown_replay_scan(&session, root.path(), None);
        session.show_tab(Tab::Lost);
        session.search_changed("Map 12");
        assert_eq!(titles(&session), ["Map 12"]);
        assert_eq!(state.get_search_label(), "1 found");
        assert_eq!(session.rows.source(0), Some(1));
        session.search_changed("zzz");
        assert!(titles(&session).is_empty());
        assert_eq!(state.get_search_label(), "nothing found");
        session.search_changed("");
        assert_eq!(titles(&session), ["Map 11", "Map 12"]);
        assert_eq!(state.get_search_label(), "");
    }

    #[test]
    fn sorting_keeps_the_found_rows() {
        let (_window, session, root) = keyless(650.0);
        shown_replay_scan(&session, root.path(), None);
        session.show_tab(Tab::Lost);
        session.search_changed("Map 1");
        assert_eq!(titles(&session), ["Map 11", "Map 12"]);
        session.sort_by(crate::table::Column::Pp);
        assert_eq!(titles(&session), ["Map 12", "Map 11"]);
        session.search_changed("Map 11");
        assert_eq!(titles(&session), ["Map 11"]);
    }

    #[test]
    fn the_offer_leaves_the_replays_out_until_asked() {
        let (window, session, root) = keyless(650.0);
        let state = window.global::<State>();
        let received = record_calls(&session);
        let folder = shown_replay_scan(&session, root.path(), None);
        state.set_offer_with_replays(true);
        session.offer(&folder, &replay_scan());
        assert!(state.get_offer_shown());
        assert!(!state.get_offer_with_replays());
        state.invoke_publish_offer();
        assert_eq!(calls(&received, 1), ["tok submit the shown scan"]);
        session.answered(&folder, Ok(Reply::Sent(answer("private", "unverified"))));
        assert_eq!(calls(&received, 1), ["tok publish abc"]);
        session.answered(&folder, Ok(Reply::Sent(answer("public", "unverified"))));
        let sent = history::submission(&folder, &login().server).unwrap();
        assert!(sent.queued.is_empty());
        assert_eq!(calls(&received, 0), Vec::<String>::new());
    }

    #[test]
    fn publishing_from_the_offer_sends_the_replays_of_the_top() {
        let (window, session, root) = keyless(650.0);
        let state = window.global::<State>();
        let received = record_calls(&session);
        let folder = shown_replay_scan(&session, root.path(), None);
        session.offer(&folder, &replay_scan());
        assert!(state.get_offer_shown());
        assert_eq!(state.get_replays_in_top(), 2);
        state.set_offer_with_replays(true);
        state.invoke_publish_offer();
        assert_eq!(calls(&received, 1), ["tok submit the shown scan"]);
        session.answered(&folder, Ok(Reply::Sent(answer("private", "unverified"))));
        assert_eq!(calls(&received, 1), ["tok publish abc"]);
        session.answered(&folder, Ok(Reply::Sent(answer("public", "unverified"))));
        let sent = history::submission(&folder, &login().server).unwrap();
        assert_eq!(sent.queued.into_iter().collect::<Vec<_>>(), [0, 1]);
        assert_eq!(calls(&received, 1), ["tok replay abc 0"]);
    }

    #[test]
    fn a_shown_scan_marks_its_replays_and_sends_the_waiting_ones() {
        let (_window, session, root) = keyless(650.0);
        let received = record_calls(&session);
        let mut sent = Sent::new(&answer("public", "unverified"), &login().server);
        sent.replays.insert(0, "aa".into());
        sent.queued.insert(1);
        shown_replay_scan(&session, root.path(), Some(sent));
        assert_eq!(marks(&session), [4, 3]);
        assert_eq!(calls(&received, 2), ["tok list", "tok replay abc 1"]);
    }

    #[test]
    fn a_new_answer_keeps_the_replays_of_the_scan() {
        let (_window, session, root) = keyless(650.0);
        let _received = record_calls(&session);
        let mut sent = Sent::new(&answer("private", "unverified"), &login().server);
        sent.replays.insert(0, "aa".into());
        let folder = shown_replay_scan(&session, root.path(), Some(sent));
        session.answered(&folder, Ok(Reply::Sent(answer("public", "unverified"))));
        let kept = history::submission(&folder, &login().server).unwrap();
        assert_eq!(kept.visibility, "public");
        assert_eq!(kept.replays.get(&0).map(String::as_str), Some("aa"));
        assert!(kept.queued.is_empty());
    }

    #[test]
    fn a_refused_publish_from_the_offer_shares_nothing_later() {
        let (window, session, root) = keyless(650.0);
        let state = window.global::<State>();
        let received = record_calls(&session);
        let folder = shown_replay_scan(&session, root.path(), None);
        session.offer(&folder, &replay_scan());
        state.invoke_publish_offer();
        assert_eq!(calls(&received, 1), ["tok submit the shown scan"]);
        session.answered(&folder, Err(ApiError::TooSoon { retry_after: 60 }.into()));
        let private = Sent::new(&answer("private", "unverified"), &login().server);
        history::remember(&folder, &private).unwrap();
        state.invoke_publish_scan();
        assert_eq!(calls(&received, 1), ["tok publish abc"]);
        session.answered(&folder, Ok(Reply::Sent(answer("public", "unverified"))));
        let sent = history::submission(&folder, &login().server).unwrap();
        assert!(sent.queued.is_empty());
        assert!(
            received
                .recv_timeout(std::time::Duration::from_millis(300))
                .is_err()
        );
    }

    #[test]
    fn waiting_replays_stay_queued_while_the_osu_folder_is_unknown() {
        let (window, session, root) = keyless(650.0);
        let state = window.global::<State>();
        let received = record_calls(&session);
        state.set_osu_dir("".into());
        let mut sent = Sent::new(&answer("public", "unverified"), &login().server);
        sent.queued.extend([0, 1]);
        let folder = shown_replay_scan(&session, root.path(), Some(sent));
        assert_eq!(calls(&received, 1), ["tok list"]);
        std::thread::sleep(std::time::Duration::from_millis(300));
        let kept = history::submission(&folder, &login().server).unwrap();
        assert_eq!(kept.queued.into_iter().collect::<Vec<_>>(), [0, 1]);
    }

    #[test]
    fn a_replay_that_went_out_is_marked_at_once() {
        use lost_scores_app::replays::Change;
        let (_window, session, root) = keyless(650.0);
        let _received = record_calls(&session);
        let mut sent = Sent::new(&answer("public", "unverified"), &login().server);
        sent.queued.extend([0, 1]);
        let folder = shown_replay_scan(&session, root.path(), Some(sent));
        session.replay_changed(
            &folder,
            "abc",
            &Change::Shared {
                row: 0,
                sha256: "aa".into(),
            },
        );
        let kept = history::submission(&folder, &login().server).unwrap();
        assert_eq!(kept.replays.get(&0).map(String::as_str), Some("aa"));
        assert_eq!(kept.queued.into_iter().collect::<Vec<_>>(), [1]);
        assert_eq!(marks(&session), [4, 3]);
        session.replay_changed(&folder, "another", &Change::Dropped { row: 1 });
        let kept = history::submission(&folder, &login().server).unwrap();
        assert_eq!(kept.queued.into_iter().collect::<Vec<_>>(), [1]);
    }

    #[test]
    fn a_share_turned_away_while_another_runs_starts_when_it_ends() {
        use std::sync::Arc;
        use std::sync::atomic::AtomicBool;
        let (_window, session, root) = keyless(650.0);
        let received = record_calls(&session);
        let other = root.path().join("other");
        *session.replays_running.borrow_mut() =
            Some((other.clone(), Arc::new(AtomicBool::new(false))));
        let mut sent = Sent::new(&answer("public", "unverified"), &login().server);
        sent.queued.insert(0);
        shown_replay_scan(&session, root.path(), Some(sent));
        assert_eq!(calls(&received, 1), ["tok list"]);
        session.replayed(&other, Ok(crate::send::Shared::default()));
        assert_eq!(calls(&received, 1), ["tok replay abc 0"]);
    }

    #[test]
    fn unpublishing_stops_only_that_scans_replays() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};
        let (_window, session, root) = keyless(650.0);
        let _received = record_calls(&session);
        let mut sent = Sent::new(&answer("public", "unverified"), &login().server);
        sent.queued.extend([0, 1]);
        let folder = shown_replay_scan(&session, root.path(), Some(sent));
        let other = Arc::new(AtomicBool::new(false));
        *session.replays_running.borrow_mut() = Some((root.path().join("other"), other.clone()));
        session.answered(&folder, Ok(Reply::Sent(answer("private", "unverified"))));
        let kept = history::submission(&folder, &login().server).unwrap();
        assert!(kept.queued.is_empty());
        assert!(!other.load(Ordering::Relaxed));
        let stop = Arc::new(AtomicBool::new(false));
        *session.replays_running.borrow_mut() = Some((folder.clone(), stop.clone()));
        session.answered(&folder, Ok(Reply::Sent(answer("private", "unverified"))));
        assert!(stop.load(Ordering::Relaxed));
    }

    #[test]
    fn deleting_stops_that_scans_replays() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};
        let (_window, session, root) = keyless(650.0);
        let _received = record_calls(&session);
        let mut sent = Sent::new(&answer("public", "unverified"), &login().server);
        sent.queued.insert(0);
        let folder = shown_replay_scan(&session, root.path(), Some(sent));
        let stop = Arc::new(AtomicBool::new(false));
        *session.replays_running.borrow_mut() = Some((folder.clone(), stop.clone()));
        session.answered(&folder, Ok(Reply::Deleted));
        assert!(stop.load(Ordering::Relaxed));
    }

    fn shown_scan(session: &Session, root: &Path, sent: Option<Submission>) -> PathBuf {
        let scan = saved_scan();
        let folder = history::save(root, &scan).unwrap();
        if let Some(sent) = sent {
            history::remember(&folder, &Sent::new(&sent, &login().server)).unwrap();
        }
        session.show_scan(0, Some(folder.clone()), &scan);
        folder
    }

    #[test]
    fn submitting_keeps_the_id_next_to_the_scan_and_shows_private() {
        let (window, session, root) = keyless(650.0);
        let state = window.global::<State>();
        let received = record_calls(&session);
        let folder = shown_scan(&session, root.path(), None);
        assert!(state.get_can_send());
        assert_eq!(state.get_sent_state(), "");
        session.submit_scan();
        assert!(state.get_sending());
        assert_eq!(calls(&received, 1), ["tok submit the shown scan"]);
        session.answered(&folder, Ok(Reply::Sent(answer("private", "unverified"))));
        assert!(!state.get_sending());
        assert_eq!(
            history::submission(&folder, &login().server),
            Some(Sent::new(&answer("private", "unverified"), &login().server))
        );
        assert_eq!(state.get_sent_state(), "Private");
        assert!(!state.get_sent_public());
    }

    #[test]
    fn publishing_then_unpublishing_flips_the_state() {
        let (window, session, root) = keyless(650.0);
        let state = window.global::<State>();
        let received = record_calls(&session);
        let folder = shown_scan(&session, root.path(), Some(answer("private", "unverified")));
        assert_eq!(state.get_sent_state(), "Private");
        state.invoke_publish_scan();
        assert_eq!(calls(&received, 2), ["tok list", "tok publish abc"]);
        session.answered(&folder, Ok(Reply::Sent(answer("public", "unverified"))));
        assert_eq!(state.get_sent_state(), "Published \u{b7} checking");
        assert!(state.get_sent_public());
        state.invoke_unpublish_scan();
        assert_eq!(calls(&received, 1), ["tok unpublish abc"]);
        session.answered(&folder, Ok(Reply::Sent(answer("private", "verified"))));
        assert_eq!(state.get_sent_state(), "Private");
        assert!(!state.get_sent_public());
        assert_eq!(
            history::submission(&folder, &login().server)
                .unwrap()
                .status,
            "verified"
        );
    }

    #[test]
    fn deleting_asks_first_and_forgets_the_id() {
        let (window, session, root) = keyless(650.0);
        let state = window.global::<State>();
        let received = record_calls(&session);
        let folder = shown_scan(&session, root.path(), Some(answer("public", "unverified")));
        state.invoke_ask_unsend();
        assert!(state.get_unsend_confirm_open());
        assert_eq!(calls(&received, 1), ["tok list"]);
        state.invoke_confirm_unsend();
        assert!(!state.get_unsend_confirm_open());
        assert_eq!(calls(&received, 1), ["tok delete abc"]);
        session.answered(&folder, Ok(Reply::Deleted));
        assert_eq!(history::submission(&folder, &login().server), None);
        assert_eq!(state.get_sent_state(), "");
        state.invoke_ask_unsend();
        state.invoke_cancel_unsend();
        assert!(!state.get_unsend_confirm_open());
    }

    #[test]
    fn with_own_keys_sending_is_off() {
        let (window, session, root) = session(650.0);
        let state = window.global::<State>();
        state.set_own_keys(true);
        let received = record_calls(&session);
        shown_scan(&session, root.path(), None);
        assert!(!state.get_can_send());
        session.submit_scan();
        assert!(!state.get_sending());
        assert_eq!(calls(&received, 0), Vec::<String>::new());
    }

    #[test]
    fn the_share_hint_says_why_sharing_is_off() {
        let (window, _session, _root) = session(650.0);
        let state = window.global::<State>();
        state.set_own_keys(true);
        assert_eq!(
            state.get_share_tip(),
            "Sharing works only with the osu! login"
        );
        state.set_own_keys(false);
        state.set_logged_in(false);
        assert_eq!(state.get_share_tip(), "Log in with osu! to share scans");
        state.set_logged_in(true);
        state.set_busy(true);
        assert_eq!(state.get_share_tip(), "Wait for the scan to finish");
        state.set_busy(false);
        assert_eq!(
            state.get_share_tip(),
            "Publish this scan in the Hall of Copium"
        );
    }

    #[test]
    fn the_table_switch_counts_the_lost_scores_of_the_table() {
        use lost_scores_app::rows::{Hits, LostRow};
        use lost_scores_core::grade::Grade;
        use lost_scores_core::maps::MapStatus;
        let (window, session, root) = session(650.0);
        let state = window.global::<State>();
        let row = |beatmap_id: u32, status: MapStatus| LostRow {
            pp: 300.0,
            kept_pp: 150.0,
            score: 1_234_567,
            kept_score: 1_300_000,
            mods: "HD".into(),
            unix_time: 1_700_000_000,
            played: "2023-11-14 22:13".into(),
            status,
            beatmap_md5: format!("{beatmap_id:032x}"),
            beatmap_id: Some(beatmap_id),
            beatmapset_id: None,
            artist: "A".into(),
            title: format!("Map {beatmap_id}"),
            version: "x".into(),
            file: format!("{beatmap_id}.osr"),
            creator: "m".into(),
            hits: Hits {
                n300: 300,
                n100: 20,
                n50: 1,
                misses: 2,
            },
            accuracy: 96.5,
            grade: Grade::S,
            max_combo: 500,
        };
        let mut scan = saved_scan();
        scan.lost = vec![
            row(11, MapStatus::Ranked),
            row(12, MapStatus::Ranked),
            row(13, MapStatus::Loved),
        ];
        let folder = history::save(root.path(), &scan).unwrap();
        state.invoke_set_include_all(false);
        session.show_scan(0, Some(folder), &scan);
        assert_eq!(state.get_lost_count(), 2);
        state.invoke_set_include_all(true);
        assert_eq!(state.get_lost_count(), 3);
    }

    #[test]
    fn a_too_soon_answer_says_when() {
        let (window, session, root) = keyless(650.0);
        let state = window.global::<State>();
        let received = record_calls(&session);
        let folder = shown_scan(&session, root.path(), None);
        session.submit_scan();
        calls(&received, 1);
        session.answered(&folder, Err(ApiError::TooSoon { retry_after: 1790 }.into()));
        let said = "You can send five new scans an hour; try again in 30 min";
        assert!(state.get_copied());
        assert_eq!(state.get_copied_text(), said);
        assert_eq!(state.get_status(), said);
        assert!(!state.get_sending());
        assert_eq!(history::submission(&folder, &login().server), None);
    }

    #[test]
    fn an_expired_login_while_sending_returns_to_the_connect_card() {
        let _files = files();
        let (window, session, root) = keyless(650.0);
        let state = window.global::<State>();
        let received = record_calls(&session);
        let folder = shown_scan(&session, root.path(), None);
        session.submit_scan();
        calls(&received, 1);
        session.answered(&folder, Err(ApiError::LoginRequired.into()));
        assert!(!state.get_connected());
        assert_eq!(state.get_status(), LOGIN_EXPIRED);
        assert!(!state.get_can_send());
    }

    #[test]
    fn the_own_list_updates_the_shown_state_and_drops_a_deleted_scan() {
        let (window, session, root) = keyless(650.0);
        let state = window.global::<State>();
        let received = record_calls(&session);
        let folder = shown_scan(&session, root.path(), Some(answer("private", "unverified")));
        calls(&received, 1);
        let epoch = session.sent_epoch.get();
        let listed = SubmissionInfo {
            id: "abc".into(),
            made_at: 1_794_649_471,
            visibility: "public".into(),
            status: "verified".into(),
            lost_pp: Some(1.5),
        };
        session.listed(&folder, epoch, Ok(Reply::Listed(vec![listed])));
        assert_eq!(state.get_sent_state(), "Published \u{b7} verified");
        session.listed(&folder, epoch + 1, Ok(Reply::Listed(Vec::new())));
        assert_eq!(state.get_sent_state(), "Published \u{b7} verified");
        session.listed(&folder, epoch, Ok(Reply::Listed(Vec::new())));
        assert_eq!(state.get_sent_state(), "");
        assert_eq!(history::submission(&folder, &login().server), None);
    }

    #[test]
    fn a_scan_a_moderator_hid_or_rejected_is_marked_apart() {
        let (window, session, root) = keyless(650.0);
        let state = window.global::<State>();
        let received = record_calls(&session);
        let folder = shown_scan(&session, root.path(), Some(answer("private", "unverified")));
        calls(&received, 1);
        let epoch = session.sent_epoch.get();
        for (status, blocked) in [
            ("hidden", true),
            ("rejected", true),
            ("verified", false),
            ("unverified", false),
        ] {
            let listed = SubmissionInfo {
                id: "abc".into(),
                made_at: 1_794_649_471,
                visibility: "public".into(),
                status: status.into(),
                lost_pp: Some(1.5),
            };
            session.listed(&folder, epoch, Ok(Reply::Listed(vec![listed])));
            assert_eq!(state.get_sent_blocked(), blocked, "{status}");
        }
    }

    #[test]
    fn a_scan_gone_from_the_server_can_be_sent_again() {
        let (window, session, root) = keyless(650.0);
        let state = window.global::<State>();
        let received = record_calls(&session);
        let folder = shown_scan(&session, root.path(), Some(answer("private", "unverified")));
        state.invoke_publish_scan();
        calls(&received, 2);
        session.answered(&folder, Err(ApiError::NotOnServer.into()));
        assert_eq!(state.get_sent_state(), "");
        assert_eq!(history::submission(&folder, &login().server), None);
        assert_eq!(
            state.get_copied_text(),
            "This scan is no longer on the Lost Scores server"
        );
    }

    fn finished_scan(session: &Session, root: &Path, lost: f64) -> PathBuf {
        let mut scan = saved_scan();
        scan.potential = Some(PotentialResult {
            current_pp: 8_557.4,
            potential_pp: 8_557.4 + lost,
            current_accuracy: 98.38,
            potential_accuracy: 97.95,
            global_rank: Some(14_856),
            lost_in_top: 3,
            rows: Vec::new(),
            online: Vec::new(),
        });
        let folder = history::save(root, &scan).unwrap();
        session.handle(Message::Finished(Box::new(Finished {
            scan,
            folder: folder.clone(),
        })));
        session.handle(Message::Refreshed(None));
        folder
    }

    fn last_log(session: &Session) -> String {
        session
            .log
            .row_data(session.log.row_count() - 1)
            .unwrap()
            .text
            .to_string()
    }

    #[test]
    fn a_finished_scan_is_saved_to_the_private_profile_quietly() {
        let (window, session, root) = keyless(650.0);
        let state = window.global::<State>();
        let received = record_calls(&session);
        begin(&session);
        let folder = finished_scan(&session, root.path(), 110.8);
        assert_eq!(calls(&received, 1), ["tok submit the shown scan"]);
        session.answered(&folder, Ok(Reply::Sent(answer("private", "unverified"))));
        assert_eq!(state.get_sent_state(), "Private");
        assert!(!state.get_copied());
        assert!(state.get_offer_shown());
    }

    #[test]
    fn the_scan_list_marks_published_scans_with_their_check_state() {
        let (window, session, root) = keyless(650.0);
        let state = window.global::<State>();
        let received = record_calls(&session);
        begin(&session);
        let folder = finished_scan(&session, root.path(), 110.8);
        calls(&received, 1);
        *session.entries.borrow_mut() = history::list(root.path());
        let marks = || state.get_scan_marks().iter().collect::<Vec<i32>>();
        session.answered(&folder, Ok(Reply::Sent(answer("private", "unverified"))));
        assert_eq!(marks(), [0]);
        session.answered(&folder, Ok(Reply::Sent(answer("public", "unverified"))));
        assert_eq!(marks(), [1]);
        session.answered(&folder, Ok(Reply::Sent(answer("public", "verified"))));
        assert_eq!(marks(), [2]);
        session.answered(&folder, Ok(Reply::Sent(answer("public", "hidden"))));
        assert_eq!(marks(), [3]);
        session.answered(&folder, Ok(Reply::Sent(answer("private", "hidden"))));
        assert_eq!(marks(), [0]);
    }

    #[test]
    fn the_log_is_one_selectable_text_kept_in_a_file() {
        let (window, session, _root) = session(650.0);
        let state = window.global::<State>();
        session.log("first line");
        session.log("second line");
        let text = state.get_log_text().to_string();
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines[lines.len() - 2].ends_with("] first line"), "{text}");
        assert!(lines[lines.len() - 1].ends_with("] second line"), "{text}");
        let saved = std::fs::read_to_string(setup::data_root().unwrap().join("log.txt")).unwrap();
        assert!(saved.contains("] first line\n"), "{saved}");
        for line in 0..crate::log::KEEP_LINES {
            session.log(&format!("line {line}"));
        }
        assert_eq!(session.log.row_count(), crate::log::KEEP_LINES);
        assert_eq!(state.get_log_text().lines().count(), crate::log::KEEP_LINES);
    }

    #[test]
    fn with_own_keys_a_finished_scan_stays_on_the_computer() {
        let (_window, session, root) = session(650.0);
        let received = record_calls(&session);
        begin(&session);
        finished_scan(&session, root.path(), 110.8);
        assert_eq!(calls(&received, 0), Vec::<String>::new());
    }

    #[test]
    fn a_refused_save_after_a_scan_only_goes_to_the_log() {
        let (window, session, root) = keyless(650.0);
        let state = window.global::<State>();
        let received = record_calls(&session);
        begin(&session);
        let folder = finished_scan(&session, root.path(), 110.8);
        calls(&received, 1);
        let status = state.get_status();
        session.answered(&folder, Err(ApiError::TooSoon { retry_after: 1790 }.into()));
        assert!(!state.get_copied());
        assert_eq!(state.get_status(), status);
        assert!(
            last_log(&session).starts_with("Could not save the scan to your profile"),
            "{}",
            last_log(&session)
        );
        assert_eq!(state.get_sent_state(), "");
    }

    #[test]
    fn share_brings_back_the_publish_card_and_publishes_the_saved_scan() {
        let (window, session, root) = keyless(650.0);
        let state = window.global::<State>();
        let received = record_calls(&session);
        begin(&session);
        let folder = finished_scan(&session, root.path(), 110.8);
        calls(&received, 1);
        session.answered(&folder, Ok(Reply::Sent(answer("private", "unverified"))));
        state.invoke_dismiss_offer();
        assert!(!state.get_offer_shown());
        state.invoke_share();
        assert!(state.get_offer_shown());
        state.invoke_publish_offer();
        assert_eq!(calls(&received, 1), ["tok publish abc"]);
        session.answered(&folder, Ok(Reply::Sent(answer("public", "unverified"))));
        assert!(!state.get_offer_shown());
        assert_eq!(state.get_sent_state(), "Published \u{b7} checking");
    }

    #[test]
    fn share_with_nothing_lost_publishes_at_once() {
        let (window, session, root) = keyless(650.0);
        let state = window.global::<State>();
        let received = record_calls(&session);
        begin(&session);
        let folder = finished_scan(&session, root.path(), 0.0);
        calls(&received, 1);
        session.answered(&folder, Ok(Reply::Sent(answer("private", "unverified"))));
        state.invoke_share();
        assert!(!state.get_offer_shown());
        assert_eq!(calls(&received, 1), ["tok publish abc"]);
    }

    #[test]
    fn a_fresh_scan_offers_the_hall_of_fame_until_answered() {
        let (window, session, root) = keyless(650.0);
        let state = window.global::<State>();
        let _received = record_calls(&session);
        begin(&session);
        let folder = finished_scan(&session, root.path(), 110.8);
        assert!(state.get_offer_shown());
        assert_eq!(
            (
                state.get_offer_lost().to_string(),
                state.get_offer_before().to_string(),
                state.get_offer_after().to_string()
            ),
            ("111".into(), "8,557".into(), "8,668".into())
        );
        state.invoke_dismiss_offer();
        assert!(!state.get_offer_shown());
        session.show_scan(0, Some(folder), &saved_scan());
        assert!(!state.get_offer_shown());
    }

    #[test]
    fn publishing_from_the_offer_sends_then_publishes() {
        let (window, session, root) = keyless(650.0);
        let state = window.global::<State>();
        let received = record_calls(&session);
        begin(&session);
        let folder = finished_scan(&session, root.path(), 110.8);
        state.invoke_publish_offer();
        assert!(!state.get_offer_shown());
        assert_eq!(calls(&received, 1), ["tok submit the shown scan"]);
        session.answered(&folder, Ok(Reply::Sent(answer("private", "unverified"))));
        assert_eq!(calls(&received, 1), ["tok publish abc"]);
        session.answered(&folder, Ok(Reply::Sent(answer("public", "unverified"))));
        assert_eq!(state.get_sent_state(), "Published \u{b7} checking");
        assert!(!state.get_offer_shown());
    }

    #[test]
    fn with_own_keys_no_offer_is_made() {
        let (window, session, root) = session(650.0);
        let state = window.global::<State>();
        begin(&session);
        finished_scan(&session, root.path(), 110.8);
        assert!(!state.get_offer_shown());
    }

    #[test]
    fn a_scan_that_lost_no_pp_offers_nothing() {
        let (window, session, root) = keyless(650.0);
        let state = window.global::<State>();
        begin(&session);
        finished_scan(&session, root.path(), 0.2);
        assert!(!state.get_offer_shown());
    }

    #[test]
    fn deleting_a_sent_scan_says_its_server_copy_stays() {
        let _files = files();
        let (window, session, _root) = keyless(650.0);
        let state = window.global::<State>();
        let dir = history::scans_dir().unwrap();
        let mut older = saved_scan();
        older.made_at -= 600;
        let older = history::save(&dir, &older).unwrap();
        let newer = history::save(&dir, &saved_scan()).unwrap();
        history::remember(
            &older,
            &Sent::new(&answer("public", "unverified"), &login().server),
        )
        .unwrap();
        session.reload_scans(&dir);
        let index = session
            .entries
            .borrow()
            .iter()
            .position(|entry| entry.folder == older)
            .unwrap();
        session.ask_delete_scan(index);
        assert!(state.get_delete_confirm_open());
        assert!(
            state
                .get_delete_message()
                .starts_with("Delete the scan from ")
                && state.get_delete_message().contains("This can't be undone."),
            "{}",
            state.get_delete_message()
        );
        assert!(
            state
                .get_delete_message()
                .contains("Its copy on the Lost Scores server stays"),
            "{}",
            state.get_delete_message()
        );
        session.cancel_delete();
        session.ask_delete_older();
        assert!(
            state
                .get_delete_message()
                .contains("1 of them stays on the Lost Scores server"),
            "{}",
            state.get_delete_message()
        );
        session.cancel_delete();
        std::fs::remove_dir_all(older).unwrap();
        std::fs::remove_dir_all(newer).unwrap();
    }

    #[test]
    fn a_declined_offer_stays_hidden_after_the_status_refresh() {
        let (window, session, root) = keyless(650.0);
        let state = window.global::<State>();
        begin(&session);
        let folder = finished_scan(&session, root.path(), 110.8);
        state.invoke_dismiss_offer();
        let scan = history::load(&folder).unwrap();
        session.handle(Message::Refreshed(Some(Box::new(Finished {
            scan,
            folder,
        }))));
        assert!(!state.get_offer_shown());
    }
}
