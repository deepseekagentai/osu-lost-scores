use std::sync::{Arc, Mutex, PoisonError};

use indicatif::{ProgressBar, ProgressStyle};
use lost_scores_app::pipeline::{Event, Step};

#[derive(Default)]
struct Shown {
    bar: Option<(&'static str, ProgressBar)>,
    last_wait: Option<u64>,
}

#[derive(Clone, Default)]
pub struct Notes(Arc<Mutex<Shown>>);

impl Notes {
    pub fn waiting(&self, seconds: u64, maps: Option<usize>) {
        let mut shown = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let fresh = shown.last_wait.is_none_or(|last| seconds > last);
        shown.last_wait = Some(seconds);
        let left = match maps {
            Some(1) => "1 map left, next check in ".to_owned(),
            Some(count) => format!("{count} maps left, next check in "),
            None => String::new(),
        };
        match &shown.bar {
            Some((label, bar)) => {
                bar.set_message(format!("{label}, shared queue: {left}{seconds} s"))
            }
            None if fresh => {
                eprintln!(
                    "waiting in the shared queue of the Lost Scores server: {left}{seconds} s"
                );
            }
            None => {}
        }
    }

    fn show(&self, bar: Option<(&'static str, ProgressBar)>) {
        let mut shown = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        shown.bar = bar;
        shown.last_wait = None;
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
                    self.notes.show(Some((label, created.clone())));
                    self.bar = Some((step, created));
                }
                let Some((_, bar)) = &self.bar else {
                    return;
                };
                bar.set_message(label);
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

    fn message(printer: &Printer) -> String {
        printer.bar.as_ref().unwrap().1.message()
    }

    #[test]
    fn a_queue_wait_shows_on_the_bar_until_the_next_request() {
        let mut printer = Printer::new();
        let notes = printer.notes();
        printer.handle(Event::Progress {
            step: Step::Catalog,
            done: 0,
            total: 5,
        });
        notes.waiting(12, None);
        assert_eq!(message(&printer), "map statuses, shared queue: 12 s");
        notes.waiting(11, Some(40));
        assert_eq!(
            message(&printer),
            "map statuses, shared queue: 40 maps left, next check in 11 s"
        );
        printer.handle(Event::Progress {
            step: Step::Catalog,
            done: 1,
            total: 5,
        });
        assert_eq!(message(&printer), "map statuses");
    }

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
