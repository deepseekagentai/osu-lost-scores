pub mod account;
pub mod app;
pub mod avatar;
pub mod layout;
pub mod log;
pub mod newest;
pub mod scan;
pub mod table;
pub mod update;
pub mod view;
pub mod worker;

use std::path::Path;

use anyhow::Result;
use lost_scores_app::setup;

slint::include_modules!();

pub const APP_ID: &str = "kz.lemon4ik.LostScores";

pub fn show_path(path: &Path) -> String {
    path.display().to_string()
}

pub fn choose_folder(dir: &Path) -> Result<String> {
    setup::resolve_osu_dir(Some(dir)).map(|chosen| show_path(&chosen))
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use i_slint_backend_testing::ElementHandle;
    use slint::platform::{PointerEventButton, WindowEvent};
    use slint::{ComponentHandle, LogicalPosition, LogicalSize, ModelRc, SharedString, VecModel};

    use super::{MainWindow, State, choose_folder};

    fn click_start_scan(window: &MainWindow) {
        ElementHandle::find_by_element_type_name(window, "StartScan")
            .next()
            .unwrap()
            .mock_single_click(PointerEventButton::Left);
    }

    #[test]
    fn clicks_do_not_reach_the_controls_under_the_results_overlay() {
        i_slint_backend_testing::init_integration_test_with_mock_time();
        let window = MainWindow::new().unwrap();
        window.show().unwrap();
        window.window().set_size(LogicalSize::new(650.0, 800.0));
        let started = Rc::new(Cell::new(0));
        let counter = started.clone();
        window
            .global::<State>()
            .on_start_scan(move || counter.set(counter.get() + 1));

        window.window().dispatch_event(WindowEvent::PointerMoved {
            position: LogicalPosition::new(1.0, 1.0),
        });
        click_start_scan(&window);
        assert_eq!(started.get(), 1);

        window.global::<State>().set_show_results(true);
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(250));
        click_start_scan(&window);
        assert_eq!(started.get(), 1);
    }

    #[test]
    fn the_newest_scan_is_listed_above_older_scans() {
        i_slint_backend_testing::init_no_event_loop();
        let window = MainWindow::new().unwrap();
        window.show().unwrap();
        window.window().set_size(LogicalSize::new(650.0, 800.0));
        let labels = [
            "15 Nov 2026 · 10:00",
            "14 Nov 2026 · 09:00",
            "13 Nov 2026 · 08:00",
        ];
        let state = window.global::<State>();
        state.set_scans(ModelRc::new(VecModel::from(
            labels.map(SharedString::from).to_vec(),
        )));
        let selected = Rc::new(Cell::new(-1));
        let chosen = selected.clone();
        state.on_select_scan(move |index| chosen.set(index));
        state.set_has_results(true);
        state.set_show_results(true);
        state.set_scans_open(true);
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(250));

        let find = |label: &str| {
            let found: Vec<_> = ElementHandle::find_by_accessible_label(&window, label).collect();
            assert_eq!(found.len(), 1, "{label}");
            found.into_iter().next().unwrap()
        };
        let y = |label: &str| find(label).absolute_position().y;
        assert!(y(labels[0]) < y("Older scans"));
        assert!(y("Older scans") < y(labels[1]));
        assert!(y(labels[1]) < y(labels[2]));

        find(labels[2]).mock_single_click(PointerEventButton::Left);
        assert_eq!(selected.get(), 2);
    }

    #[test]
    fn a_folder_without_data_r_is_refused_with_the_cli_message() {
        let dir = tempfile::tempdir().unwrap();
        let error = choose_folder(dir.path()).unwrap_err().to_string();
        assert!(error.ends_with("is not an osu! folder: there is no Data/r inside"));
    }

    #[test]
    fn the_window_builds() {
        i_slint_backend_testing::init_no_event_loop();
        let window = MainWindow::new().unwrap();
        assert_eq!(window.get_product(), "osu! Lost Scores Analyzer");
    }
}
