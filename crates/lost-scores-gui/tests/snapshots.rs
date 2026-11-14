use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::thread;
use std::time::Duration;

use image::{Rgba, RgbaImage};
use jiff::tz::TimeZone;
use lost_scores_app::history::{SavedScan, VERSION};
use lost_scores_app::pipeline::PotentialResult;
use lost_scores_app::rows::{Hits, LostRow, OnlineRow, PotentialRow};
use lost_scores_core::grade::Grade;
use lost_scores_core::maps::MapStatus;
use lost_scores_gui::avatar::render_avatar;
use lost_scores_gui::table::{Column, Tab, Table, when};
use lost_scores_gui::view::{RowsModel, foot};
use lost_scores_gui::{LogEntry, MainWindow, State};
use slint::platform::WindowEvent;
use slint::platform::software_renderer::{
    MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType,
};
use slint::platform::{Platform, WindowAdapter};
use slint::{ComponentHandle, ModelRc, PhysicalSize, VecModel};

const SCALE: f32 = 1.25;
const LOGICAL: (f32, f32) = (650.0, 800.0);
const WIDE: (f32, f32) = (1100.0, 800.0);
const COMPACT: (f32, f32) = (650.0, 720.0);
const TWO_COLUMNS: (f32, f32) = (1600.0, 800.0);

struct Offscreen(Rc<MinimalSoftwareWindow>);

impl Platform for Offscreen {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
}

fn draw(surface: &Rc<MinimalSoftwareWindow>, path: &PathBuf) {
    draw_sized(surface, LOGICAL, path);
}

fn draw_sized(surface: &Rc<MinimalSoftwareWindow>, logical: (f32, f32), path: &PathBuf) {
    let width = (logical.0 * SCALE) as u32;
    let height = (logical.1 * SCALE) as u32;
    surface.set_size(PhysicalSize::new(width, height));
    slint::platform::update_timers_and_animations();
    thread::sleep(Duration::from_millis(700));
    for _ in 0..3 {
        slint::platform::update_timers_and_animations();
        surface.request_redraw();
    }
    let mut buffer = vec![PremultipliedRgbaColor::default(); (width * height) as usize];
    surface.draw_if_needed(|renderer| {
        renderer.render(&mut buffer, width as usize);
    });
    let bytes: Vec<u8> = buffer
        .iter()
        .flat_map(|p| [p.red, p.green, p.blue, p.alpha])
        .collect();
    RgbaImage::from_raw(width, height, bytes)
        .unwrap()
        .save(path)
        .unwrap();
}

fn avatar() -> slint::Image {
    let source = RgbaImage::from_fn(256, 256, |x, y| Rgba([(x as u8), (y as u8), 180, 255]));
    let mut png = Vec::new();
    source
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    let pixels = (120.0 * SCALE) as u32;
    slint::Image::from_rgba8(render_avatar(&png, pixels).unwrap())
}

fn log(lines: &[(&str, &str)]) -> ModelRc<LogEntry> {
    ModelRc::new(VecModel::from(
        lines
            .iter()
            .map(|(time, text)| LogEntry {
                time: (*time).into(),
                text: (*text).into(),
            })
            .collect::<Vec<_>>(),
    ))
}

