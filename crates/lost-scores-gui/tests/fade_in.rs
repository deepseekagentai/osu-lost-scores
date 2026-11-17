use std::cell::Cell;
use std::rc::Rc;
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

struct Offscreen {
    surface: Rc<MinimalSoftwareWindow>,
    now: Rc<Cell<Duration>>,
}

impl Platform for Offscreen {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.surface.clone())
    }

    fn duration_since_start(&self) -> Duration {
        self.now.get()
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
    let now = Rc::new(Cell::new(Duration::ZERO));
    slint::platform::set_platform(Box::new(Offscreen {
        surface: surface.clone(),
        now: now.clone(),
    }))
    .unwrap();
    let window = MainWindow::new().unwrap();
    window.show().unwrap();
    window
        .window()
        .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: 1.0 });
    surface.set_size(PhysicalSize::new(WIDTH, HEIGHT));
    frame(&surface);
    now.set(Duration::from_secs(5));
    frame(&surface);
    window.global::<State>().set_about_open(true);
    frame(&surface);
    now.set(Duration::from_millis(5030));
    let opening = frame(&surface);
    now.set(Duration::from_secs(6));
    let open = frame(&surface);
    now.set(Duration::from_secs(7));
    let settled = frame(&surface);
    assert!(open == settled, "the sheet is still moving after a second");
    assert!(opening != open, "the sheet popped up without fading in");
}
