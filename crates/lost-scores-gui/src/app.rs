use std::path::Path;
use std::thread;
use std::time::Instant;

use anyhow::Result;
use lost_scores_app::setup;
use slint::ComponentHandle;

use crate::{APP_ID, MainWindow, State, choose_folder, show_path};

const START_MESSAGE: &str =
    "Could not find your osu! folder: pick it with the button next to the path";

pub fn run() -> Result<()> {
    let started = Instant::now();
    let window = MainWindow::new()?;
    slint::set_xdg_app_id(APP_ID)?;
    trace_first_frame(&window, started);
    let state = window.global::<State>();
    match setup::resolve_osu_dir(None) {
        Ok(dir) => state.set_osu_dir(show_path(&dir).into()),
        Err(_) => state.set_status(START_MESSAGE.into()),
    }
    let weak = window.as_weak();
    state.on_pick_folder(move || {
        let weak = weak.clone();
        thread::spawn(move || {
            let Some(dir) = rfd::FileDialog::new()
                .set_title("Select osu! Game Directory")
                .pick_folder()
            else {
                return;
            };
            let result = choose_folder(Path::new(&dir)).map_err(|error| error.to_string());
            let _ = weak.upgrade_in_event_loop(move |window| {
                let state = window.global::<State>();
                match result {
                    Ok(path) => {
                        state.set_osu_dir(path.into());
                        state.set_status("".into());
                    }
                    Err(message) => state.set_status(message.into()),
                }
            });
        });
    });
    window.run()?;
    Ok(())
}

fn trace_first_frame(window: &MainWindow, started: Instant) {
    if std::env::var_os("LOST_SCORES_GUI_TRACE").is_none() {
        return;
    }
    let mut reported = false;
    let _ = window.window().set_rendering_notifier(move |state, _| {
        if !reported && matches!(state, slint::RenderingState::AfterRendering) {
            reported = true;
            eprintln!("first frame after {} ms", started.elapsed().as_millis());
        }
    });
}
