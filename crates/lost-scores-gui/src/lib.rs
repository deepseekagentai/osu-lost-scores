pub mod account;
pub mod app;
pub mod avatar;
pub mod clipboard;
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
    use slint::platform::{Key, PointerEventButton, WindowEvent};
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

    #[test]
    fn delete_older_scans_is_set_apart_below_the_list() {
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
        state.set_has_results(true);
        state.set_show_results(true);
        state.set_scans_open(true);
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(250));

        let y = |label: &str| {
            ElementHandle::find_by_accessible_label(&window, label)
                .next()
                .unwrap()
                .absolute_position()
                .y
        };
        let gap = y("Delete older scans") - y(labels[2]);
        assert!(gap >= 33.0, "gap {gap}");
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
    fn start_scan_sees_the_text_still_being_typed_in_scores_to_show() {
        i_slint_backend_testing::init_no_event_loop();
        let window = MainWindow::new().unwrap();
        window.show().unwrap();
        window.window().set_size(LogicalSize::new(650.0, 800.0));
        let seen = Rc::new(std::cell::RefCell::new(String::from("unset")));
        let record = seen.clone();
        let weak = window.as_weak();
        let state = window.global::<State>();
        state.set_connected(true);
        state.on_start_scan(move || {
            let window = weak.upgrade().unwrap();
            *record.borrow_mut() = window.global::<State>().get_scores_pending().to_string();
        });

        ElementHandle::find_by_element_type_name(&window, "ScoresBox")
            .next()
            .unwrap()
            .mock_single_click(PointerEventButton::Left);
        for event in [
            WindowEvent::KeyPressed { text: "3".into() },
            WindowEvent::KeyReleased { text: "3".into() },
        ] {
            window.window().dispatch_event(event);
        }
        click_start_scan(&window);
        assert_eq!(*seen.borrow(), "3");
    }

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
        let copy = ElementHandle::find_by_accessible_label(&window, "Copy")
            .next()
            .unwrap();
        let copy_x = copy.absolute_position().x;
        assert!(
            copy_x > button_x && copy_x < button_x + 340.0,
            "button {button_x}, copy {copy_x}"
        );
    }

    #[test]
    fn the_share_popup_has_open_copy_and_catbox_in_each_row() {
        i_slint_backend_testing::init_no_event_loop();
        let window = MainWindow::new().unwrap();
        window.show().unwrap();
        window.window().set_size(LogicalSize::new(650.0, 800.0));
        let state = window.global::<State>();
        state.set_share_images(ModelRc::new(VecModel::from(
            ["Lost Scores", "Badge"].map(SharedString::from).to_vec(),
        )));
        let calls = Rc::new(std::cell::RefCell::new(Vec::new()));
        let (open, copy, upload) = (calls.clone(), calls.clone(), calls.clone());
        state.on_open_image(move |index| open.borrow_mut().push(("open", index)));
        state.on_copy_image(move |index| copy.borrow_mut().push(("copy", index)));
        state.on_upload_image(move |index| upload.borrow_mut().push(("catbox", index)));
        state.set_has_results(true);
        state.set_show_results(true);
        state.set_has_images(true);
        state.set_share_open(true);
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(250));

        let count = |label: &str| ElementHandle::find_by_accessible_label(&window, label).count();
        assert_eq!(count("Badge"), 1);
        for label in ["Open", "Copy", "catbox"] {
            assert_eq!(count(label), 2, "{label}");
        }
        for (label, index) in [("Open", 1), ("Copy", 0), ("catbox", 1)] {
            let buttons: Vec<_> = ElementHandle::find_by_accessible_label(&window, label).collect();
            buttons[index].mock_single_click(PointerEventButton::Left);
        }
        assert_eq!(*calls.borrow(), [("open", 1), ("copy", 0), ("catbox", 1)]);
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

    fn press(window: &MainWindow, key: Key) {
        let text = SharedString::from(key);
        window
            .window()
            .dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
        window
            .window()
            .dispatch_event(WindowEvent::KeyReleased { text });
    }

    type Calls = Rc<std::cell::RefCell<Vec<&'static str>>>;

    fn keyboard_window() -> (MainWindow, Calls) {
        i_slint_backend_testing::init_no_event_loop();
        let window = MainWindow::new().unwrap();
        window.show().unwrap();
        window.window().set_size(LogicalSize::new(650.0, 800.0));
        let calls: Calls = Rc::default();
        let state = window.global::<State>();
        let record = |name: &'static str| {
            let calls = calls.clone();
            move || calls.borrow_mut().push(name)
        };
        state.on_cancel_logout(record("cancel-logout"));
        state.on_confirm_logout(record("confirm-logout"));
        state.on_cancel_keys(record("cancel-keys"));
        state.on_save_keys(record("save-keys"));
        state.on_cancel_delete(record("cancel-delete"));
        state.on_confirm_delete(record("confirm-delete"));
        state.on_cancel_clear_cache(record("cancel-cache"));
        state.on_confirm_clear_cache(record("confirm-cache"));
        state.on_cancel_upload(record("cancel-upload"));
        state.on_close_about(record("close-about"));
        state.on_back(record("back"));
        let searches = calls.clone();
        state.on_search_changed(move |text| {
            searches.borrow_mut().push(if text.is_empty() {
                "search-cleared"
            } else {
                "search-edited"
            })
        });
        let uploads = calls.clone();
        state.on_confirm_upload(move |_| uploads.borrow_mut().push("confirm-upload"));
        (window, calls)
    }

    fn settle() {
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(250));
    }

    #[test]
    fn escape_closes_one_layer_at_a_time_from_the_top() {
        let (window, calls) = keyboard_window();
        let state = window.global::<State>();
        state.set_has_results(true);
        state.set_show_results(true);
        state.set_confirm_open(true);
        state.set_share_open(true);
        settle();

        press(&window, Key::Escape);
        assert!(!state.get_share_open());
        assert!(calls.borrow().is_empty());

        press(&window, Key::Escape);
        assert_eq!(*calls.borrow(), ["cancel-logout"]);
        state.set_confirm_open(false);
        settle();

        press(&window, Key::Escape);
        assert_eq!(*calls.borrow(), ["cancel-logout", "back"]);
    }

    #[test]
    fn escape_closes_the_scan_list_before_the_overlay_goes_back() {
        let (window, calls) = keyboard_window();
        let state = window.global::<State>();
        state.set_has_results(true);
        state.set_show_results(true);
        state.set_scans_open(true);
        settle();
        press(&window, Key::Escape);
        assert!(!state.get_scans_open());
        assert!(calls.borrow().is_empty());
        press(&window, Key::Escape);
        assert_eq!(*calls.borrow(), ["back"]);
    }

    #[test]
    fn escape_clears_the_search_text_before_the_overlay_goes_back() {
        let (window, calls) = keyboard_window();
        let state = window.global::<State>();
        state.set_has_results(true);
        state.set_show_results(true);
        state.set_search_text("camellia".into());
        state.set_search_index(3);
        settle();

        press(&window, Key::Escape);
        assert_eq!(state.get_search_text(), "");
        assert!(state.get_show_results());
        assert_eq!(*calls.borrow(), ["search-cleared"]);

        press(&window, Key::Escape);
        assert_eq!(*calls.borrow(), ["search-cleared", "back"]);
    }

    #[test]
    fn escape_in_the_search_field_clears_it_too() {
        let (window, calls) = keyboard_window();
        let state = window.global::<State>();
        state.set_has_results(true);
        state.set_show_results(true);
        state.set_search_text("camellia".into());
        settle();
        ElementHandle::find_by_element_type_name(&window, "TextInput")
            .next()
            .unwrap()
            .mock_single_click(PointerEventButton::Left);
        press(&window, Key::Escape);
        assert_eq!(state.get_search_text(), "");
        assert_eq!(*calls.borrow(), ["search-cleared"]);
    }

    #[test]
    fn escape_does_nothing_on_the_main_page() {
        let (window, calls) = keyboard_window();
        settle();
        press(&window, Key::Escape);
        press(&window, Key::Return);
        assert!(calls.borrow().is_empty());
    }

    #[test]
    fn enter_presses_the_primary_button_of_the_sheet_on_top() {
        let (window, calls) = keyboard_window();
        type Open = fn(&MainWindow, bool);
        let sheets: [(Open, &str); 5] = [
            (
                |w, v| w.global::<State>().set_delete_confirm_open(v),
                "confirm-delete",
            ),
            (
                |w, v| w.global::<State>().set_confirm_open(v),
                "confirm-logout",
            ),
            (
                |w, v| w.global::<State>().set_cache_confirm_open(v),
                "confirm-cache",
            ),
            (|w, v| w.global::<State>().set_keys_open(v), "save-keys"),
            (|w, v| w.global::<State>().set_about_open(v), "close-about"),
        ];
        for (open, name) in sheets {
            calls.borrow_mut().clear();
            open(&window, true);
            settle();
            press(&window, Key::Return);
            assert_eq!(*calls.borrow(), [name]);
            open(&window, false);
            settle();
        }
    }

    #[test]
    fn enter_in_the_keys_sheet_waits_while_it_is_busy() {
        let (window, calls) = keyboard_window();
        let state = window.global::<State>();
        state.set_keys_open(true);
        state.set_keys_busy(true);
        settle();
        press(&window, Key::Return);
        press(&window, Key::Escape);
        assert!(calls.borrow().is_empty());
    }

    #[test]
    fn a_text_field_keeps_enter_and_still_lets_escape_through() {
        let (window, calls) = keyboard_window();
        let state = window.global::<State>();
        state.set_keys_open(true);
        settle();
        ElementHandle::find_by_element_type_name(&window, "TextField")
            .next()
            .unwrap()
            .mock_single_click(PointerEventButton::Left);
        press(&window, Key::Return);
        assert!(calls.borrow().is_empty(), "{:?}", calls.borrow());
        press(&window, Key::Escape);
        assert_eq!(*calls.borrow(), ["cancel-keys"]);
    }

    #[test]
    fn enter_never_uploads_but_escape_cancels_the_upload_sheet() {
        let (window, calls) = keyboard_window();
        let state = window.global::<State>();
        state.set_upload_confirm_open(true);
        settle();
        press(&window, Key::Return);
        assert!(calls.borrow().is_empty());
        press(&window, Key::Escape);
        assert_eq!(*calls.borrow(), ["cancel-upload"]);
    }

    #[test]
    fn escape_closes_the_sheet_before_the_ones_under_it() {
        let (window, calls) = keyboard_window();
        let state = window.global::<State>();
        state.set_keys_open(true);
        state.set_confirm_open(true);
        state.set_about_open(true);
        settle();
        press(&window, Key::Escape);
        assert_eq!(*calls.borrow(), ["close-about"]);
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