#[test]
#[ignore = "writes PNG snapshots to $LOST_SCORES_SNAPSHOTS"]
fn render_snapshots() {
    let dir = PathBuf::from(std::env::var("LOST_SCORES_SNAPSHOTS").expect("LOST_SCORES_SNAPSHOTS"));
    std::fs::create_dir_all(&dir).unwrap();
    let surface = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(Offscreen(surface.clone()))).unwrap();
    let window = MainWindow::new().unwrap();
    window.show().unwrap();
    window
        .window()
        .dispatch_event(WindowEvent::ScaleFactorChanged {
            scale_factor: SCALE,
        });
    let state = window.global::<State>();
    state.set_osu_dir("/home/player/games/osu!".into());

    draw(&surface, &dir.join("1-main-disconnected.png"));

    state.set_keys_open(true);
    state.set_keys_user("".into());
    state.set_keys_user_missing(true);
    state.set_keys_id_missing(true);
    state.set_keys_secret_missing(true);
    draw(&surface, &dir.join("2-keys-sheet-required.png"));

    state.set_keys_user("SamplePlayer".into());
    state.set_keys_user_from_config(true);
    state.set_keys_id("41234".into());
    state.set_keys_secret("hidden-secret".into());
    state.set_keys_user_missing(false);
    state.set_keys_id_missing(false);
    state.set_keys_secret_missing(false);
    state.set_keys_error("User not found or API keys are invalid".into());
    draw(&surface, &dir.join("3-keys-sheet-rejected.png"));

    state.set_keys_open(false);
    state.set_confirm_open(true);
    draw(&surface, &dir.join("4-logout-confirm.png"));

    state.set_confirm_open(false);
    state.set_connected(true);
    state.set_user_name("SamplePlayer".into());
    state.set_avatar(avatar());
    state.set_has_avatar(true);
    state.set_stats_ready(true);
    state.set_perf_now("8,557".into());
    state.set_perf_potential("8,668".into());
    state.set_acc_now("98.38%".into());
    state.set_acc_potential("97.95%".into());
    state.set_rank("#14,856".into());
    state.set_include_all(true);
    state.set_show_lost(true);
    state.set_status("Scan finished: 340 lost plays, 238 on ranked maps".into());
    state.set_progress(1.0);
    state.set_log(log(&[
        ("[17:50:02]", "osu!: /home/player/games/osu!"),
        ("[17:50:02]", "player: SamplePlayer"),
        (
            "[17:50:09]",
            "14982 replays read in 5.1 s, 12287 of the player's have a local beatmap and pp",
        ),
        (
            "[17:50:15]",
            "asked again about 6 maps whose status can change, 0 changed",
        ),
    ]));
    draw(&surface, &dir.join("5-main-connected.png"));

    state.set_busy(true);
    state.set_progress(0.42);
    state.set_status("Calculating pp: 1,798 of 14,982 replays".into());
    state.set_log(log(&[
        ("[17:50:02]", "osu!: /home/player/games/osu!"),
        ("[17:50:02]", "player: SamplePlayer"),
        (
            "[17:50:07]",
            "14982 replays read in 4.9 s, 12287 of the player's have a local beatmap and pp",
        ),
        (
            "[17:50:08]",
            "asking the osu! API about 1738 maps, about 181 requests at one per second",
        ),
    ]));
    draw(&surface, &dir.join("6-main-scanning.png"));

    state.set_busy(false);
    state.set_progress(0.0);
    state.set_status("Operation failed".into());
    state.set_log(log(&[
        ("[17:50:02]", "osu!: /home/player/games/osu!"),
        (
            "",
            "Error: the osu! API did not accept the keys or does not know this player",
        ),
    ]));
    draw(&surface, &dir.join("7-main-failed.png"));
}

const SEPTEMBER_2024: i64 = 1_725_406_740;

fn hits(n100: u32, n50: u32, misses: u32) -> Hits {
    Hits {
        n300: 400,
        n100,
        n50,
        misses,
    }
}

#[allow(clippy::too_many_arguments)]
fn lost_row(
    artist: &str,
    title: &str,
    version: &str,
    mods: &str,
    grade: Grade,
    status: MapStatus,
    pp: f64,
    kept: f64,
    minutes: i64,
) -> LostRow {
    LostRow {
        pp,
        kept_pp: kept,
        score: 1_234_567,
        kept_score: 1_000_000,
        mods: mods.into(),
        unix_time: SEPTEMBER_2024 - minutes * 60,
        played: "2024-09-03".into(),
        status,
        beatmap_md5: "0b577cbe9bc774600d20cfa17c0aa94c".into(),
        beatmap_id: Some(1_000_000 + minutes as u32),
        beatmapset_id: None,
        artist: artist.into(),
        title: title.into(),
        version: version.into(),
        file: "a.osr".into(),
        creator: "Sample Mapper".into(),
        hits: hits(12, 2, 1),
        accuracy: 98.54,
        grade,
        max_combo: 500,
    }
}

