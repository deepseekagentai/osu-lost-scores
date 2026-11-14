use std::time::{Duration, Instant};

pub struct Timings {
    started: Instant,
    steps: Vec<(&'static str, Duration)>,
}

impl Default for Timings {
    fn default() -> Self {
        Self::new()
    }
}

impl Timings {
    pub fn new() -> Self {
        Self {
            started: Instant::now(),
            steps: Vec::new(),
        }
    }

    pub fn add(&mut self, step: &'static str, took: Duration) {
        self.steps.push((step, took));
    }

    pub fn count(&self, step: &str) -> usize {
        self.steps.iter().filter(|(name, _)| *name == step).count()
    }

    pub fn time<T>(&mut self, step: &'static str, run: impl FnOnce() -> T) -> T {
        let started = Instant::now();
        let result = run();
        self.add(step, started.elapsed());
        result
    }

    pub fn report(&self) -> String {
        render(&self.steps, self.started.elapsed())
    }
}

const TOTAL: &str = "total";

fn render(steps: &[(&'static str, Duration)], total: Duration) -> String {
    let width = steps
        .iter()
        .map(|(step, _)| step.len())
        .chain([TOTAL.len()])
        .max()
        .unwrap_or(0);
    let mut lines = vec!["timings:".to_owned()];
    for (step, took) in steps.iter().copied().chain([(TOTAL, total)]) {
        lines.push(format!("  {step:<width$}  {:>6.2} s", took.as_secs_f64()));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_are_aligned_and_end_with_the_total() {
        let steps = [
            ("songs index", Duration::from_millis(2310)),
            ("pp", Duration::from_millis(12840)),
        ];
        assert_eq!(
            render(&steps, Duration::from_millis(15204)),
            "timings:\n  songs index    2.31 s\n  pp            12.84 s\n  total         15.20 s"
        );
    }
}
