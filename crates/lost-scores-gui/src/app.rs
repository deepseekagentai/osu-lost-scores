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
use lost_scores_app::online::Online;
use lost_scores_app::pipeline::{PotentialResult, Source};
use lost_scores_app::rows::thousands;
use lost_scores_app::setup::Settings;
use lost_scores_app::updates::latest_release;
use lost_scores_app::{cache, history, keys, setup};
use osu_api::{Credentials, UserKey};
use slint::{ComponentHandle, ModelRc, VecModel, Weak};

use crate::account::{Account, Field, KeysError, Profile, check_fields, keys_error, parse_account};
use crate::avatar::{avatar_pixels, fetch, render_avatar};
use crate::clipboard::{Clipboard, Request};
use crate::log::{clock_now, split, stamp};
use crate::newest::{load_newest, newest_of_player};
use crate::scan::{NEEDS_FOLDER, NEEDS_KEYS, Phase, ScanState, check_folders};
use crate::scans::{older_done, older_prompt, reread, trash_message};
use crate::share::render::{ShareImage, images_in};
use crate::share::upload::{self, Choices, Consent};
use crate::table::{Column, Tab, Table, anchor_index, tone, when, wrap};
use crate::update::update_message;
use crate::view::{RowsModel, foot, mods_width};
use crate::worker::{self, Job, Message, Sink};
use crate::{APP_ID, LogEntry, MainWindow, State, choose_folder, show_path};

const AVATAR_LOGICAL: f32 = 120.0;
const ROW_HEIGHT: f32 = 64.0;
const ACCOUNT_SETTINGS: &str = "https://osu.ppy.sh/home/account/edit";
const BEATMAP_PAGE: &str = "https://osu.ppy.sh/b/";
const DEVELOPER_SITE: &str = "https://lemon4ik.kz/";
const UPDATE_TIMEOUT: Duration = Duration::from_secs(15);
const COPIED_FOR: Duration = Duration::from_secs(2);

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
    settings: RefCell<Settings>,
    player: Cell<Option<u32>>,
    epoch: Cell<u64>,
    dialog: Arc<AtomicBool>,
    table: Rc<RefCell<Table>>,
    rows: Rc<RowsModel>,
    tab: Cell<Tab>,
    query: RefCell<String>,
    matches: RefCell<Vec<usize>>,
    position: Cell<usize>,
    entries: RefCell<Vec<history::ScanEntry>>,
    folder: RefCell<Option<PathBuf>>,
    clipboard: RefCell<Clipboard>,
    copied: slint::Timer,
    consent: Cell<Consent>,
    choices: RefCell<Choices>,
    uploading: Cell<bool>,
    pending_delete: RefCell<Option<Removal>>,
    opens_results: Cell<bool>,
    opener: RefCell<Opener>,
}

type Opener = Arc<dyn Fn(&OsStr) -> std::io::Result<()> + Send + Sync>;

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
            matches: RefCell::new(Vec::new()),
            position: Cell::new(0),
            entries: RefCell::new(Vec::new()),
            folder: RefCell::new(None),
            clipboard: RefCell::new(Clipboard::system()),
            copied: slint::Timer::default(),
            consent: Cell::new(Consent::new(false)),
            choices: RefCell::new(Choices::default()),
            uploading: Cell::new(false),
            pending_delete: RefCell::new(None),
            opens_results: Cell::new(false),
            opener: RefCell::new(Arc::new(|target| open::that(target))),
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
        move || watcher.refresh_avatar(),
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