fn weighted_row(place: usize, title: &str, pp: f64, lost: bool, status: MapStatus) -> PotentialRow {
    let weight = 0.95_f64.powi(place as i32 - 1);
    PotentialRow {
        place,
        pp,
        weight,
        weighted: pp * weight,
        lost,
        beatmap_id: 2_000_000 + place as u32,
        beatmapset_id: None,
        artist: "Sample Artist".into(),
        title: title.into(),
        version: "Expert".into(),
        creator: "Sample Mapper".into(),
        mods: "HDDT".into(),
        accuracy: 99.12,
        unix_time: SEPTEMBER_2024 - place as i64 * 3600,
        played: "2024-09-03".into(),
        hits: hits(8, 1, 0),
        score: 2_345_678,
        grade: Grade::SH,
        status,
        kept_pp: lost.then_some(pp - 14.0),
    }
}

fn online_row(place: usize, title: &str, pp: f64) -> OnlineRow {
    let weight = 0.95_f64.powi(place as i32 - 1);
    OnlineRow {
        place,
        pp,
        weight,
        weighted: pp * weight,
        beatmap_id: 3_000_000 + place as u32,
        beatmapset_id: None,
        artist: "Sample Artist".into(),
        title: title.into(),
        version: "Insane".into(),
        creator: "Sample Mapper".into(),
        mods: "".into(),
        accuracy: 97.8,
        grade: Grade::A,
        hits: hits(20, 3, 2),
        score: 1_900_000,
        unix_time: SEPTEMBER_2024 - place as i64 * 7200,
        played: "2024-09-03".into(),
    }
}

fn fake_scan(unicode: bool) -> SavedScan {
    let mut lost = vec![
        lost_row(
            "Sample Artist",
            "Sample Title One",
            "Insane",
            "HDDT",
            Grade::S,
            MapStatus::Ranked,
            411.87,
            396.1,
            10,
        ),
        lost_row(
            "Another Artist",
            "A Very Long Sample Title That Has To Be Cut With An Ellipsis Somewhere Near The End",
            "Extra Long Difficulty Name That Also Needs To Be Elided",
            "HDHR",
            Grade::X,
            MapStatus::Loved,
            388.2,
            360.0,
            200,
        ),
        lost_row(
            "Third Artist",
            "Third Sample",
            "Hard",
            "",
            Grade::A,
            MapStatus::Graveyard,
            301.4,
            300.9,
            4000,
        ),
        lost_row(
            "Fourth Artist",
            "Fourth Sample",
            "Normal",
            "EZNF",
            Grade::B,
            MapStatus::Qualified,
            250.0,
            120.0,
            90_000,
        ),
        lost_row(
            "Fifth Artist",
            "Fifth Sample",
            "Easy",
            "FL",
            Grade::C,
            MapStatus::Unknown,
            199.99,
            150.0,
            120_000,
        ),
        lost_row(
            "Sixth Artist",
            "Sixth Sample",
            "Lunatic",
            "HT",
            Grade::D,
            MapStatus::Ranked,
            120.5,
            119.0,
            300_000,
        ),
        lost_row(
            "Seventh Artist",
            "Camellia Remix",
            "Another",
            "NCHD",
            Grade::SH,
            MapStatus::Ranked,
            99.0,
            10.0,
            400_000,
        ),
    ];
    if unicode {
        lost.insert(
            1,
            lost_row(
                "Кино",
                "Группа крови (Cyrillic sample title)",
                "Лёгкая",
                "HD",
                Grade::S,
                MapStatus::Ranked,
                350.0,
                330.0,
                50,
            ),
        );
        lost.insert(
            2,
            lost_row(
                "初音ミク",
                "千本桜 (Japanese sample title)",
                "超高難度",
                "DT",
                Grade::A,
                MapStatus::Loved,
                340.0,
                300.0,
                70,
            ),
        );
    }
    let rows = vec![
        weighted_row(1, "Potential One", 455.0, true, MapStatus::Ranked),
        weighted_row(2, "Potential Two", 430.5, false, MapStatus::Ranked),
        weighted_row(3, "Potential Three", 420.1, true, MapStatus::Loved),
        weighted_row(4, "Potential Four", 400.0, false, MapStatus::Ranked),
    ];
    let online = vec![
        online_row(1, "Online One", 440.0),
        online_row(2, "Online Two", 430.5),
        online_row(3, "Online Three", 400.0),
    ];
    SavedScan {
        version: VERSION,
        made_at: SEPTEMBER_2024,
        player_id: 1,
        player_name: "SamplePlayer".into(),
        include_all: true,
        check_missing: false,
        lost_total: lost.len(),
        lost_ranked: lost.len() - 3,
        lost,
        potential: Some(PotentialResult {
            current_pp: 8557.4,
            potential_pp: 8668.1,
            current_accuracy: 98.38,
            potential_accuracy: 97.95,
            global_rank: Some(14_856),
            lost_in_top: 2,
            rows,
            online,
        }),
    }
}

