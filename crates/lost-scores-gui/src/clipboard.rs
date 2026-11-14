use std::panic::{self, AssertUnwindSafe};
use std::sync::mpsc::{self, Sender};
use std::thread;

use crate::share::render::panic_message;

pub trait Pasteboard {
    fn put_text(&mut self, text: &str) -> Result<(), String>;
}

type Done = Box<dyn FnOnce(Result<(), String>) + Send>;

struct Job {
    text: String,
    done: Done,
}

pub struct Clipboard {
    jobs: Sender<Job>,
}

impl Clipboard {
    pub fn spawn<P: Pasteboard>(open: impl FnMut() -> Option<P> + Send + 'static) -> Self {
        let (jobs, inbox) = mpsc::channel::<Job>();
        let mut open = open;
        thread::spawn(move || {
            let mut board: Option<P> = None;
            for job in inbox {
                if board.is_none() {
                    board = open();
                }
                let result = match board.as_mut() {
                    Some(held) => {
                        let attempt =
                            panic::catch_unwind(AssertUnwindSafe(|| held.put_text(&job.text)));
                        attempt.unwrap_or_else(|payload| {
                            board = None;
                            Err(format!(
                                "the clipboard stopped: {}",
                                panic_message(payload.as_ref())
                            ))
                        })
                    }
                    None => Err("the clipboard is not available".to_owned()),
                };
                (job.done)(result);
            }
        });
        Self { jobs }
    }

    pub fn system() -> Self {
        Self::spawn(ArboardBoard::open)
    }

    pub fn send(&self, text: String, done: impl FnOnce(Result<(), String>) + Send + 'static) {
        let job = Job {
            text,
            done: Box::new(done),
        };
        if let Err(rejected) = self.jobs.send(job) {
            (rejected.0.done)(Err("the clipboard is not available".to_owned()));
        }
    }
}

pub struct ArboardBoard(arboard::Clipboard);

impl ArboardBoard {
    fn open() -> Option<Self> {
        arboard::Clipboard::new().ok().map(Self)
    }
}

impl Pasteboard for ArboardBoard {
    fn put_text(&mut self, text: &str) -> Result<(), String> {
        self.0
            .set_text(text.to_owned())
            .map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};
    use std::thread::{self, ThreadId};

    use super::{Clipboard, Pasteboard};

    #[derive(Default)]
    struct Log {
        puts: Vec<(String, ThreadId)>,
        opened: usize,
        dropped: usize,
    }

    struct Fake(Arc<Mutex<Log>>);

    impl Pasteboard for Fake {
        fn put_text(&mut self, text: &str) -> Result<(), String> {
            let mut log = self.0.lock().unwrap();
            log.puts.push((text.to_owned(), thread::current().id()));
            drop(log);
            if text == "explodes" {
                panic!("the clipboard blew up");
            }
            if text == "broken" {
                return Err("cannot write the text".to_owned());
            }
            Ok(())
        }
    }

    impl Drop for Fake {
        fn drop(&mut self) {
            self.0.lock().unwrap().dropped += 1;
        }
    }

    fn board(log: &Arc<Mutex<Log>>, available: bool) -> Clipboard {
        let log = log.clone();
        Clipboard::spawn(move || {
            log.lock().unwrap().opened += 1;
            available.then(|| Fake(log.clone()))
        })
    }

    fn ask(clipboard: &Clipboard, text: &str) -> Result<(), String> {
        let (answer, outcome) = mpsc::channel();
        clipboard.send(text.to_owned(), move |result| answer.send(result).unwrap());
        outcome.recv().unwrap()
    }

    #[test]
    fn the_text_goes_in_and_the_outcome_comes_back() {
        let log = Arc::new(Mutex::new(Log::default()));
        let clipboard = board(&log, true);
        assert_eq!(ask(&clipboard, "link"), Ok(()));
        assert_eq!(
            ask(&clipboard, "broken"),
            Err("cannot write the text".to_owned())
        );
        let log = log.lock().unwrap();
        let what: Vec<_> = log.puts.iter().map(|(what, _)| what.as_str()).collect();
        assert_eq!(what, ["link", "broken"]);
    }

    #[test]
    fn one_thread_owns_the_clipboard_for_its_whole_life() {
        let log = Arc::new(Mutex::new(Log::default()));
        let clipboard = board(&log, true);
        for _ in 0..3 {
            ask(&clipboard, "x").unwrap();
        }
        {
            let log = log.lock().unwrap();
            let ids: Vec<_> = log.puts.iter().map(|(_, id)| *id).collect();
            assert!(ids.iter().all(|id| *id == ids[0]));
            assert_ne!(ids[0], thread::current().id());
            assert_eq!(log.opened, 1);
            assert_eq!(log.dropped, 0);
        }
        drop(clipboard);
        for _ in 0..100 {
            if log.lock().unwrap().dropped == 1 {
                return;
            }
            thread::sleep(std::time::Duration::from_millis(20));
        }
        panic!("the clipboard was never released");
    }

    #[test]
    fn a_panic_in_a_job_is_that_jobs_error_and_the_clipboard_is_reopened() {
        let log = Arc::new(Mutex::new(Log::default()));
        let clipboard = board(&log, true);
        assert_eq!(ask(&clipboard, "first"), Ok(()));
        let failed = ask(&clipboard, "explodes");
        assert_eq!(
            failed,
            Err("the clipboard stopped: the clipboard blew up".to_owned())
        );
        assert_eq!(ask(&clipboard, "next"), Ok(()));
        let log = log.lock().unwrap();
        assert_eq!(log.opened, 2);
        assert_eq!(log.puts.last().unwrap().0, "next");
    }

    #[test]
    fn a_missing_clipboard_is_an_error_and_is_tried_again() {
        let log = Arc::new(Mutex::new(Log::default()));
        let clipboard = board(&log, false);
        for _ in 0..2 {
            assert_eq!(
                ask(&clipboard, "x"),
                Err("the clipboard is not available".to_owned())
            );
        }
        assert_eq!(log.lock().unwrap().opened, 2);
    }
}
