use std::rc::Rc;
use std::thread;
use std::time::Duration;

use i_slint_backend_testing::ElementHandle;
use lost_scores_gui::{MainWindow, State};
use slint::ComponentHandle;
use slint::PhysicalSize;
use slint::platform::software_renderer::{
    MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType,
};
use slint::platform::{Platform, WindowAdapter, WindowEvent};

const BUTTON: [u8; 3] = [0x3a, 0x37, 0x43];

struct Offscreen(Rc<MinimalSoftwareWindow>);

impl Platform for Offscreen {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
}

#[test]
fn the_secret_eye_has_no_background_until_hovered() {
    let surface = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(Offscreen(surface.clone()))).unwrap();
    let window = MainWindow::new().unwrap();
    window.show().unwrap();
    window
        .window()
        .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: 1.0 });
    let (width, height) = (650_u32, 800_u32);
    surface.set_size(PhysicalSize::new(width, height));
    window.global::<State>().set_keys_open(true);

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

    let eye = ElementHandle::find_by_accessible_label(&window, "Show the secret")
        .next()
        .expect("the eye button");
    let corner = eye.absolute_position();
    let pixel = buffer[((corner.y as u32 + 3) * width + corner.x as u32 + 3) as usize];
    let near = |a: u8, b: u8| a.abs_diff(b) < 6;
    let rgb = [pixel.red, pixel.green, pixel.blue];
    assert!(
        !(0..3).all(|channel| near(rgb[channel], BUTTON[channel])),
        "the corner of the eye button is the button background {rgb:?}"
    );
}
