use std::cell::Cell;
use std::collections::HashMap;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use lost_scores_app::cache;
use lost_scores_app::history::{self, SavedScan};
use lost_scores_app::online::Player;
use lost_scores_app::pipeline::{
    self, Api, Event, PlayerLookup, Source, Step, find, load_catalog, lost, read, refresh_statuses,
};
use lost_scores_app::shared::Left;
use lost_scores_app::timings::Timings;
use lost_scores_core::Scan;
use lost_scores_core::maps::MapCatalog;
use osu_api::ApiError;

use crate::account::lookup_failure;
use crate::share::rasters::{self, CoverKey, Fetch, HttpFetch, LocalCovers};
use crate::share::render::{FAILURE_PREFIX, panic_message};
use crate::share::{ShareOptions, cover_keys};

const API_HINT: &str = "add your own API keys";
const PROGRESS_GAP: Duration = Duration::from_millis(33);

pub struct Job {
    pub source: Source,
    pub scans_dir: PathBuf,
    pub include_all: bool,
    pub renderer: PathBuf,
    pub share_count: usize,
    pub show_lost_in_top: bool,
}

pub struct Finished {
    pub scan: SavedScan,
    pub folder: PathBuf,
}

pub enum Message {
    Event(Event),
    Queue(u64, Option<Left>),
    Saving,
    Finished(Box<Finished>),
    Refreshed(Option<Box<Finished>>),
    Failed(String),
    LoginExpired,
}

pub trait Sink {
    fn is_open(&self) -> bool;
    fn send(&self, message: Message) -> bool;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Finished,
    Failed,
    Closed,
}

#[derive(Default)]
pub struct Throttle {
    last: Option<Instant>,
}

impl Throttle {
    pub fn passes(&mut self, event: &Event, now: Instant) -> bool {
        let Event::Progress { done, total, .. } = event else {
            return true;
        };
        let due = self
            .last
            .is_none_or(|last| now.duration_since(last) >= PROGRESS_GAP);
        if done >= total || due {
            self.last = Some(now);
            return true;
        }
        false
    }
}

pub fn run(job: &Job, api: &mut dyn Api, sink: &dyn Sink) -> Outcome {
    let attempt = panic::catch_unwind(AssertUnwindSafe(|| execute(job, api, sink)));
    match attempt {
        Ok(Ok(outcome)) => outcome,
        Ok(Err(error)) if expired(&error) => end(sink, Message::LoginExpired),
        Ok(Err(error)) => end(sink, Message::Failed(format!("{error:#}"))),
        Err(_) => end(
            sink,
            Message::Failed("the scan stopped unexpectedly".to_owned()),
        ),
    }
}

fn expired(error: &anyhow::Error) -> bool {
    error
        .chain()
        .any(|cause| matches!(cause.downcast_ref(), Some(ApiError::LoginRequired)))
}

fn end(sink: &dyn Sink, message: Message) -> Outcome {
    if sink.send(message) {
        Outcome::Failed
    } else {
        Outcome::Closed
    }
}

