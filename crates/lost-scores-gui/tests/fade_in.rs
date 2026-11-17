use std::rc::Rc;
use std::thread;
use std::time::Duration;

use lost_scores_gui::{MainWindow, State};
use slint::ComponentHandle;
use slint::PhysicalSize;
use slint::platform::software_renderer::{
    MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType,
};
use slint::platform::{Platform, WindowAdapter, WindowEvent};

const WIDTH: u32 = 650;
const HEIGHT: u32 = 800;

struct Offscreen(Rc<MinimalSoftwareWindow>);

impl Platform for Offscreen {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
}

fn frame(surface: &Rc<MinimalSoftwareWindow>) -> Vec<u8> {
    slint::platform::update_timers_and_animations();
    surface.request_redraw();
    let mut buffer = vec![PremultipliedRgbaColor::default(); (WIDTH * HEIGHT) as usize];
    surface.draw_if_needed(|renderer| {
        renderer.render(&mut buffer, WIDTH as usize);
    });
    buffer
        .iter()
        .flat_map(|p| [p.red, p.green, p.blue])
        .collect()
}

#[test]
fn the_about_sheet_fades_in() {
    let surface = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(Offscreen(surface.clone()))).unwrap();
    let window = MainWindow::new().unwrap();
    window.show().unwrap();
    window
        .window()
        .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: 1.0 });
    surface.set_size(PhysicalSize::new(WIDTH, HEIGHT));
    frame(&surface);
    window.global::<State>().set_about_open(true);
    frame(&surface);
    let opening = frame(&surface);
    thread::sleep(Duration::from_millis(400));
    let open = frame(&surface);
    thread::sleep(Duration::from_millis(400));
    let settled = frame(&surface);
    assert!(open == settled, "the sheet is still moving after 400 ms");
    assert!(opening != open, "the sheet popped up without fading in");
}
