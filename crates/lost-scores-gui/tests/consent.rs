use std::cell::RefCell;
use std::rc::Rc;

use i_slint_backend_testing::ElementHandle;
use lost_scores_gui::{MainWindow, State};
use slint::ComponentHandle;
use slint::PhysicalSize;
use slint::platform::software_renderer::{
    MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType,
};
use slint::platform::{Platform, WindowAdapter, WindowEvent};

struct Offscreen(Rc<MinimalSoftwareWindow>);

impl Platform for Offscreen {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
}

fn element(window: &MainWindow, label: &str) -> ElementHandle {
    ElementHandle::find_by_accessible_label(window, label)
        .next()
        .unwrap_or_else(|| panic!("no element labelled {label:?}"))
}

#[test]
fn the_login_panel_links_the_terms_and_the_privacy_policy_inside_the_narrowest_window() {
    let surface = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(Offscreen(surface.clone()))).unwrap();
    let window = MainWindow::new().unwrap();
    window.show().unwrap();
    window
        .window()
        .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: 1.0 });
    surface.set_size(PhysicalSize::new(650, 800));
    slint::platform::update_timers_and_animations();

    let state = window.global::<State>();
    let opened = Rc::new(RefCell::new(Vec::new()));
    let terms_opened = opened.clone();
    state.on_open_terms(move || terms_opened.borrow_mut().push("terms"));
    let privacy_opened = opened.clone();
    state.on_open_privacy(move || privacy_opened.borrow_mut().push("privacy"));

    let button = element(&window, "Log in with osu!");
    let lead = element(&window, "By logging in, you accept the");
    let terms = element(&window, "Terms");
    let privacy = element(&window, "Privacy policy");

    let left = button.absolute_position().x;
    let right = left + button.size().width;
    assert!(
        lead.absolute_position().x >= left,
        "the line starts left of the button"
    );
    assert!(
        privacy.absolute_position().x + privacy.size().width <= right,
        "the line ends right of the button"
    );
    assert!(
        lead.absolute_position().y > button.absolute_position().y,
        "the line is not under the button"
    );

    terms.invoke_accessible_default_action();
    privacy.invoke_accessible_default_action();
    assert_eq!(*opened.borrow(), ["terms", "privacy"]);
}

#[test]
fn the_bottom_link_stays_inside_the_shortest_window_before_login() {
    let surface = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(Offscreen(surface.clone()))).unwrap();
    let window = MainWindow::new().unwrap();
    window.show().unwrap();
    window
        .window()
        .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: 1.0 });
    let (width, height) = (650_u32, 680_u32);
    surface.set_size(PhysicalSize::new(width, height));
    let mut buffer = vec![PremultipliedRgbaColor::default(); (width * height) as usize];
    for _ in 0..2 {
        slint::platform::update_timers_and_animations();
        surface.request_redraw();
        surface.draw_if_needed(|renderer| {
            renderer.render(&mut buffer, width as usize);
        });
    }

    let link = element(&window, "Hall of Copium");
    let bottom = link.absolute_position().y + link.size().height;
    assert!(
        bottom <= height as f32,
        "the Hall of Copium link ends at {bottom}, below the {height} px window"
    );
}
