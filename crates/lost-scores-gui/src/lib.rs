pub mod account;
pub mod app;
pub mod avatar;
pub mod layout;
pub mod log;
pub mod newest;
pub mod scan;
pub mod scans;
pub mod share;
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
    fn only_older_scans_have_a_trash_button() {
        i_slint_backend_testing::init_no_event_loop();
        let window = MainWindow::new().unwrap();
        window.show().unwrap();
        window.window().set_size(LogicalSize::new(650.0, 800.0));
        let labels = [
            "15 November 2026 · 10:00",
            "14 November 2026 · 09:00",
            "13 November 2026 · 08:00",
        ];
        let state = window.global::<State>();
        state.set_scans(ModelRc::new(VecModel::from(
            labels.map(SharedString::from).to_vec(),
        )));
        let deleted = Rc::new(Cell::new(-1));
        let older = Rc::new(Cell::new(0));
        let (one, many) = (deleted.clone(), older.clone());
        state.on_delete_scan(move |index| one.set(index));
        state.on_delete_older_scans(move || many.set(many.get() + 1));
        state.set_has_results(true);
        state.set_show_results(true);
        state.set_scans_open(true);
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(250));

        let buttons: Vec<_> =
            ElementHandle::find_by_accessible_label(&window, "Move to trash").collect();
        assert_eq!(buttons.len(), 2);
        buttons[1].mock_single_click(PointerEventButton::Left);
        assert_eq!(deleted.get(), 2);
        buttons[0].mock_single_click(PointerEventButton::Left);
        assert_eq!(deleted.get(), 1);

        ElementHandle::find_by_accessible_label(&window, "Delete older scans")
            .next()
            .unwrap()
            .mock_single_click(PointerEventButton::Left);
        assert_eq!(older.get(), 1);
    }

    fn header_window(width: f32) -> MainWindow {
        i_slint_backend_testing::init_no_event_loop();
        let window = MainWindow::new().unwrap();
        window.show().unwrap();
        window.window().set_size(LogicalSize::new(width, 800.0));
        let state = window.global::<State>();
        state.set_has_results(true);
        state.set_show_results(true);
        state.set_has_images(true);
        let weak = window.as_weak();
        state.on_open_share(move || {
            let window = weak.upgrade().unwrap();
            let state = window.global::<State>();
            state.set_share_images(ModelRc::new(VecModel::from(vec![SharedString::from(
                "Lost Scores",
            )])));
            state.set_share_open(true);
        });
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(250));
        window
    }

    const SHARE_TIP: &str = "Share the images of this scan";

    #[test]
    fn the_narrow_header_has_one_share_button() {
        let window = header_window(650.0);
        let count = ElementHandle::find_by_accessible_label(&window, SHARE_TIP).count();
        assert_eq!(count, 1);
    }

    #[test]
    fn the_share_popup_opens_by_its_button_in_two_columns() {
        let window = header_window(1600.0);
        let buttons: Vec<_> = ElementHandle::find_by_accessible_label(&window, SHARE_TIP).collect();
        assert_eq!(buttons.len(), 1);
        let button_x = buttons[0].absolute_position().x;
        buttons[0].mock_single_click(PointerEventButton::Left);
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(250));
        let copy = ElementHandle::find_by_accessible_label(&window, "Copy image")
            .next()
            .unwrap();
        let copy_x = copy.absolute_position().x;
        assert!(
            copy_x > button_x && copy_x < button_x + 392.0,
            "button {button_x}, copy {copy_x}"
        );
    }

    #[test]
    fn the_share_popup_has_a_row_for_each_image() {
        i_slint_backend_testing::init_no_event_loop();
        let window = MainWindow::new().unwrap();
        window.show().unwrap();
        window.window().set_size(LogicalSize::new(650.0, 800.0));
        let state = window.global::<State>();
        state.set_share_images(ModelRc::new(VecModel::from(
            ["Lost Scores", "Badge"].map(SharedString::from).to_vec(),
        )));
        let copied = Rc::new(Cell::new(-1));
        let uploaded = Rc::new(Cell::new(-1));
        let (copy, upload) = (copied.clone(), uploaded.clone());
        state.on_copy_image(move |index| copy.set(index));
        state.on_upload_image(move |index| upload.set(index));
        state.set_has_results(true);
        state.set_show_results(true);
        state.set_has_images(true);
        state.set_share_open(true);
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(250));

        let count = |label: &str| ElementHandle::find_by_accessible_label(&window, label).count();
        assert_eq!(count("Badge"), 1);
        assert_eq!(count("Copy image"), 2);
        let buttons: Vec<_> =
            ElementHandle::find_by_accessible_label(&window, "Upload to catbox").collect();
        assert_eq!(buttons.len(), 2);
        buttons[1].mock_single_click(PointerEventButton::Left);
        assert_eq!(uploaded.get(), 1);
        assert_eq!(copied.get(), -1);
    }

    #[test]
    fn the_upload_sheet_reports_the_checkbox() {
        i_slint_backend_testing::init_no_event_loop();
        let window = MainWindow::new().unwrap();
        window.show().unwrap();
        window.window().set_size(LogicalSize::new(650.0, 800.0));
        let state = window.global::<State>();
        let answers = Rc::new(std::cell::RefCell::new(Vec::new()));
        let log = answers.clone();
        state.on_confirm_upload(move |remember| log.borrow_mut().push(remember));
        state.set_upload_confirm_open(true);
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(250));

        let upload = || {
            ElementHandle::find_by_accessible_label(&window, "Upload")
                .next()
                .unwrap()
        };
        upload().mock_single_click(PointerEventButton::Left);
        ElementHandle::find_by_accessible_label(&window, "Don't ask again")
            .next()
            .unwrap()
            .mock_single_click(PointerEventButton::Left);
        upload().mock_single_click(PointerEventButton::Left);
        assert_eq!(*answers.borrow(), [false, true]);
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
