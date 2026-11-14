use std::cell::RefCell;
use std::rc::Rc;

use slint::{ComponentHandle, LogicalSize};

use crate::{MainWindow, State};

#[derive(Default)]
pub struct Fullscreen {
    undo: Option<LogicalSize>,
}

impl Fullscreen {
    pub fn enter(&mut self, window: &slint::Window) {
        self.undo =
            (!window.is_maximized()).then(|| window.size().to_logical(window.scale_factor()));
        window.set_maximized(true);
    }

    pub fn leave(&mut self, window: &slint::Window) {
        if let Some(size) = self.undo.take() {
            window.set_maximized(false);
            window.set_size(size);
        }
    }

    pub fn forget(&mut self) {
        self.undo = None;
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
    let leaving = fullscreen.clone();
    state.on_back(move || {
        if let Some(window) = weak.upgrade() {
            window.global::<State>().set_show_results(false);
            leaving.borrow_mut().leave(window.window());
        }
    });
    state.on_columns_took_over(move || fullscreen.borrow_mut().forget());
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
        assert!(!window.window().is_maximized());
    }

    #[test]
    fn back_keeps_a_window_the_user_had_maximized() {
        i_slint_backend_testing::init_no_event_loop();
        let window = MainWindow::new().unwrap();
        window.show().unwrap();
        window.window().set_size(LogicalSize::new(700.0, 800.0));
        bind(&window);
        let state = window.global::<State>();

        state.invoke_see_full_results();
        state.invoke_back();
        window.window().set_maximized(true);
        window.window().set_size(LogicalSize::new(1400.0, 900.0));

        state.invoke_see_full_results();
        state.invoke_back();
        assert!(window.window().is_maximized());
        assert_eq!(window.window().size(), PhysicalSize::new(1400, 900));
    }

    #[test]
    fn maximizing_into_two_columns_drops_the_undo_so_back_does_not_shrink_it() {
        i_slint_backend_testing::init_no_event_loop();
        let window = MainWindow::new().unwrap();
        window.show().unwrap();
        window.window().set_size(LogicalSize::new(700.0, 800.0));
        bind(&window);
        let state = window.global::<State>();

        state.invoke_see_full_results();
        window.window().set_size(LogicalSize::new(1600.0, 900.0));
        slint::platform::update_timers_and_animations();
        assert!(window.get_two_columns());
        assert!(!state.get_show_results());

        state.invoke_back();
        assert!(window.window().is_maximized());
        assert_eq!(window.window().size(), PhysicalSize::new(1600, 900));
    }
}