fn config_player() -> Option<String> {
    let osu_dir = setup::resolve_osu_dir(None).ok()?;
    setup::player(&setup::read_user_config(&osu_dir)).ok()
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
        self.bind(&state);
        self.bind_extras(&state);
        self.load_newest();
        *self.credentials.borrow_mut() = stored_credentials();
        let name = self.saved_player().or_else(config_player);
        if let (Some(credentials), Some(name)) = (self.credentials.borrow().clone(), name) {
            state.set_connected(true);
            state.set_user_name(name.clone().into());
            self.fetch_profile(credentials, name);
        }
    }

    fn saved_player(&self) -> Option<String> {
        self.settings.borrow().player.clone()
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
        if let Some(count) = setup::parse_scores_to_show(text) {
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
        state.on_set_tab(move |index| session.show_tab(Tab::from_index(index)));
        let session = self.clone();
        state.on_sort_by(move |index| session.sort_by(Column::from_index(index)));
        let session = self.clone();
        state.on_search_changed(move |text| session.search_changed(&text));
        let session = self.clone();
        state.on_search_step(move |step| session.search_step(step));
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
        state.on_open_share(move || session.open_share());
        let session = self.clone();
        state.on_open_image(move |index| session.open_image(index as usize));
        let session = self.clone();
        state.on_copy_image(move |index| session.copy_image(index as usize));
        let session = self.clone();
        state.on_upload_image(move |index| session.ask_upload(index as usize));
        let session = self.clone();
        state.on_confirm_upload(move |remember| session.confirm_upload(remember));
        let session = self.clone();
        state.on_cancel_upload(move || session.cancel_upload());
    }

    fn bind_extras(self: &Rc<Self>, state: &State<'_>) {
        let session = self.clone();
        state.on_open_developer_site(move || session.launch("the page", DEVELOPER_SITE.into()));
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
        state.on_check_updates(move || session.check_updates());
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

    fn check_updates(self: &Rc<Self>) {
        let state = self.window.global::<State>();
        if state.get_checking_updates() {
            return;
        }
        state.set_checking_updates(true);
        let weak = self.weak.clone();
        thread::spawn(move || {
            let agent: ureq::Agent = ureq::Agent::config_builder()
                .timeout_global(Some(UPDATE_TIMEOUT))
                .user_agent(lost_scores_app::online::USER_AGENT)
                .build()
                .into();
            let result = latest_release(&agent);
            on_ui(&weak, move |app| {
                app.window.global::<State>().set_checking_updates(false);
                match result {
                    Ok(release) => {
                        let message = update_message(env!("CARGO_PKG_VERSION"), release.as_ref());
                        app.log(&message.text);
                        if let Some(url) = message.open {
                            app.launch("the page", url.into());
                        }
                    }
                    Err(error) => app.log(&format!("Could not check for updates: {error:#}")),
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
        let state = self.window.global::<State>();
        self.relist(index);
        state.set_scan_date(when(scan.made_at, &zone).into());
        state.set_has_results(true);
        self.refresh_shared();
        self.show_tab(self.tab.get());
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
    }

    fn ask_delete_scan(&self, index: usize) {
        let entry = match self.entries.borrow().get(index) {
            Some(entry) if index > 0 => entry.clone(),
            _ => return,
        };
        let date = when(entry.made_at, &TimeZone::system());
        self.ask_delete(
            vec![entry.folder],
            format!("Move the scan from {date} to the trash?"),
            format!("Moved the scan from {date} to the trash"),
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
        self.ask_delete(folders, older_prompt(count, bytes), older_done(count));
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
            let result = history::remove(&dir, &removal.folders, |folders| {
                trash::delete_all(folders).map_err(|error| anyhow::anyhow!(trash_message(&error)))
            });
            on_ui(&weak, move |app| {
                match result {
                    Ok(()) => {
                        app.log(&removal.done);
                        app.toast("Moved to the trash");
                    }
                    Err(error) => app.fail_removal(&format!("{error:#}")),
                }
                app.reload_scans(&dir);
            });
        });
    }

    fn fail_removal(&self, reason: &str) {
        self.log(&format!("Could not move the scans to the trash: {reason}"));
        self.toast("Could not move to the trash: see the log");
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
        let labels: Vec<slint::SharedString> =
            found.iter().map(|image| image.label.into()).collect();
        state.set_has_images(!found.is_empty());
        state.set_share_images(ModelRc::new(VecModel::from(labels)));
        self.choices.borrow_mut().replace(found);
    }

    fn open_share(&self) {
        self.refresh_shared();
        let state = self.window.global::<State>();
        state.set_share_open(state.get_has_images());
    }

    fn toast(&self, text: &str) {
        let state = self.window.global::<State>();
        state.set_copied_text(text.into());
        state.set_copied(true);
        let weak = self.weak.clone();
        self.copied
            .start(slint::TimerMode::SingleShot, COPIED_FOR, move || {
                if let Some(window) = weak.upgrade() {
                    window.global::<State>().set_copied(false);
                }
            });
    }

    fn put_on_clipboard(
        &self,
        request: Request,
        done: impl FnOnce(&Rc<Session>, Result<(), String>) + Send + 'static,
    ) {
        let weak = self.weak.clone();
        self.clipboard.borrow().send(request, move |result| {
            on_ui(&weak, move |app| done(app, result));
        });
    }

    fn shared_image(&self, index: usize) -> Option<ShareImage> {
        self.choices.borrow().get(index)
    }

    fn open_image(&self, index: usize) {
        self.window.global::<State>().set_share_open(false);
        let Some(image) = self.shared_image(index) else {
            return;
        };
        self.launch("the image", image.path.into_os_string());
    }

    fn launch(&self, what: &'static str, target: OsString) {
        let opener = self.opener.borrow().clone();
        let weak = self.weak.clone();
        thread::spawn(move || {
            if let Err(error) = opener(&target) {
                let reason = error.to_string();
                on_ui(&weak, move |app| app.open_failed(what, &reason));
            }
        });
    }

    fn open_failed(&self, what: &str, reason: &str) {
        self.log(&format!("Could not open {what}: {reason}"));
        self.toast(&format!("Could not open {what}: see the log"));
    }

    fn copy_failed(&self, error: &str) {
        self.log(&format!("Could not copy the image: {error}"));
        self.toast("Could not copy the image: see the log");
    }

    fn copy_image(self: &Rc<Self>, index: usize) {
        self.window.global::<State>().set_share_open(false);
        let Some(image) = self.shared_image(index) else {
            return;
        };
        self.put_on_clipboard(Request::Image(image.path), |app, result| match result {
            Ok(()) => app.toast("Image copied"),
            Err(error) => app.copy_failed(&error),
        });
    }

    fn ask_upload(self: &Rc<Self>, index: usize) {
        let state = self.window.global::<State>();
        state.set_share_open(false);
        if self.uploading.get() {
            self.toast("An upload is already running");
            return;
        }
        if self.consent.get().must_ask() {
            if self.choices.borrow_mut().hold(index) {
                state.set_upload_confirm_open(true);
            }
        } else if let Some(image) = self.shared_image(index) {
            self.upload(image);
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
        if let Some(image) = held {
            self.upload(image);
        }
    }

    fn cancel_upload(&self) {
        self.choices.borrow_mut().release();
        self.window.global::<State>().set_upload_confirm_open(false);
    }

    fn upload(self: &Rc<Self>, image: ShareImage) {
        self.uploading.set(true);
        self.log(&format!("Uploading {} to catbox...", image.label));
        self.toast("Uploading...");
        let weak = self.weak.clone();
        thread::spawn(move || {
            let result = upload::upload(&upload::agent(), &image.path);
            on_ui(&weak, move |app| {
                app.uploading.set(false);
                app.uploaded(result);
            });
        });
    }

    fn uploaded(&self, result: Result<String>) {
        let link = match result {
            Ok(link) => link,
            Err(error) => {
                self.log(&format!("Could not upload the image: {error:#}"));
                self.toast("Could not upload: see the log");
                return;
            }
        };
        let text = link.clone();
        self.put_on_clipboard(Request::Text(text), move |app, copied| match copied {
            Ok(()) => {
                app.log(&format!("Link copied: {link}"));
                app.toast("Link copied");
            }
            Err(error) => {
                app.log(&format!(
                    "Uploaded, but the link could not be copied: {error}. Link: {link}"
                ));
                app.toast("Uploaded: the link is in the log");
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

    fn show_tab(&self, tab: Tab) {
        self.table.borrow_mut().clear_fresh();
        self.show_rows(tab);
        self.jump(0, false);
    }

    fn show_rows(&self, tab: Tab) {
        self.tab.set(tab);
        let state = self.window.global::<State>();
        state.set_tab(tab.index() as i32);
        state.set_foot(foot(&self.table.borrow(), tab));
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
        let current = self
            .matches
            .borrow()
            .get(self.position.get())
            .and_then(|&row| self.table.borrow().rows(tab).get(row).cloned());
        self.table.borrow_mut().sort(tab, column);
        self.rows.refresh();
        self.show_sorting();
        self.rerun_search(false);
        let position = current.and_then(|item| {
            let table = self.table.borrow();
            self.matches
                .borrow()
                .iter()
                .position(|&row| table.rows(tab).get(row) == Some(&item))
        });
        match position {
            Some(position) => {
                self.position.set(position);
                self.show_match(true);
            }
            None if self.matches.borrow().is_empty() => self.jump(0, false),
            None => self.show_match(true),
        }
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
        let found = self.table.borrow().search(self.tab.get(), &query);
        self.position.set(0);
        *self.matches.borrow_mut() = found;
        self.show_match(jump);
    }

    fn search_step(&self, step: i32) {
        let total = self.matches.borrow().len();
        self.position.set(wrap(self.position.get(), total, step));
        self.show_match(true);
    }

    fn show_match(&self, jump: bool) {
        let state = self.window.global::<State>();
        let matches = self.matches.borrow();
        let label = if self.query.borrow().trim().is_empty() {
            String::new()
        } else if matches.is_empty() {
            "0/0".to_owned()
        } else {
            format!("{}/{}", self.position.get() + 1, matches.len())
        };
        state.set_search_label(label.into());
        match matches.get(self.position.get()) {
            Some(&row) => {
                state.set_search_index(row as i32);
                if jump {
                    self.jump(row as i32, true);
                }
            }
            None => state.set_search_index(-1),
        }
    }

    fn copy_row(&self, index: usize) {
        let text = self.table.borrow().copy_text(self.tab.get(), index);
        self.put_on_clipboard(Request::Text(text), |app, result| match result {
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
        let line = stamp(clock_now(), text);
        let (time, text) = crate::log::split(&line);
        self.log.push(LogEntry {
            time: time.into(),
            text: text.into(),
        });
    }

    fn close_popups(&self) {
        let state = self.window.global::<State>();
        state.set_share_open(false);
        state.set_scans_open(false);
    }

    fn open_keys(&self) {
        let state = self.window.global::<State>();
        let saved = self.saved_player();
        let from_config = saved.is_none();
        let name = saved.or_else(config_player).unwrap_or_default();
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
        let shown = match &account {
            Account::Name(name) => name.clone(),
            Account::Id(id) => id.to_string(),
        };
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
                let name = profile.username.clone();
                self.change_settings(|settings| settings.player = Some(name.clone()));
                *self.credentials.borrow_mut() = Some(credentials);
                self.epoch.set(self.epoch.get() + 1);
                *self.avatar.borrow_mut() = None;
                state.set_has_avatar(false);
                self.rendered.set(0);
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

    fn fetch_profile(self: &Rc<Self>, credentials: Credentials, name: String) {
        let weak = self.weak.clone();
        let epoch = self.epoch.get();
        thread::spawn(move || {
            let result = validate(&credentials, &Account::Name(name));
            on_ui(&weak, move |app| app.profile_arrived(epoch, result));
        });
    }

    fn profile_arrived(self: &Rc<Self>, epoch: u64, result: Result<Profile, KeysError>) {
        if self.epoch.get() == epoch {
            self.profile_fetched(result);
        }
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

    fn refresh_avatar(&self) {
        let pixels = avatar_pixels(AVATAR_LOGICAL, self.window.window().scale_factor());
        if pixels == self.rendered.get() {
            return;
        }
        let rendered = match self.avatar.borrow().as_ref() {
            Some(bytes) => render_avatar(bytes, pixels),
            None => return,
        };
        match rendered {
            Ok(buffer) => {
                let state = self.window.global::<State>();
                state.set_avatar(slint::Image::from_rgba8(buffer));
                state.set_has_avatar(true);
                self.rendered.set(pixels);
            }
            Err(error) => {
                *self.avatar.borrow_mut() = None;
                self.log(&format!("Could not show the avatar: {error:#}"));
            }
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

    fn folder_picked(&self, dir: &Path) {
        let state = self.window.global::<State>();
        match choose_folder(dir) {
            Ok(path) => {
                self.settings.borrow_mut().osu_dir = Some(PathBuf::from(&path));
                state.set_osu_dir(path.into());
                state.set_status("".into());
            }
            Err(error) => state.set_status(error.to_string().into()),
        }
    }

    fn busy(&self) -> bool {
        self.scan.borrow().phase == Phase::Scanning || self.refreshing.get()
    }

    fn start_scan(self: &Rc<Self>) {
        let Some((job, credentials)) = self.prepare_scan() else {
            return;
        };
        self.begin_scan();
        let sink = LoopSink {
            weak: self.weak.clone(),
            closed: self.closed.clone(),
        };
        thread::spawn(move || {
            let mut api = Online::new(credentials);
            worker::run(&job, &mut api, &sink);
        });
    }

    fn begin_scan(&self) {
        self.scan.borrow_mut().start();
        let showing = self.window.global::<State>().get_show_results();
        self.opens_results
            .set(!self.window.get_two_columns() && !showing);
        self.sync();
    }

    fn prepare_scan(&self) -> Option<(Job, Credentials)> {
        self.commit_pending_scores();
        let state = self.window.global::<State>();
        let credentials = self.credentials.borrow().clone();
        let connected = credentials.is_some() && state.get_connected();
        if !connected {
            state.set_status(NEEDS_KEYS.into());
            return None;
        }
        if self.refreshing.get() || !self.scan.borrow().can_start(connected) {
            return None;
        }
        let credentials = credentials?;
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
            },
            scans_dir,
            include_all: settings.include_all,
            renderer: std::env::current_exe().unwrap_or_default(),
            share_count: usize::from(clamped_scores(settings.scores_to_show)),
            show_lost_in_top: settings.show_lost_in_top_image,
        };
        Some((job, credentials))
    }

    fn sync(&self) {
        let state = self.window.global::<State>();
        let scan = self.scan.borrow();
        state.set_status(scan.status.clone().into());
        state.set_progress(scan.progress);
        state.set_busy(self.busy());
        for line in &scan.log[self.shown.get()..] {
            let (time, text) = split(line);
            self.log.push(LogEntry {
                time: time.into(),
                text: text.into(),
            });
        }
        self.shown.set(scan.log.len());
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
                    .finish(done.scan.lost_total, done.scan.lost_ranked, false);
                self.show_results(&done.scan);
                self.register(&done.folder, &done.scan);
            }
            Message::Refreshed(updated) => {
                self.refreshing.set(false);
                if let Some(done) = updated {
                    self.counts
                        .set((done.scan.lost_total, done.scan.lost_ranked));
                    self.show_results(&done.scan);
                    self.register(&done.folder, &done.scan);
                }
                self.refresh_shared();
                let (total, ranked) = self.counts.get();
                let images = self.window.global::<State>().get_has_images();
                self.scan.borrow_mut().finish(total, ranked, images);
                self.open_results_if_narrow();
            }
            Message::Failed(error) => {
                self.refreshing.set(false);
                self.scan.borrow_mut().fail(&error, clock_now());
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
        if let Some(path) = setup::credentials_file()
            && let Err(error) = keys::delete(&path)
        {
            self.log(&format!("Could not delete the saved keys: {error:#}"));
            return;
        }
        *self.credentials.borrow_mut() = None;
        self.epoch.set(self.epoch.get() + 1);
        self.player.set(None);
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

fn validate(credentials: &Credentials, account: &Account) -> Result<Profile, KeysError> {
    let mut online = Online::new(credentials.clone());
    online.check().map_err(|error| keys_error(&error))?;
    let player = match account {
        Account::Name(name) => online.account(UserKey::Username, name),
        Account::Id(id) => online.account(UserKey::Id, &id.to_string()),
    }
    .map_err(|error| keys_error(&error))?;
    Ok(Profile::from_player(&player))
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    use lost_scores_app::setup;
    use osu_api::Credentials;
    use slint::{ComponentHandle, LogicalSize, Model};
    use tempfile::TempDir;

    use super::Session;
    use crate::account::KeysError;
    use crate::share::render::ShareImage;
    use crate::worker::Message;
    use crate::{MainWindow, State};

    static HOME: Mutex<(usize, Option<TempDir>)> = Mutex::new((0, None));

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
        session.profile_arrived(stale, Err(KeysError::Unreachable));
        assert_eq!(state.get_perf_now(), "");
        assert!(!state.get_stats_ready());
        session.profile_arrived(stale + 1, Err(KeysError::Unreachable));
        assert_eq!(state.get_perf_now(), "-");
        assert!(state.get_stats_ready());
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
        state.set_scores_pending("999".into());
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
            state.set_share_open(true);
            state.set_scans_open(true);
            ask(&state);
            assert!(!state.get_share_open(), "ask {index}");
            assert!(!state.get_scans_open(), "ask {index}");
        }
        assert!(state.get_confirm_open() && state.get_cache_confirm_open());
        assert!(state.get_about_open() && state.get_keys_open());
    }

    #[test]
    fn a_failed_open_is_logged_and_toasted() {
        let (window, session, _root) = session(650.0);
        session.open_failed("the image", "no viewer installed");
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

    #[test]
    fn a_results_view_that_is_already_shown_stays_as_it_is() {
        let (window, session, _root) = session(650.0);
        window.global::<State>().set_show_results(true);
        begin(&session);
        finish(&session);
        assert!(window.global::<State>().get_show_results());
    }
}