struct Results {
    table: Rc<RefCell<Table>>,
    rows: Rc<RowsModel>,
}

impl Results {
    fn new(scan: &SavedScan) -> Self {
        let table = Rc::new(RefCell::new(Table::in_zone(scan, &TimeZone::UTC)));
        Self {
            rows: Rc::new(RowsModel::new(table.clone())),
            table,
        }
    }

    fn present(&self, state: &State<'_>, scan: &SavedScan, tab: Tab) {
        state.set_rows(ModelRc::from(self.rows.clone()));
        state.set_has_results(true);
        state.set_scan_date(when(scan.made_at, &TimeZone::UTC).into());
        state.set_tab(tab.index() as i32);
        state.set_foot(foot(&self.table.borrow(), tab));
        self.rows.show(tab);
        let (column, descending) = self.table.borrow().sorting(tab);
        state.set_sort_column(column.index());
        state.set_sort_desc(descending);
        state.set_search_index(-1);
        state.set_search_label("".into());
    }
}

#[test]
#[ignore = "writes PNG snapshots to $LOST_SCORES_SNAPSHOTS"]
fn render_results_snapshots() {
    let dir = PathBuf::from(std::env::var("LOST_SCORES_SNAPSHOTS").expect("LOST_SCORES_SNAPSHOTS"));
    std::fs::create_dir_all(&dir).unwrap();
    let surface = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(Offscreen(surface.clone()))).unwrap();
    let window = MainWindow::new().unwrap();
    window.show().unwrap();
    window
        .window()
        .dispatch_event(WindowEvent::ScaleFactorChanged {
            scale_factor: SCALE,
        });
    let state = window.global::<State>();
    state.set_show_results(true);

    draw(&surface, &dir.join("8-results-empty.png"));

    let scan = fake_scan(false);
    let results = Results::new(&scan);
    results.present(&state, &scan, Tab::Lost);
    draw(&surface, &dir.join("9-results-lost-narrow.png"));
    draw_sized(&surface, WIDE, &dir.join("10-results-lost-wide.png"));

    results.present(&state, &scan, Tab::Online);
    draw(&surface, &dir.join("11-results-online.png"));

    results.present(&state, &scan, Tab::Potential);
    draw(&surface, &dir.join("12-results-potential.png"));

    results.present(&state, &scan, Tab::Lost);
    results.table.borrow_mut().sort(Tab::Lost, Column::Beatmap);
    results.rows.refresh();
    state.set_sort_column(Column::Beatmap.index());
    state.set_sort_desc(false);
    draw(&surface, &dir.join("13-results-sorted-by-beatmap.png"));

    results.present(&state, &scan, Tab::Lost);
    let found = results.table.borrow().search(Tab::Lost, "camellia");
    state.set_search_label(format!("1/{}", found.len()).into());
    state.set_search_index(found[0] as i32);
    draw(&surface, &dir.join("14-results-search.png"));

    let unicode = fake_scan(true);
    let results = Results::new(&unicode);
    results.present(&state, &unicode, Tab::Lost);
    draw(&surface, &dir.join("15-results-cyrillic-japanese.png"));

    let scan = fake_scan(false);
    let results = Results::new(&scan);
    results.present(&state, &scan, Tab::Lost);
    state.set_connected(true);
    state.set_user_name("SamplePlayer".into());
    state.set_avatar(avatar());
    state.set_has_avatar(true);
    state.set_stats_ready(true);
    state.set_perf_now("8,557".into());
    state.set_perf_potential("8,668".into());
    state.set_acc_now("98.38%".into());
    state.set_acc_potential("97.95%".into());
    state.set_rank("#14,856".into());
    state.set_status("Scan finished: 7 lost plays, 4 on ranked maps".into());
    state.set_progress(1.0);
    state.set_log(log(&[
        ("[17:50:02]", "player: SamplePlayer"),
        ("[17:50:15]", "7 lost plays found, 4 of them on ranked maps"),
    ]));
    state.set_show_results(false);
    draw(&surface, &dir.join("16-main-one-column.png"));
    draw_sized(&surface, COMPACT, &dir.join("17-main-compact.png"));
    draw_sized(&surface, TWO_COLUMNS, &dir.join("18-main-two-columns.png"));
    state.set_show_results(true);
    draw_sized(
        &surface,
        LOGICAL,
        &dir.join("19-results-overlay-with-back.png"),
    );

    state.set_show_results(false);
    state.set_version("0.1.0".into());
    state.set_about_open(true);
    draw(&surface, &dir.join("20-about-sheet.png"));
    state.set_about_open(false);
    state.set_cache_confirm_open(true);
    draw(&surface, &dir.join("21-clear-cache-confirm.png"));

    state.set_cache_confirm_open(false);
    state.set_show_results(true);
    draw_sized(
        &surface,
        (1470.0, 800.0),
        &dir.join("22-results-overlay-1470.png"),
    );
    state.set_show_results(false);
    draw_sized(
        &surface,
        (1480.0, 800.0),
        &dir.join("23-two-columns-1480.png"),
    );
    draw_sized(
        &surface,
        (1480.0, 660.0),
        &dir.join("24-two-columns-660-high.png"),
    );
    state.set_keys_open(true);
    state.set_keys_user("SamplePlayer".into());
    state.set_keys_id("41234".into());
    state.set_keys_secret("hidden-secret".into());
    draw(&surface, &dir.join("25-keys-sheet.png"));
    draw_sized(
        &surface,
        (650.0, 660.0),
        &dir.join("26-keys-sheet-short.png"),
    );
}

