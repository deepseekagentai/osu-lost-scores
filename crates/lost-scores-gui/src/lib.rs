pub mod account;
pub mod app;
pub mod avatar;
pub mod clipboard;
pub mod layout;
pub mod log;
pub mod newest;
pub mod previews;
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

    use super::{FootItem, MainWindow, Preview, State, choose_folder};

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
    fn the_crosses_stay_left_of_the_scan_lists_scrollbar() {
        i_slint_backend_testing::init_no_event_loop();
        let window = MainWindow::new().unwrap();
        window.show().unwrap();
        window.window().set_size(LogicalSize::new(650.0, 800.0));
        let labels: Vec<SharedString> = (0..14)
            .map(|n| SharedString::from(format!("{} Nov 2026 · 09:00", 14 - n)))
            .collect();
        let state = window.global::<State>();
        state.set_scans(ModelRc::new(VecModel::from(labels)));
        state.set_has_results(true);
        state.set_show_results(true);
        state.set_scans_open(true);
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(250));

        let bars: Vec<_> = ElementHandle::find_by_element_type_name(&window, "ScrollBar")
            .map(|bar| bar.absolute_position().x)
            .collect();
        assert!(!bars.is_empty(), "no scrollbar found");
        let popup_bar = bars.iter().copied().fold(f32::MIN, f32::max);
        let crosses: Vec<_> =
            ElementHandle::find_by_accessible_label(&window, "Move to trash").collect();
        assert!(!crosses.is_empty());
        for cross in crosses {
            let right = cross.absolute_position().x + cross.size().width;
            assert!(
                right <= popup_bar,
                "cross right {right}, scrollbar {popup_bar}"
            );
        }
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

    fn images_window(width: f32, labels: &[&str]) -> MainWindow {
        i_slint_backend_testing::init_no_event_loop();
        make_images_window(width, labels)
    }

    fn make_images_window(width: f32, labels: &[&str]) -> MainWindow {
        let window = MainWindow::new().unwrap();
        window.show().unwrap();
        window.window().set_size(LogicalSize::new(width, 800.0));
        let state = window.global::<State>();
        state.set_has_results(true);
        state.set_show_results(true);
        state.set_has_images(!labels.is_empty());
        let previews: Vec<Preview> = labels
            .iter()
            .map(|label| Preview {
                label: (*label).into(),
                image: slint::Image::default(),
                width: 0.0,
                height: 0.0,
                ready: false,
            })
            .collect();
        state.set_previews(ModelRc::new(VecModel::from(previews)));
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(250));
        window
    }

    fn count(window: &MainWindow, type_name: &str) -> usize {
        ElementHandle::find_by_element_type_name(window, type_name).count()
    }

    #[test]
    fn table_mode_has_the_three_tabs_and_images_mode_has_none() {
        let window = images_window(650.0, &["Lost Scores"]);
        let mut tabs: Vec<_> = ElementHandle::find_by_element_type_name(&window, "Tab")
            .map(|tab| (tab.absolute_position().x, tab.accessible_label().unwrap()))
            .collect();
        tabs.sort_by(|a, b| a.0.total_cmp(&b.0));
        let labels: Vec<_> = tabs.iter().map(|(_, label)| label.as_str()).collect();
        assert_eq!(labels, ["Lost Scores", "Online Top", "Potential Top"]);
        window.global::<State>().set_images_shown(true);
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(250));
        assert_eq!(count(&window, "Tab"), 0);
    }

    #[test]
    fn the_tab_buttons_ask_for_the_tables() {
        let window = images_window(650.0, &["Lost Scores"]);
        let calls = Rc::new(std::cell::RefCell::new(Vec::new()));
        let tables = calls.clone();
        window
            .global::<State>()
            .on_set_tab(move |index| tables.borrow_mut().push(index));
        for label in ["Lost Scores", "Online Top", "Potential Top"] {
            ElementHandle::find_by_element_type_name(&window, "Tab")
                .find(|tab| tab.accessible_label().as_deref() == Some(label))
                .unwrap()
                .mock_single_click(PointerEventButton::Left);
        }
        assert_eq!(*calls.borrow(), [0, 1, 2]);
    }

    fn switch_places(width: f32) {
        let window = make_images_window(width, &["Lost Scores"]);
        let state = window.global::<State>();
        state.set_tab(1);
        let calls = Rc::new(std::cell::RefCell::new(Vec::new()));
        let (images, tables) = (calls.clone(), calls.clone());
        state.on_show_images(move || images.borrow_mut().push(-1));
        state.on_set_tab(move |index| tables.borrow_mut().push(index));
        let label_y = |label: &str| {
            ElementHandle::find_by_accessible_label(&window, label)
                .next()
                .unwrap()
                .absolute_position()
        };
        let table = label_y("Table");
        let images = label_y("Images");
        assert!(images.x < table.x, "{width}");
        assert_eq!(table.y, images.y);
        let selector = ElementHandle::find_by_element_type_name(&window, "ScanSelector")
            .next()
            .unwrap();
        if width >= 1480.0 {
            assert!(
                table.y >= selector.absolute_position().y
                    && table.y < selector.absolute_position().y + selector.size().height,
                "the switch is not in the first row: {} vs {}",
                table.y,
                selector.absolute_position().y
            );
        }
        let panel = ElementHandle::find_by_element_type_name(&window, "ResultsPanel")
            .next()
            .unwrap();
        let edge = panel.absolute_position().x + panel.size().width;
        assert!(
            images.x < edge && images.x > edge - 220.0,
            "{width}: {}",
            images.x
        );
    }

    #[test]
    fn switching_modes_fades_the_content_and_slides_the_thumb() {
        let window = images_window(650.0, &["Lost Scores"]);
        let state = window.global::<State>();
        state.set_images_shown(false);
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(400));
        let tabs = || {
            ElementHandle::find_by_element_type_name(&window, "Tab")
                .filter(|tab| tab.accessible_label().as_deref() == Some("Lost Scores"))
                .count()
        };
        let thumb_x = || {
            ElementHandle::find_by_element_type_name(&window, "ModeThumb")
                .next()
                .unwrap()
                .absolute_position()
                .x
        };
        assert_eq!(tabs(), 1);
        let table_x = thumb_x();
        state.set_images_shown(true);
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(60));
        assert_eq!(tabs(), 1, "the table stays while it fades out");
        let middle = thumb_x();
        assert!(
            middle < table_x,
            "the thumb is moving: {middle} vs {table_x}"
        );
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(400));
        assert_eq!(tabs(), 0, "the pictures replaced the table");
        let images_x = thumb_x();
        assert!(images_x < middle, "{images_x} vs {middle}");
    }

    #[test]
    fn a_click_anywhere_on_the_switch_flips_the_mode() {
        let window = images_window(650.0, &["Lost Scores"]);
        let state = window.global::<State>();
        state.set_tab(1);
        let calls = Rc::new(std::cell::RefCell::new(Vec::new()));
        let (images, tables) = (calls.clone(), calls.clone());
        state.on_show_images(move || images.borrow_mut().push(-1));
        state.on_set_tab(move |index| tables.borrow_mut().push(index));
        let click = |label: &str| {
            ElementHandle::find_by_accessible_label(&window, label)
                .next()
                .unwrap()
                .mock_single_click(PointerEventButton::Left);
        };
        state.set_images_shown(false);
        click("Table");
        click("Images");
        state.set_images_shown(true);
        click("Images");
        click("Table");
        assert_eq!(*calls.borrow(), [-1, -1, 1, 1]);
    }

    fn arrows(window: &MainWindow) -> usize {
        ["Previous picture", "Next picture"]
            .iter()
            .map(|label| ElementHandle::find_by_accessible_label(window, label).count())
            .sum()
    }

    #[test]
    fn the_arrows_show_only_in_images_mode_with_pictures() {
        let window = images_window(650.0, &["Badge", "Lost Scores"]);
        let state = window.global::<State>();
        assert_eq!(arrows(&window), 0);
        state.set_images_shown(true);
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(250));
        assert_eq!(arrows(&window), 2);
        state.set_has_images(false);
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(250));
        assert_eq!(arrows(&window), 0);
    }

    #[test]
    fn the_arrows_sit_in_the_bottom_right_corner_and_follow_their_enabled_state() {
        let window = images_window(650.0, &["Badge", "Lost Scores"]);
        let state = window.global::<State>();
        state.set_images_shown(true);
        state.set_next_picture_ok(true);
        let steps = Rc::new(std::cell::RefCell::new(Vec::new()));
        let log = steps.clone();
        state.on_step_picture(move |direction| log.borrow_mut().push(direction));
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(250));
        let find = |label: &str| {
            ElementHandle::find_by_accessible_label(&window, label)
                .next()
                .unwrap()
        };
        let (previous, next) = (find("Previous picture"), find("Next picture"));
        assert!(previous.absolute_position().x < next.absolute_position().x);
        assert!(next.absolute_position().x + next.size().width > 560.0);
        assert!(next.absolute_position().y > 600.0);
        previous.mock_single_click(PointerEventButton::Left);
        next.mock_single_click(PointerEventButton::Left);
        assert_eq!(*steps.borrow(), [1]);
    }

    #[test]
    fn the_switch_sits_at_the_right_of_the_header_in_one_column() {
        i_slint_backend_testing::init_no_event_loop();
        switch_places(650.0);
    }

    #[test]
    fn the_switch_sits_at_the_right_of_the_header_in_two_columns() {
        i_slint_backend_testing::init_no_event_loop();
        switch_places(1600.0);
    }

    #[test]
    fn images_mode_hides_the_search_the_header_the_tabs_and_the_foot_in_one_column() {
        i_slint_backend_testing::init_no_event_loop();
        search_and_header_follow_the_tab(650.0);
    }

    #[test]
    fn images_mode_hides_the_search_the_header_the_tabs_and_the_foot_in_two_columns() {
        i_slint_backend_testing::init_no_event_loop();
        search_and_header_follow_the_tab(1600.0);
    }

    fn search_and_header_follow_the_tab(width: f32) {
        {
            let window = make_images_window(width, &["Lost Scores"]);
            let state = window.global::<State>();
            state.set_foot(ModelRc::new(VecModel::from(vec![FootItem {
                label: "Lost plays".into(),
                value: "9".into(),
                tail: "".into(),
                tone: 0,
            }])));
            i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(250));
            assert_eq!(count(&window, "FootPair"), 1, "{width} tables");
            assert_eq!(count(&window, "Tab"), 3, "{width} tables");
            assert_eq!(count(&window, "SearchField"), 1, "{width} tables");
            assert_eq!(count(&window, "SearchButtons"), 1, "{width} tables");
            assert!(count(&window, "HeaderCell") > 0, "{width} tables");
            state.set_images_shown(true);
            i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(250));
            assert_eq!(count(&window, "SearchField"), 0, "{width} images");
            assert_eq!(count(&window, "SearchButtons"), 0, "{width} images");
            assert_eq!(count(&window, "HeaderCell"), 0, "{width} images");
            assert_eq!(count(&window, "FootPair"), 0, "{width} images");
            assert_eq!(count(&window, "Tab"), 0, "{width} images");
        }
    }

    #[test]
    fn one_catbox_button_sits_next_to_the_switch_and_none_on_the_pictures() {
        let window = images_window(650.0, &["Badge", "Lost Scores", "Potential Top"]);
        let state = window.global::<State>();
        state.set_images_shown(true);
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(400));
        let labelled =
            |label: &str| ElementHandle::find_by_accessible_label(&window, label).count();
        assert_eq!(labelled("catbox"), 1);
        assert_eq!(labelled("Open"), 0);
        let catbox = ElementHandle::find_by_accessible_label(&window, "catbox")
            .next()
            .unwrap();
        let switch = ElementHandle::find_by_element_type_name(&window, "ModeSwitch")
            .next()
            .unwrap();
        let middle =
            |element: &ElementHandle| element.absolute_position().y + element.size().height / 2.0;
        assert!((middle(&catbox) - middle(&switch)).abs() < 2.0);
        assert!(catbox.absolute_position().x + catbox.size().width <= switch.absolute_position().x);
        state.set_images_shown(false);
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(400));
        assert_eq!(labelled("catbox"), 0);
    }

    #[test]
    fn the_catbox_menu_lists_the_pictures_and_uploads_the_chosen_one() {
        let window = images_window(650.0, &["Badge", "Lost Scores", "Potential Top"]);
        let state = window.global::<State>();
        state.set_images_shown(true);
        let uploads = Rc::new(std::cell::RefCell::new(Vec::new()));
        let seen = uploads.clone();
        state.on_upload_image(move |index| seen.borrow_mut().push(index));
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(400));
        let labelled =
            |label: &str| ElementHandle::find_by_accessible_label(&window, label).count();
        let click = |label: &str| {
            ElementHandle::find_by_accessible_label(&window, label)
                .next()
                .unwrap()
                .mock_single_click(PointerEventButton::Left);
        };
        assert_eq!(labelled("Potential Top"), 0);
        click("catbox");
        for label in ["Badge", "Lost Scores", "Potential Top"] {
            assert_eq!(labelled(label), 1, "{label}");
        }
        click("Potential Top");
        assert_eq!(labelled("Badge"), 0);
        assert_eq!(*uploads.borrow(), [2]);
    }

    #[test]
    fn escape_closes_the_catbox_menu_before_anything_else() {
        let window = images_window(650.0, &["Badge"]);
        let state = window.global::<State>();
        state.set_images_shown(true);
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(400));
        ElementHandle::find_by_accessible_label(&window, "catbox")
            .next()
            .unwrap()
            .mock_single_click(PointerEventButton::Left);
        assert_eq!(
            ElementHandle::find_by_accessible_label(&window, "Badge").count(),
            1
        );
        press(&window, Key::Escape);
        assert_eq!(
            ElementHandle::find_by_accessible_label(&window, "Badge").count(),
            0
        );
        assert!(state.get_show_results());
    }

    fn picture(window: &MainWindow, label: &str) -> ElementHandle {
        ElementHandle::find_by_accessible_label(window, &format!("{label} picture"))
            .next()
            .unwrap()
    }

    #[test]
    fn a_click_on_a_picture_opens_it() {
        let window = images_window(650.0, &["Badge", "Lost Scores"]);
        let state = window.global::<State>();
        state.set_images_shown(true);
        let opened = Rc::new(std::cell::RefCell::new(Vec::new()));
        let seen = opened.clone();
        state.on_open_image(move |index| seen.borrow_mut().push(index));
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(250));
        picture(&window, "Lost Scores").mock_single_click(PointerEventButton::Left);
        assert_eq!(*opened.borrow(), [1]);
    }

    #[test]
    fn a_right_click_on_a_picture_offers_copy_and_open() {
        let window = images_window(650.0, &["Badge", "Lost Scores"]);
        let state = window.global::<State>();
        state.set_images_shown(true);
        let calls = Rc::new(std::cell::RefCell::new(Vec::new()));
        let (copy, open) = (calls.clone(), calls.clone());
        state.on_copy_image(move |index| copy.borrow_mut().push(("copy", index)));
        state.on_open_image(move |index| open.borrow_mut().push(("open", index)));
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(250));
        let labelled =
            |label: &str| ElementHandle::find_by_accessible_label(&window, label).count();
        assert_eq!(labelled("Copy image"), 0);

        picture(&window, "Badge").mock_single_click(PointerEventButton::Right);
        assert_eq!(labelled("Copy image"), 1);
        assert_eq!(labelled("Open"), 1);
        ElementHandle::find_by_accessible_label(&window, "Copy image")
            .next()
            .unwrap()
            .mock_single_click(PointerEventButton::Left);
        assert_eq!(labelled("Copy image"), 0);

        picture(&window, "Lost Scores").mock_single_click(PointerEventButton::Right);
        ElementHandle::find_by_accessible_label(&window, "Open")
            .next()
            .unwrap()
            .mock_single_click(PointerEventButton::Left);
        assert_eq!(*calls.borrow(), [("copy", 0), ("open", 1)]);
    }

    #[test]
    fn escape_closes_the_picture_menu_before_anything_else() {
        let window = images_window(650.0, &["Badge"]);
        let state = window.global::<State>();
        state.set_images_shown(true);
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(250));
        picture(&window, "Badge").mock_single_click(PointerEventButton::Right);
        assert_eq!(
            ElementHandle::find_by_accessible_label(&window, "Copy image").count(),
            1
        );
        press(&window, Key::Escape);
        assert_eq!(
            ElementHandle::find_by_accessible_label(&window, "Copy image").count(),
            0
        );
        assert!(state.get_show_results());
    }

    #[test]
    fn images_mode_has_no_open_folder_button_and_no_picture_titles() {
        let window = images_window(650.0, &["Lost Scores", "Badge"]);
        window.global::<State>().set_images_shown(true);
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(250));
        let labelled =
            |label: &str| ElementHandle::find_by_accessible_label(&window, label).count();
        assert_eq!(labelled("Open folder"), 0);
        assert_eq!(labelled("Open the scan's folder"), 1);
        assert_eq!(labelled("Lost Scores"), 0);
        assert_eq!(labelled("Badge"), 0);
        assert_eq!(labelled("Open"), 0);
        assert_eq!(labelled("catbox"), 1);
    }

    #[test]
    fn a_scan_without_images_shows_one_muted_line() {
        let window = images_window(650.0, &[]);
        window.global::<State>().set_images_shown(true);
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(250));
        let line = "This scan has no images. Images are made after every scan from now on.";
        assert_eq!(
            ElementHandle::find_by_accessible_label(&window, line).count(),
            1
        );
        assert_eq!(
            ElementHandle::find_by_accessible_label(&window, "Open").count(),
            0
        );
        assert_eq!(
            ElementHandle::find_by_accessible_label(&window, "catbox").count(),
            0
        );
    }

    #[test]
    fn every_picture_is_centred() {
        let window = images_window(650.0, &[]);
        let state = window.global::<State>();
        let make = |label: &str, width: f32| Preview {
            label: label.into(),
            image: slint::Image::from_rgba8(slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(
                width as u32,
                60,
            )),
            width,
            height: 60.0,
            ready: true,
        };
        state.set_has_images(true);
        state.set_previews(ModelRc::new(VecModel::from(vec![
            make("Badge", 300.0),
            make("Lost Scores", 500.0),
        ])));
        state.set_images_shown(true);
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(250));

        let image = |width: f32| {
            ElementHandle::find_by_element_type_name(&window, "Image")
                .find(|element| element.size().width == width && element.size().height == 60.0)
                .unwrap()
        };
        let card = ElementHandle::find_by_element_type_name(&window, "PreviewCard")
            .next()
            .unwrap();
        let badge = image(300.0);
        let wide = image(500.0);
        let centre = card.absolute_position().x + card.size().width / 2.0;
        assert!((badge.absolute_position().x + 150.0 - centre).abs() < 1.0);
        assert!((wide.absolute_position().x + 250.0 - centre).abs() < 1.0);
    }

    #[test]
    fn the_header_has_no_share_button_in_one_column() {
        i_slint_backend_testing::init_no_event_loop();
        no_share_button(650.0);
    }

    #[test]
    fn the_header_has_no_share_button_in_two_columns() {
        i_slint_backend_testing::init_no_event_loop();
        no_share_button(1600.0);
    }

    fn no_share_button(width: f32) {
        {
            let window = make_images_window(width, &["Lost Scores"]);
            let share = "Share the images of this scan";
            assert_eq!(
                ElementHandle::find_by_accessible_label(&window, share).count(),
                0
            );
            assert_eq!(count(&window, "ShareButton"), 0);
        }
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
        settle();

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
    fn escape_in_the_scores_field_cancels_the_edit_and_stays_there() {
        let (window, calls) = keyboard_window();
        let state = window.global::<State>();
        state.set_connected(true);
        state.set_search_text("camellia".into());
        state.set_scores_to_show(10);
        settle();
        let boxed = || {
            ElementHandle::find_by_element_type_name(&window, "ScoresBox")
                .next()
                .unwrap()
        };
        boxed().mock_single_click(PointerEventButton::Left);
        let text = SharedString::from("3");
        window
            .window()
            .dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
        window
            .window()
            .dispatch_event(WindowEvent::KeyReleased { text });
        assert_eq!(state.get_scores_pending(), "3");
        press(&window, Key::Escape);
        assert_eq!(state.get_scores_pending(), "");
        assert_eq!(state.get_search_text(), "camellia");
        assert!(calls.borrow().is_empty(), "{:?}", calls.borrow());
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
