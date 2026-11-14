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