fn execute(job: &Job, api: &mut dyn Api, sink: &dyn Sink) -> Result<Outcome> {
    let open = Cell::new(sink.is_open());
    let mut throttle = Throttle::default();
    let mut events = |event: Event| {
        if throttle.passes(&event, Instant::now()) && !sink.send(Message::Event(event)) {
            open.set(false);
        }
    };
    let mut timings = Timings::new();
    macro_rules! proceed {
        () => {
            if !open.get() || !sink.is_open() {
                return Ok(Outcome::Closed);
            }
        };
    }
    proceed!();
    let read = read(
        &job.source,
        Some(&mut *api),
        API_HINT,
        &mut timings,
        &mut events,
    )
    .map_err(|error| match error.downcast::<PlayerLookup>() {
        Ok(PlayerLookup {
            error: ApiError::LoginRequired,
            ..
        }) => anyhow::Error::new(ApiError::LoginRequired),
        Ok(lookup) => anyhow!(lookup_failure(&lookup.player, &lookup.error)),
        Err(other) => other,
    })?;
    proceed!();
    let player = read
        .player
        .ok_or_else(|| anyhow!("the osu! API did not return the player"))?;
    let mut catalog = load_catalog(
        job.source.maps.as_deref(),
        &read.scan,
        Some(&mut *api),
        API_HINT,
        pipeline::unix_now(),
        &mut timings,
        &mut events,
    )?;
    proceed!();
    let scores = find(&read.scan, &catalog, &mut timings);
    let mut found = lost(&scores, &catalog);
    pipeline::fill_set_ids(
        job.source.maps.as_deref(),
        &mut found,
        &mut catalog,
        &mut *api,
        &mut events,
    )?;
    let result =
        pipeline::potential_top(api, &player, &scores, &catalog, &mut timings, &mut events)?;
    proceed!();
    sink.send(Message::Saving);
    let scan = pipeline::saved(&job.source, job.include_all, &player, found, result);
    let folder = history::save(&job.scans_dir, &scan)?;
    let shown = scan.clone();
    if !sink.send(Message::Finished(Box::new(Finished {
        scan,
        folder: folder.clone(),
    }))) {
        return Ok(Outcome::Closed);
    }
    proceed!();
    let local = local_covers(&read.songs_dir, &read.scan, &shown);
    create_images(
        job,
        &shown,
        &folder,
        Some(&local),
        &mut events,
        &mut HttpFetch::new(),
    );
    proceed!();
    let finished = Completed {
        job,
        scan: &read.scan,
        player: &player,
        folder: &folder,
    };
    let refreshed = refresh(&finished, api, &mut catalog, &mut timings, &mut events)?;
    proceed!();
    sink.send(Message::Refreshed(refreshed));
    Ok(Outcome::Finished)
}

struct Completed<'a> {
    job: &'a Job,
    scan: &'a Scan,
    player: &'a Player,
    folder: &'a Path,
}

fn refresh(
    finished: &Completed<'_>,
    api: &mut dyn Api,
    catalog: &mut MapCatalog,
    timings: &mut Timings,
    events: &mut dyn FnMut(Event),
) -> Result<Option<Box<Finished>>> {
    let Completed {
        job,
        scan,
        player,
        folder,
    } = *finished;
    let changed = match refresh_statuses(
        job.source.maps.as_deref(),
        catalog,
        api,
        pipeline::unix_now(),
        timings,
        events,
    ) {
        Ok(changed) => changed,
        Err(error) if expired(&error) => return Err(error),
        Err(error) => {
            events(Event::Log(format!(
                "could not refresh map statuses: {error:#}"
            )));
            return Ok(None);
        }
    };
    if changed == 0 {
        return Ok(None);
    }
    let scores = find(scan, catalog, timings);
    let found = lost(&scores, catalog);
    let result = match pipeline::potential_top(api, player, &scores, catalog, timings, events) {
        Ok(result) => result,
        Err(error) if expired(&error) => return Err(error),
        Err(error) => {
            events(Event::Log(format!(
                "could not rebuild the potential top: {error:#}"
            )));
            return Ok(None);
        }
    };
    let updated = pipeline::saved(&job.source, job.include_all, player, found, result);
    match history::replace(folder, &updated) {
        Ok(()) => {
            create_images(job, &updated, folder, None, events, &mut HttpFetch::new());
            Ok(Some(Box::new(Finished {
                scan: updated,
                folder: folder.to_path_buf(),
            })))
        }
        Err(error) => {
            events(Event::Log(format!(
                "could not save the updated scan: {error:#}"
            )));
            Ok(None)
        }
    }
}

pub fn render_command(
    renderer: &Path,
    folder: &Path,
    count: usize,
    show_lost_in_top: bool,
) -> Command {
    let mut command = Command::new(renderer);
    command
        .arg("render-images")
        .arg("--cached")
        .arg("--count")
        .arg(count.to_string());
    if show_lost_in_top {
        command.arg("--show-lost-in-top");
    }
    command.arg(folder);
    command
}

