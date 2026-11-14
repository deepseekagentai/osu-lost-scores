use std::sync::{Arc, Mutex, PoisonError};

use indicatif::{ProgressBar, ProgressStyle};
use lost_scores_app::pipeline::{Event, Step};

#[derive(Clone, Default)]
pub struct Notes(Arc<Mutex<Option<ProgressBar>>>);

impl Notes {
    pub fn say(&self, line: &str) {
        match self
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
        {
            Some(bar) => bar.suspend(|| eprintln!("{line}")),
            None => eprintln!("{line}"),
        }
    }

    fn show(&self, bar: Option<ProgressBar>) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = bar;
    }
}

pub struct Printer {
    bar: Option<(Step, ProgressBar)>,
    notes: Notes,
}

impl Printer {
    pub fn new() -> Self {
        Self {
            bar: None,
            notes: Notes::default(),
        }
    }

    pub fn notes(&self) -> Notes {
        self.notes.clone()
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
                if self.bar.is_none() {
                    let created = bar(label);
                    self.notes.show(Some(created.clone()));
                    self.bar = Some((step, created));
                }
                let Some((_, bar)) = &self.bar else {
                    return;
                };
                bar.update(|state| {
                    state.set_len(total as u64);
                    state.set_pos(done as u64);
                });
            }
        }
    }

    pub fn clear(&mut self) {
        if let Some((_, bar)) = self.bar.take() {
            self.notes.show(None);
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

#[cfg(test)]
mod tests {
    use lost_scores_app::pipeline::{Event, Step};

    use super::Printer;

    #[test]
    fn a_finished_run_leaves_no_bar_behind() {
        let mut printer = Printer::new();
        printer.handle(Event::Progress {
            step: Step::MissingMaps,
            done: 3,
            total: 10,
        });
        assert!(printer.bar.is_some());
        crate::finish_run(&mut printer, "timings: none", true);
        assert!(printer.bar.is_none());
    }
}
