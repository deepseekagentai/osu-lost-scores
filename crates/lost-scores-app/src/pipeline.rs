use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use lost_scores_core::maps::{self, MapCatalog, MapSource, Wanted};
use lost_scores_core::missing::{OsuFiles, fetch_missing, missing_maps};
use lost_scores_core::potential::{self, Play};
use lost_scores_core::{Scan, Stage, lost as find_lost, pp};
use osu_api::{ApiError, Score};
use serde::{Deserialize, Serialize};

use crate::online::Player;
use crate::rows::{
    LostRow, OnlineRow, PotentialRow, online_rows, potential_rows, printable, rows as lost_rows,
};
use crate::setup;
use crate::timings::Timings;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Songs,
    Replays,
    MissingMaps,
    Pp,
    Catalog,
    Refresh,
    OnlineTop,
    Potential,
    Covers,
    Images,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Log(String),
    Progress {
        step: Step,
        done: usize,
        total: usize,
    },
}

pub trait Api: MapSource<Error = ApiError> + OsuFiles<Error = ApiError> {
    fn player(&mut self, name: &str) -> Result<Player, ApiError>;
    fn top(&mut self, user_id: u32) -> Result<Vec<Score>, ApiError>;
}

pub struct Source {
    pub osu_dir: Option<PathBuf>,
    pub player: Option<String>,
    pub songs_dir: Option<PathBuf>,
    pub check_missing: bool,
    pub maps: Option<PathBuf>,
}

pub struct Read {
    pub osu_dir: PathBuf,
    pub scan: Scan,
    pub pp_failed: usize,
    pub player: Option<Player>,
}

pub struct Lost {
    pub rows: Vec<LostRow>,
    pub total: usize,
    pub ranked: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PotentialResult {
    pub current_pp: f64,
    pub potential_pp: f64,
    pub current_accuracy: f64,
    pub potential_accuracy: f64,
    pub global_rank: Option<u32>,
    pub lost_in_top: usize,
    pub rows: Vec<PotentialRow>,
    pub online: Vec<OnlineRow>,
}

fn finished(events: &mut dyn FnMut(Event), step: Step) {
    events(Event::Progress {
        step,
        done: 1,
        total: 1,
    });
}

fn forwarded<T: Send>(
    events: &mut dyn FnMut(Event),
    work: impl FnOnce(mpsc::Sender<(Step, usize, usize)>) -> T + Send,
) -> T {
    let (sender, receiver) = mpsc::channel();
    thread::scope(|scope| {
        let worker = scope.spawn(move || work(sender));
        for (step, done, total) in receiver {
            events(Event::Progress { step, done, total });
        }
        worker
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
    })
}

pub fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            i64::try_from(since.as_secs()).unwrap_or(i64::MAX)
        })
}

fn catalog_file(maps: Option<&Path>) -> Option<PathBuf> {
    maps.map(Path::to_path_buf).or_else(setup::maps_file)
}

fn open_catalog(path: Option<&Path>) -> Result<MapCatalog> {
    Ok(match path {
        Some(path) if path.is_file() => MapCatalog::load(path)?,
        _ => MapCatalog::default(),
    })
}

