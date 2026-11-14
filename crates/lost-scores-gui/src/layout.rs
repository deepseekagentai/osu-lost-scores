use std::cell::RefCell;
use std::rc::Rc;

use slint::{ComponentHandle, LogicalSize};

use crate::{MainWindow, State};

#[derive(Default)]
pub struct Fullscreen {
    before: Option<LogicalSize>,
}

impl Fullscreen {
    pub fn enter(&mut self, window: &slint::Window) {
        if !window.is_maximized() {
            self.before = Some(window.size().to_logical(window.scale_factor()));
        }
        window.set_maximized(true);
    }

    pub fn leave(&mut self, window: &slint::Window) {
        window.set_maximized(false);
        if let Some(size) = self.before.take() {
            window.set_size(size);
        }
    }
}

pub fn bind(window: &MainWindow) {
    let fullscreen = Rc::new(RefCell::new(Fullscreen::default()));
    let state = window.global::<State>();
    let weak = window.as_weak();
    let remembered = fullscreen.clone();
    state.on_see_full_results(move || {
        if let Some(window) = weak.upgrade() {
            remembered.borrow_mut().enter(window.window());
            window.global::<State>().set_show_results(true);
        }
    });
    let weak = window.as_weak();
    state.on_back(move || {
        if let Some(window) = weak.upgrade() {
            window.global::<State>().set_show_results(false);
            fullscreen.borrow_mut().leave(window.window());
        }
    });
}

#[cfg(test)]
mod tests {
    use slint::{ComponentHandle, LogicalSize, PhysicalSize};

    use super::bind;
    use crate::{MainWindow, State};

    #[test]
    fn back_restores_the_size_before_maximize() {
        i_slint_backend_testing::init_no_event_loop();
        let window = MainWindow::new().unwrap();
        window.show().unwrap();
        window.window().set_size(LogicalSize::new(700.0, 800.0));
        bind(&window);
        let state = window.global::<State>();

        state.invoke_see_full_results();
        assert!(state.get_show_results());
        window.window().set_size(LogicalSize::new(1400.0, 900.0));

        state.invoke_back();
        assert!(!state.get_show_results());
        assert_eq!(window.window().size(), PhysicalSize::new(700, 800));
    }
}