fn mods_scan() -> SavedScan {
    let mut scan = fake_scan(false);
    scan.lost.push(lost_row(
        "Eighth Artist",
        "Four Mods Sample",
        "Overdose",
        "HDHRDTFL",
        Grade::S,
        MapStatus::Ranked,
        360.0,
        340.0,
        20,
    ));
    scan.lost.push(lost_row(
        "Ninth Artist",
        "Another Combo Sample",
        "Lunatic",
        "HRDT",
        Grade::A,
        MapStatus::Ranked,
        330.0,
        300.0,
        30,
    ));
    scan.lost_total = scan.lost.len();
    scan
}

fn connect_sample(state: &State<'_>) {
    state.set_connected(true);
    state.set_user_name("SamplePlayer".into());
    state.set_avatar(avatar());
    state.set_has_avatar(true);
    state.set_stats_ready(true);
    state.set_perf_now("8,557".into());
    state.set_perf_potential("8,668".into());
    state.set_perf_tone(1);
    state.set_acc_now("98.38%".into());
    state.set_acc_potential("97.95%".into());
    state.set_acc_tone(-1);
    state.set_rank("#14,856".into());
    state.set_status("Scan finished: 9 lost plays, 6 on ranked maps".into());
    state.set_progress(1.0);
    state.set_log(log(&[
        ("[17:50:02]", "player: SamplePlayer"),
        ("[17:50:15]", "9 lost plays found, 6 of them on ranked maps"),
    ]));
}

