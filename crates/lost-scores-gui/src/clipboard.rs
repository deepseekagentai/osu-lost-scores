use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::thread;

pub trait Pasteboard {
    fn put_image(&mut self, path: &Path) -> Result<(), String>;
    fn put_text(&mut self, text: &str) -> Result<(), String>;
}

pub enum Request {
    Image(PathBuf),
    Text(String),
}

type Done = Box<dyn FnOnce(Result<(), String>) + Send>;

struct Job {
    request: Request,
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
                    Some(board) => match &job.request {
                        Request::Image(path) => board.put_image(path),
                        Request::Text(text) => board.put_text(text),
                    },
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

    pub fn send(&self, request: Request, done: impl FnOnce(Result<(), String>) + Send + 'static) {
        let job = Job {
            request,
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
    fn put_image(&mut self, path: &Path) -> Result<(), String> {
        let picture = image::open(path)
            .map_err(|error| error.to_string())?
            .into_rgba8();
        let data = arboard::ImageData {
            width: picture.width() as usize,
            height: picture.height() as usize,
            bytes: picture.into_raw().into(),
        };
        self.0.set_image(data).map_err(|error| error.to_string())
    }

    fn put_text(&mut self, text: &str) -> Result<(), String> {
        self.0
            .set_text(text.to_owned())
            .map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};
    use std::thread::{self, ThreadId};

    use super::{Clipboard, Pasteboard, Request};

    #[derive(Default)]
    struct Log {
        puts: Vec<(String, ThreadId)>,
        opened: usize,
        dropped: usize,
    }

    struct Fake(Arc<Mutex<Log>>);

    impl Pasteboard for Fake {
        fn put_image(&mut self, path: &Path) -> Result<(), String> {
            let mut log = self.0.lock().unwrap();
            log.puts
                .push((format!("image {}", path.display()), thread::current().id()));
            if path.ends_with("broken.png") {
                return Err("cannot read the image".to_owned());
            }
            Ok(())
        }

        fn put_text(&mut self, text: &str) -> Result<(), String> {
            let mut log = self.0.lock().unwrap();
            log.puts
                .push((format!("text {text}"), thread::current().id()));
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

    fn ask(clipboard: &Clipboard, request: Request) -> Result<(), String> {
        let (answer, outcome) = mpsc::channel();
        clipboard.send(request, move |result| answer.send(result).unwrap());
        outcome.recv().unwrap()
    }

    #[test]
    fn the_image_path_goes_in_and_the_outcome_comes_back() {
        let log = Arc::new(Mutex::new(Log::default()));
        let clipboard = board(&log, true);
        assert_eq!(
            ask(&clipboard, Request::Image("/a/lost.png".into())),
            Ok(())
        );
        assert_eq!(
            ask(&clipboard, Request::Image("/a/broken.png".into())),
            Err("cannot read the image".to_owned())
        );
        assert_eq!(ask(&clipboard, Request::Text("link".into())), Ok(()));
        let log = log.lock().unwrap();
        let what: Vec<_> = log.puts.iter().map(|(what, _)| what.as_str()).collect();
        assert_eq!(
            what,
            ["image /a/lost.png", "image /a/broken.png", "text link"]
        );
    }

    #[test]
    fn one_thread_owns_the_clipboard_for_its_whole_life() {
        let log = Arc::new(Mutex::new(Log::default()));
        let clipboard = board(&log, true);
        for _ in 0..3 {
            ask(&clipboard, Request::Text("x".into())).unwrap();
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
    fn a_missing_clipboard_is_an_error_and_is_tried_again() {
        let log = Arc::new(Mutex::new(Log::default()));
        let clipboard = board(&log, false);
        for _ in 0..2 {
            assert_eq!(
                ask(&clipboard, Request::Text("x".into())),
                Err("the clipboard is not available".to_owned())
            );
        }
        assert_eq!(log.lock().unwrap().opened, 2);
    }
}
