use std::rc::Rc;
use std::thread;
use std::time::Duration;

use lost_scores_gui::{MainWindow, State};
use slint::platform::software_renderer::{
    MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType,
};
use slint::platform::{Platform, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, ModelRc, PhysicalSize, SharedString, VecModel};

const SCALE: f32 = 1.0;
const ACCENT: [u8; 3] = [0xa2, 0x8a, 0xea];

struct Offscreen(Rc<MinimalSoftwareWindow>);

impl Platform for Offscreen {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
}

fn draw(surface: &Rc<MinimalSoftwareWindow>, width: u32, height: u32) -> Vec<[u8; 3]> {
    slint::platform::update_timers_and_animations();
    thread::sleep(Duration::from_millis(500));
    for _ in 0..3 {
        slint::platform::update_timers_and_animations();
        surface.request_redraw();
    }
    let mut buffer = vec![PremultipliedRgbaColor::default(); (width * height) as usize];
    surface.draw_if_needed(|renderer| {
        renderer.render(&mut buffer, width as usize);
    });
    buffer.iter().map(|p| [p.red, p.green, p.blue]).collect()
}

fn accent_pixels(pixels: &[[u8; 3]], width: u32, x: u32, y: u32, size: u32) -> usize {
    let near = |a: u8, b: u8| a.abs_diff(b) < 24;
    (y..y + size)
        .flat_map(|row| (x..x + size).map(move |column| (column, row)))
        .filter(|&(column, row)| {
            let pixel = pixels[(row * width + column) as usize];
            (0..3).all(|channel| near(pixel[channel], ACCENT[channel]))
        })
        .count()
}

#[test]
fn the_cross_on_an_older_scan_has_no_outline_when_hovered() {
    let surface = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(Offscreen(surface.clone()))).unwrap();
    let window = MainWindow::new().unwrap();
    window.show().unwrap();
    window
        .window()
        .dispatch_event(WindowEvent::ScaleFactorChanged {
            scale_factor: SCALE,
        });
    let (width, height) = (650_u32, 800_u32);
    surface.set_size(PhysicalSize::new(width, height));
    let state = window.global::<State>();
    let labels = [
        "14 November 2026 \u{b7} 09:32",
        "13 November 2026 \u{b7} 21:05",
    ];
    state.set_scans(ModelRc::new(VecModel::from(
        labels.map(SharedString::from).to_vec(),
    )));
    state.set_scan_selected(0);
    state.set_has_results(true);
    state.set_show_results(true);
    state.set_scans_open(true);

    let cross = (410.0, 157.0);
    for _ in 0..2 {
        slint::platform::update_timers_and_animations();
        surface.dispatch_event(WindowEvent::PointerMoved {
            position: LogicalPosition::new(cross.0, cross.1),
        });
    }
    let pixels = draw(&surface, width, height);
    let around_cross = accent_pixels(&pixels, width, 380, 142, 30);
    assert_eq!(around_cross, 0, "accent pixels around the hovered cross");
}
