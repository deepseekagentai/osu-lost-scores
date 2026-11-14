use indicatif::{ProgressBar, ProgressStyle};
use lost_scores_app::pipeline::{Event, Step};

pub struct Printer {
    bar: Option<(Step, ProgressBar)>,
}

impl Printer {
    pub fn new() -> Self {
        Self { bar: None }
    }

    pub fn handle(&mut self, event: Event) {
        match event {
            Event::Log(line) => {
                self.clear();
                eprintln!("{line}");
            }
            Event::Progress { step, done, total } => {
                let Some(label) = label(step) else {
                    return;
                };
                if self
                    .bar
                    .as_ref()
                    .is_some_and(|(current, _)| *current != step)
                {
                    self.clear();
                }
                let (_, bar) = self.bar.get_or_insert_with(|| (step, bar(label)));
                bar.update(|state| {
                    state.set_len(total as u64);
                    state.set_pos(done as u64);
                });
            }
        }
    }

    fn clear(&mut self) {
        if let Some((_, bar)) = self.bar.take() {
            bar.finish_and_clear();
        }
    }
}

impl Drop for Printer {
    fn drop(&mut self) {
        self.clear();
    }
}

fn label(step: Step) -> Option<&'static str> {
    match step {
        Step::MissingMaps => Some("missing maps"),
        Step::Catalog => Some("map statuses"),
        Step::Refresh => Some("status refresh"),
        _ => None,
    }
}

fn bar(label: &'static str) -> ProgressBar {
    let style = ProgressStyle::with_template("{msg} [{bar:30}] {pos}/{len}, about {eta} left")
        .expect("the progress template is valid")
        .progress_chars("=> ");
    ProgressBar::new(0).with_style(style).with_message(label)
}