fn local_covers<'a>(songs: &'a Path, scan: &Scan, saved: &SavedScan) -> LocalCovers<'a> {
    let maps: HashMap<&str, &Path> = scan
        .replays
        .iter()
        .filter_map(|replay| {
            let path = replay.beatmap.as_ref()?.path.as_path();
            path.starts_with(songs)
                .then_some((replay.header.beatmap_md5.as_str(), path))
        })
        .collect();
    let mut local = LocalCovers {
        songs: Some(songs),
        maps: HashMap::new(),
    };
    for row in &saved.lost {
        if let (Some(key), Some(path)) = (
            CoverKey::of(row.beatmapset_id, &row.beatmap_md5),
            maps.get(row.beatmap_md5.as_str()),
        ) {
            local.maps.entry(key).or_insert_with(|| path.to_path_buf());
        }
    }
    local
}

pub fn create_images(
    job: &Job,
    scan: &SavedScan,
    folder: &Path,
    local: Option<&LocalCovers>,
    events: &mut dyn FnMut(Event),
    fetch: &mut dyn Fetch,
) {
    let options = ShareOptions {
        count: job.share_count,
        include_all: scan.include_all,
        show_lost_in_top: job.show_lost_in_top,
    };
    if let Some(dir) = cache::covers_dir() {
        let keys = cover_keys(scan, &options);
        let attempt = panic::catch_unwind(AssertUnwindSafe(|| {
            let mut progress = |done, total| {
                events(Event::Progress {
                    step: Step::Covers,
                    done,
                    total,
                });
            };
            rasters::covers(&keys, &dir, local, fetch, &mut progress)
        }));
        match attempt {
            Ok(fetched) if fetched.failed > 0 => events(Event::Log(format!(
                "{} of {} covers could not be used",
                fetched.failed,
                keys.len()
            ))),
            Ok(_) => {}
            Err(payload) => events(Event::Log(format!(
                "Could not fetch the covers: {}",
                panic_message(payload.as_ref())
            ))),
        }
    }
    events(Event::Progress {
        step: Step::Images,
        done: 0,
        total: 1,
    });
    match run_renderer(job, folder) {
        Ok(warnings) => {
            for line in warnings {
                events(Event::Log(line));
            }
            events(Event::Log(format!("Images saved to {}", folder.display())));
        }
        Err(error) => events(Event::Log(format!("Could not create the images: {error}"))),
    }
}

fn run_renderer(job: &Job, folder: &Path) -> Result<Vec<String>> {
    let renderer = &job.renderer;
    let output = render_command(renderer, folder, job.share_count, job.show_lost_in_top)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .output()
        .map_err(|error| anyhow!("cannot start {}: {error}", renderer.display()))?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    let lines: Vec<String> = stderr
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect();
    if output.status.success() {
        return Ok(lines);
    }
    Err(anyhow!("{}", failure_line(&lines, &output.status)))
}

fn failure_line(lines: &[String], status: &std::process::ExitStatus) -> String {
    lines
        .iter()
        .rfind(|line| line.starts_with(FAILURE_PREFIX))
        .or_else(|| lines.last())
        .cloned()
        .unwrap_or_else(|| format!("the renderer stopped with {status}"))
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use lost_scores_app::pipeline::{Event, Step};

    use super::Throttle;

    fn progress(done: usize, total: usize) -> Event {
        Event::Progress {
            step: Step::Pp,
            done,
            total,
        }
    }

    #[test]
    fn progress_is_coalesced_to_thirty_events_a_second() {
        let mut throttle = Throttle::default();
        let start = Instant::now();
        let passed = (0..1000)
            .filter(|tick| {
                let now = start + Duration::from_millis(*tick);
                throttle.passes(&progress(*tick as usize, 5000), now)
            })
            .count();
        assert!((29..=31).contains(&passed), "{passed}");
    }

    #[test]
    fn logs_and_finished_steps_always_pass() {
        let mut throttle = Throttle::default();
        let now = Instant::now();
        assert!(throttle.passes(&progress(1, 10), now));
        assert!(!throttle.passes(&progress(2, 10), now));
        assert!(throttle.passes(&progress(10, 10), now));
        assert!(throttle.passes(&Event::Log("x".into()), now));
    }
}
