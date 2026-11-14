use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

const EVERY: usize = 64;

pub(crate) struct Ticker<'a> {
    total: usize,
    done: AtomicUsize,
    last: Mutex<usize>,
    report: &'a (dyn Fn(usize, usize) + Sync),
}

impl<'a> Ticker<'a> {
    pub(crate) fn new(total: usize, report: &'a (dyn Fn(usize, usize) + Sync)) -> Self {
        if total == 0 {
            report(0, 0);
        }
        Self {
            total,
            done: AtomicUsize::new(0),
            last: Mutex::new(0),
            report,
        }
    }

    pub(crate) fn tick(&self) {
        let done = self.done.fetch_add(1, Ordering::Relaxed) + 1;
        if !done.is_multiple_of(EVERY) && done != self.total {
            return;
        }
        let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        if done > *last {
            *last = done;
            (self.report)(done, self.total);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::thread;

    use super::Ticker;

    fn reports(total: usize, ticks: impl FnOnce(&Ticker<'_>)) -> Vec<(usize, usize)> {
        let seen = Mutex::new(Vec::new());
        let report = |done, total| seen.lock().unwrap().push((done, total));
        let ticker = Ticker::new(total, &report);
        ticks(&ticker);
        seen.into_inner().unwrap()
    }

    #[test]
    fn it_reports_every_64th_item_and_the_last_one() {
        let seen = reports(200, |ticker| (0..200).for_each(|_| ticker.tick()));
        assert_eq!(seen, [(64, 200), (128, 200), (192, 200), (200, 200)]);
    }

    #[test]
    fn a_total_that_is_a_multiple_of_64_is_reported_once() {
        let seen = reports(128, |ticker| (0..128).for_each(|_| ticker.tick()));
        assert_eq!(seen, [(64, 128), (128, 128)]);
    }

    #[test]
    fn an_empty_job_reports_zero_of_zero_at_once() {
        assert_eq!(reports(0, |_| {}), [(0, 0)]);
    }

    #[test]
    fn threads_never_make_the_count_go_backwards() {
        let seen = reports(4000, |ticker| {
            thread::scope(|scope| {
                for _ in 0..8 {
                    scope.spawn(|| (0..500).for_each(|_| ticker.tick()));
                }
            });
        });
        assert!(
            seen.windows(2).all(|pair| pair[0].0 < pair[1].0),
            "{seen:?}"
        );
        assert_eq!(seen.last(), Some(&(4000, 4000)));
        assert!(seen.iter().all(|&(_, total)| total == 4000));
    }
}
