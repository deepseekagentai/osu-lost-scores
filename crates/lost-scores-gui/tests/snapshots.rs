use std::path::PathBuf;
use std::rc::Rc;
use std::thread;
use std::time::Duration;

use image::{Rgba, RgbaImage};
use lost_scores_gui::avatar::render_avatar;
use lost_scores_gui::{LogEntry, MainWindow, State};
use slint::platform::WindowEvent;
use slint::platform::software_renderer::{
    MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType,
};
use slint::platform::{Platform, WindowAdapter};
use slint::{ComponentHandle, ModelRc, PhysicalSize, VecModel};

const SCALE: f32 = 1.25;
const LOGICAL: (f32, f32) = (650.0, 800.0);

struct Offscreen(Rc<MinimalSoftwareWindow>);

impl Platform for Offscreen {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
}

fn draw(surface: &Rc<MinimalSoftwareWindow>, path: &PathBuf) {
    let width = (LOGICAL.0 * SCALE) as u32;
    let height = (LOGICAL.1 * SCALE) as u32;
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