pub fn read(
    source: &Source,
    mut api: Option<&mut dyn Api>,
    api_hint: &str,
    timings: &mut Timings,
    events: &mut dyn FnMut(Event),
) -> Result<Read> {
    let osu_dir = setup::resolve_osu_dir(source.osu_dir.as_deref())?;
    let config = setup::read_user_config(&osu_dir);
    let player = match &source.player {
        Some(player) => player.clone(),
        None => setup::player(&config)?,
    };
    let songs_dir = match &source.songs_dir {
        Some(dir) => dir.clone(),
        None => setup::songs_dir(&osu_dir, &config)?,
    };
    let (names, found_player) = match api.as_deref_mut() {
        Some(api) => match timings.time("player lookup", || api.player(&player)) {
            Ok(found) => (found.names.clone(), Some(found)),
            Err(error) => {
                events(Event::Log(format!(
                    "could not look the player up on the osu! API: {}",
                    printable(&error.to_string())
                )));
                (vec![player.clone()], None)
            }
        },
        None => (vec![player.clone()], None),
    };
    events(Event::Log(format!(
        "osu!: {}",
        printable(&osu_dir.display().to_string())
    )));
    events(Event::Log(format!(
        "player: {}",
        printable(&names.join(", "))
    )));
    let started = Instant::now();
    let mut scan = forwarded(events, |sender| {
        lost_scores_core::scan_with(&osu_dir, &songs_dir, &names, &|stage, done, total| {
            let step = match stage {
                Stage::Songs => Step::Songs,
                Stage::Replays => Step::Replays,
            };
            let _ = sender.send((step, done, total));
        })
    })?;
    timings.add("songs index", scan.timings.songs);
    timings.add("replays", scan.timings.replays);
    let mut checking = Duration::ZERO;
    if source.check_missing {
        match api {
            Some(api) => {
                let started = Instant::now();
                add_missing_maps(&mut scan, source.maps.as_deref(), api, timings, events)?;
                checking = started.elapsed();
            }
            None => events(Event::Log(format!(
                "--check-missing needs the osu! API: {api_hint}; continuing without it"
            ))),
        }
    }
    let pp_failed = timings.time("pp", || {
        forwarded(events, |sender| {
            pp::fill_with(&osu_dir, &mut scan.replays, &|done, total| {
                let _ = sender.send((Step::Pp, done, total));
            })
        })
    });
    let with_pp = scan.replays.iter().filter(|r| r.pp.is_some()).count();
    events(Event::Log(format!(
        "{} replays read in {:.1} s, {with_pp} of the player's have a local beatmap and pp",
        scan.replay_files,
        (started.elapsed() - checking).as_secs_f64()
    )));
    if pp_failed > 0 {
        events(Event::Log(format!(
            "{pp_failed} beatmaps could not be loaded for pp"
        )));
    }
    Ok(Read {
        osu_dir,
        scan,
        pp_failed,
        player: found_player,
    })
}

fn add_missing_maps(
    scan: &mut Scan,
    maps: Option<&Path>,
    api: &mut dyn Api,
    timings: &mut Timings,
    events: &mut dyn FnMut(Event),
) -> Result<()> {
    let cache_dir = setup::osu_cache_dir().context("cannot find a folder for the map cache")?;
    let catalog_path = catalog_file(maps);
    let mut catalog = open_catalog(catalog_path.as_deref())?;
    let md5s = missing_maps(&scan.replays, &catalog);
    if md5s.is_empty() {
        return Ok(());
    }
    events(Event::Log(format!(
        "{} maps missing from Songs could hold a lost play; each one not in the cache takes two requests at one per second",
        md5s.len()
    )));
    let report = timings.time("missing maps", || {
        fetch_missing(
            &md5s,
            &mut scan.replays,
            &mut catalog,
            &cache_dir,
            api,
            |done, total| {
                events(Event::Progress {
                    step: Step::MissingMaps,
                    done,
                    total,
                });
            },
        )
    });
    if let Some(path) = &catalog_path {
        catalog.save(path)?;
    }
    let report = report?;
    events(Event::Log(format!(
        "{} maps missing from Songs looked up: {} from the cache, {} downloaded, {} not on the server, {} changed since the replay",
        report.maps, report.cached, report.downloaded, report.not_found, report.mismatched
    )));
    Ok(())
}

pub fn load_catalog(
    maps: Option<&Path>,
    scan: &Scan,
    api: Option<&mut dyn Api>,
    api_hint: &str,
    now: i64,
    timings: &mut Timings,
    events: &mut dyn FnMut(Event),
) -> Result<MapCatalog> {
    let started = Instant::now();
    let catalog_path = catalog_file(maps);
    let mut catalog = open_catalog(catalog_path.as_deref())?;
    let wanted = maps::wanted(&scan.replays, &catalog);
    if !wanted.is_empty() {
        match api {
            Some(api) => {
                update_catalog(
                    &mut catalog,
                    catalog_path.as_deref(),
                    &wanted,
                    api,
                    now,
                    events,
                )?;
            }
            None => events(Event::Log(format!(
                "{} maps have no status yet; to fetch them from the osu! API, {api_hint}",
                wanted.len()
            ))),
        }
    }
    timings.add("map catalog", started.elapsed());
    Ok(catalog)
}