#[test]
#[ignore = "writes PNG snapshots to $LOST_SCORES_SNAPSHOTS"]
fn render_walkthrough_snapshots() {
    let dir = PathBuf::from(std::env::var("LOST_SCORES_SNAPSHOTS").expect("LOST_SCORES_SNAPSHOTS"));
    std::fs::create_dir_all(&dir).unwrap();
    let surface = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(Offscreen(surface.clone()))).unwrap();
    let window = MainWindow::new().unwrap();
    window.show().unwrap();
    window
        .window()
        .dispatch_event(WindowEvent::ScaleFactorChanged {
            scale_factor: SCALE,
        });
    let state = window.global::<State>();
    state.set_osu_dir("/home/player/games/osu!".into());
    connect_sample(&state);
    state.set_include_all(false);
    state.set_check_missing(false);
    state.set_show_lost(false);
    draw(&surface, &dir.join("w01-card-toggles-off.png"));
    state.set_include_all(true);
    state.set_check_missing(true);
    state.set_show_lost(true);
    draw(&surface, &dir.join("w02-card-toggles-on.png"));

    let scan = mods_scan();
    let results = Results::new(&scan);
    state.set_mods_width(lost_scores_gui::view::mods_width(
        results.table.borrow().rows(Tab::Lost),
    ));
    results.present(&state, &scan, Tab::Lost);
    state.set_show_results(true);
    draw(&surface, &dir.join("w03-results-four-mods-narrow.png"));
    draw_sized(&surface, WIDE, &dir.join("w04-results-four-mods-wide.png"));
    state.set_show_results(false);
    draw_sized(
        &surface,
        (1480.0, 800.0),
        &dir.join("w05-two-columns-1480x800.png"),
    );
    draw_sized(
        &surface,
        (2048.0, 1280.0),
        &dir.join("w06-two-columns-2048x1280.png"),
    );
    state.set_show_results(true);

    let sorts = [
        ("w07-sort-beatmap", Column::Beatmap, 1),
        ("w08-sort-mods", Column::Mods, 1),
        ("w09-sort-mods-reversed", Column::Mods, 2),
        ("w10-sort-hits", Column::Hits, 1),
        ("w11-sort-score", Column::Score, 1),
        ("w12-sort-accuracy", Column::Accuracy, 1),
        ("w13-sort-pp", Column::Pp, 1),
    ];
    for (name, column, clicks) in sorts {
        results.present(&state, &scan, Tab::Lost);
        for _ in 0..clicks {
            results.table.borrow_mut().sort(Tab::Lost, column);
        }
        results.rows.refresh();
        let (column, descending) = results.table.borrow().sorting(Tab::Lost);
        state.set_sort_column(column.index());
        state.set_sort_desc(descending);
        draw_sized(&surface, WIDE, &dir.join(format!("{name}.png")));
    }
}

#[test]
#[ignore = "writes PNG snapshots to $LOST_SCORES_SNAPSHOTS"]
fn render_walkthrough2_snapshots() {
    let dir = PathBuf::from(std::env::var("LOST_SCORES_SNAPSHOTS").expect("LOST_SCORES_SNAPSHOTS"));
    std::fs::create_dir_all(&dir).unwrap();
    let surface = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(Offscreen(surface.clone()))).unwrap();
    let window = MainWindow::new().unwrap();
    window.show().unwrap();
    window
        .window()
        .dispatch_event(WindowEvent::ScaleFactorChanged {
            scale_factor: SCALE,
        });
    let state = window.global::<State>();
    state.set_osu_dir("/home/player/games/osu!".into());
    connect_sample(&state);
    state.set_include_all(false);
    state.set_check_missing(false);
    state.set_show_lost(false);
    draw(&surface, &dir.join("w14-toggle-icons-off.png"));
    state.set_include_all(true);
    state.set_check_missing(true);
    state.set_show_lost(true);
    draw(&surface, &dir.join("w15-toggle-icons-on.png"));

    let scan = fake_scan(false);
    let results = Results::new(&scan);
    results.present(&state, &scan, Tab::Lost);
    state.set_show_results(true);
    draw(&surface, &dir.join("w16-results-overlay-settled.png"));
    state.set_show_results(false);
    draw(&surface, &dir.join("w17-controls-after-back.png"));

    state.set_show_results(true);
    state.set_scans(ModelRc::new(VecModel::from(vec![
        "14 November 2026 · 09:44".into(),
        "14 November 2026 · 09:32".into(),
        "14 November 2026 · 08:57".into(),
    ])));
    state.set_scan_selected(0);
    state.set_scans_open(true);
    draw(&surface, &dir.join("w18-scan-list.png"));
}
