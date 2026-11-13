use indicatif::{ProgressBar, ProgressStyle};

pub fn bar(label: &'static str) -> ProgressBar {
    let style = ProgressStyle::with_template("{msg} [{bar:30}] {pos}/{len}, about {eta} left")
        .expect("the progress template is valid")
        .progress_chars("=> ");
    ProgressBar::new(0).with_style(style).with_message(label)
}

pub fn track(bar: &ProgressBar) -> impl FnMut(usize, usize) + '_ {
    move |done, total| {
        bar.update(|state| {
            state.set_len(total as u64);
            state.set_pos(done as u64);
        });
    }
}