fn update_catalog(
    catalog: &mut MapCatalog,
    path: Option<&Path>,
    wanted: &[Wanted],
    api: &mut dyn Api,
    now: i64,
    events: &mut dyn FnMut(Event),
) -> Result<()> {
    let with_id = wanted.iter().filter(|w| w.beatmap_id.is_some()).count();
    let requests = with_id.div_ceil(maps::BATCH_SIZE) + (wanted.len() - with_id);
    events(Event::Log(format!(
        "asking the osu! API about {} maps, about {requests} requests at one per second",
        wanted.len()
    )));
    let result = maps::fill(catalog, wanted, api, now, |done, total| {
        events(Event::Progress {
            step: Step::Catalog,
            done,
            total,
        });
    });
    if let Some(path) = path {
        catalog.save(path)?;
    }
    let report = result?;
    events(Event::Log(format!(
        "{} maps found by id, {} by md5, {} played on an older version, {} not on the server",
        report.by_id, report.by_md5, report.outdated, report.not_found
    )));
    Ok(())
}

pub fn refresh_statuses(
    maps: Option<&Path>,
    catalog: &mut MapCatalog,
    api: &mut dyn Api,
    now: i64,
    timings: &mut Timings,
    events: &mut dyn FnMut(Event),
) -> Result<usize> {
    let ids = maps::stale(catalog, now);
    if ids.is_empty() {
        return Ok(0);
    }
    let changed = timings.time("status refresh", || {
        maps::refresh(catalog, &ids, api, now, |done, total| {
            events(Event::Progress {
                step: Step::Refresh,
                done,
                total,
            });
        })
    });
    if let Some(path) = catalog_file(maps) {
        catalog.save(&path)?;
    }
    let changed = changed?;
    events(Event::Log(format!(
        "asked again about {} maps whose status can change, {changed} changed",
        ids.len()
    )));
    Ok(changed)
}

pub fn lost(scan: &Scan, catalog: &MapCatalog, timings: &mut Timings) -> Lost {
    let found = timings.time("lost scores", || find_lost::find(&scan.replays, catalog));
    Lost {
        total: found.len(),
        ranked: found.iter().filter(|l| l.status.gives_pp()).count(),
        rows: lost_rows(&found, catalog),
    }
}

pub fn potential(
    api: &mut dyn Api,
    player: &Player,
    scan: &Scan,
    catalog: &MapCatalog,
    timings: &mut Timings,
    events: &mut dyn FnMut(Event),
) -> Result<PotentialResult> {
    let found = timings.time("lost scores", || find_lost::find(&scan.replays, catalog));
    let lost = potential::lost_plays(&found, catalog);
    let scores: Vec<Score> = timings
        .time("online top", || api.top(player.id))?
        .into_iter()
        .filter(|score| score.pp.is_some())
        .collect();
    finished(events, Step::OnlineTop);
    let online: Vec<Play> = scores
        .iter()
        .map(|score| Play {
            beatmap_id: score.beatmap.id,
            pp: score.pp.unwrap_or_default(),
            accuracy: score.accuracy * 100.0,
        })
        .collect();
    let lost_only: Vec<Play> = lost.iter().map(|(_, play)| *play).collect();
    let result = timings.time("potential top", || {
        potential::potential(&online, &lost_only, player.pp)
    });
    finished(events, Step::Potential);
    Ok(PotentialResult {
        current_pp: player.pp,
        potential_pp: result.pp,
        current_accuracy: player.accuracy,
        potential_accuracy: result.accuracy,
        global_rank: player.global_rank,
        lost_in_top: result.lost_in_top,
        rows: potential_rows(&result, &scores, &found, &lost),
        online: online_rows(&scores),
    })
}
