use std::time::{Duration, Instant};

use slint::ComponentHandle;

fn pss_kb() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/self/smaps_rollup").ok()?;
    let line = text.lines().find(|line| line.starts_with("Pss:"))?;
    line.split_whitespace().nth(1)?.parse().ok()
}

fn main() -> anyhow::Result<()> {
    let started = Instant::now();
    lost_scores_gui::app::run_with(|window| {
        let mut reported = false;
        let _ = window.window().set_rendering_notifier(move |state, _| {
            if !reported && matches!(state, slint::RenderingState::AfterRendering) {
                reported = true;
                eprintln!("first frame after {} ms", started.elapsed().as_millis());
            }
        });
        slint::Timer::single_shot(Duration::from_secs(6), || {
            eprintln!("pss {} kB", pss_kb().unwrap_or_default());
            let _ = slint::quit_event_loop();
        });
    })
}
