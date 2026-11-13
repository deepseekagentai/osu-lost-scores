mod detect;
mod dump;
mod keys;
mod online;
mod report;
mod setup;
mod timings;

use std::cmp::Reverse;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, ensure};
use clap::{Args, Parser, Subcommand, ValueEnum};
use lost_scores_core::maps::{self, MapCatalog, Wanted};
use lost_scores_core::missing::fetch_missing;
use lost_scores_core::potential::{self, Play};
use lost_scores_core::{Scan, lost, pp};
use osu_api::{Credentials, Score};
use serde::Serialize;

use crate::dump::Dump;
use crate::online::{Online, Player};
use crate::timings::Timings;

#[derive(Parser)]
#[command(
    name = "lost-scores",
    version,
    about = "Find osu!stable plays that ScoreV1 overwrote and the pp they were worth"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    #[command(flatten)]
    source: Source,
    #[command(flatten)]
    view: View,
    #[arg(long, global = true, help = "Print how long each step took")]
    timings: bool,
}

#[derive(Args)]
struct Source {
    #[arg(
        long,
        global = true,
        help = "osu! folder; remembered after the first use"
    )]
    osu_dir: Option<PathBuf>,
    #[arg(
        long,
        global = true,
        help = "Player name; read from the osu! config by default"
    )]
    player: Option<String>,
    #[arg(
        long,
        global = true,
        help = "Songs folder; read from the osu! config by default"
    )]
    songs_dir: Option<PathBuf>,
    #[arg(long, global = true, help = "Do not use the osu! API")]
    offline: bool,
    #[arg(
        long,
        global = true,
        help = "Find lost plays on maps missing from Songs by downloading them (slow the first time)"
    )]
    check_missing: bool,
}

#[derive(Args)]
struct View {
    #[arg(long, help = "Map catalog with statuses and ranked dates")]
    maps: Option<PathBuf>,
    #[arg(long, help = "Also show plays on loved and unranked maps")]
    all: bool,
    #[arg(long, value_enum, default_value_t = SortBy::Pp, help = "Order of the table")]
    sort: SortBy,
    #[arg(long, default_value_t = 50, help = "Rows to show, 0 shows all")]
    limit: usize,
    #[arg(long, help = "Print JSON instead of a table")]
    json: bool,
}

#[derive(Clone, Copy, ValueEnum)]
enum SortBy {
    Pp,
    Date,
}

#[derive(Subcommand)]
enum Command {
    #[command(about = "Dump every selected replay with its beatmap and pp as JSON")]
    Scan {
        #[arg(long, help = "Write the JSON to this file instead of standard output")]
        output: Option<PathBuf>,
    },
    #[command(about = "Show your top 200 as it would be if ScoreV1 had kept your best plays")]
    Potential {
        #[arg(long, help = "Map catalog with statuses and ranked dates")]
        maps: Option<PathBuf>,
        #[arg(long, default_value_t = 50, help = "Rows to show, 0 shows all")]
        limit: usize,
        #[arg(long, help = "Print JSON instead of a table")]
        json: bool,
    },
    #[command(about = "Save the keys of your own osu! OAuth application")]
    Login,
    #[command(about = "Delete the saved osu! OAuth keys")]
    Logout,
}

#[derive(Serialize)]
struct PotentialReport<'a> {
    current_pp: f64,
    potential_pp: f64,
    current_accuracy: f64,
    potential_accuracy: f64,
    global_rank: Option<u32>,
    lost_in_top: usize,
    top: &'a [report::PotentialRow],
}

#[derive(Serialize)]
struct LostReport<'a> {
    lost_total: usize,
    lost_ranked: usize,
    lost: &'a [report::LostRow],
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {}", report::printable(&format!("{err:#}")));
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<()> {
    let mut timings = Timings::new();
    let result = match cli.command {
        Some(Command::Scan { output }) => {
            dump_replays(&cli.source, output.as_deref(), &mut timings)
        }
        Some(Command::Potential { maps, limit, json }) => {
            show_potential(&cli.source, maps.as_deref(), limit, json, &mut timings)
        }
        Some(Command::Login) => login(),
        Some(Command::Logout) => logout(),
        None => show_lost(&cli.source, &cli.view, &mut timings),
    };
    if cli.timings {
        eprintln!("{}", timings.report());
    }
    result
}

fn connect(source: &Source) -> Option<Online> {
    if source.offline {
        return None;
    }
    keys::from_env()
        .or_else(|| setup::credentials_file().and_then(|path| keys::load(&path)))
        .map(Online::new)
}

fn read(
    source: &Source,
    maps: Option<&Path>,
    mut api: Option<&mut Online>,
    timings: &mut Timings,
) -> Result<(Scan, usize, Option<Player>)> {
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
                eprintln!(
                    "could not look the player up on the osu! API: {}",
                    report::printable(&error.to_string())
                );
                (vec![player.clone()], None)
            }
        },
        None => (vec![player.clone()], None),
    };
    eprintln!(
        "osu!: {}",
        report::printable(&osu_dir.display().to_string())
    );
    eprintln!("player: {}", report::printable(&names.join(", ")));
    let started = Instant::now();
    let mut scan = lost_scores_core::scan(&osu_dir, &songs_dir, &names)?;
    timings.add("songs index", scan.timings.songs);
    timings.add("replays", scan.timings.replays);
    if source.check_missing {
        match api {
            Some(api) => add_missing_maps(&mut scan, maps, api, timings)?,
            None => eprintln!(
                "--check-missing needs the osu! API: run lost-scores login first; continuing without it"
            ),
        }
    }
    let pp_failed = timings.time("pp", || pp::fill(&osu_dir, &mut scan.replays));
    let with_pp = scan.replays.iter().filter(|r| r.pp.is_some()).count();
    eprintln!(
        "{} replays read in {:.1} s, {with_pp} of the player's have a local beatmap and pp",
        scan.replay_files,
        started.elapsed().as_secs_f64()
    );
    if pp_failed > 0 {
        eprintln!("{pp_failed} beatmaps could not be loaded for pp");
    }
    Ok((scan, pp_failed, found_player))
}

fn dump_replays(source: &Source, output: Option<&Path>, timings: &mut Timings) -> Result<()> {
    let mut api = connect(source);
    let (scan, pp_failed, _) = read(source, None, api.as_mut(), timings)?;
    let dump = Dump::new(&scan, pp_failed);
    eprintln!("{}", dump.stats);
    match output {
        Some(path) => {
            let file =
                File::create(path).with_context(|| format!("cannot create {}", path.display()))?;
            write_json(BufWriter::new(file), &dump)
        }
        None => write_json(io::stdout().lock(), &dump),
    }
}

fn show_lost(source: &Source, view: &View, timings: &mut Timings) -> Result<()> {
    let mut api = connect(source);
    let (scan, _, _) = read(source, view.maps.as_deref(), api.as_mut(), timings)?;
    let catalog = load_catalog(view.maps.as_deref(), &scan, api.as_mut(), timings)?;
    let found = timings.time("lost scores", || lost::find(&scan.replays, &catalog));
    let ranked = found.iter().filter(|l| l.status.gives_pp()).count();
    let show_all = view.all || catalog.is_empty();
    let mut rows = report::rows(&found, &catalog);
    rows.retain(|row| show_all || row.status.gives_pp());
    if let SortBy::Date = view.sort {
        rows.sort_by_key(|row| Reverse(row.unix_time));
    }
    if view.json {
        return write_json(
            io::stdout().lock(),
            &LostReport {
                lost_total: found.len(),
                lost_ranked: ranked,
                lost: &rows,
            },
        );
    }
    println!("{}", report::table(&rows, view.limit, show_all));
    if catalog.is_empty() {
        eprintln!("no map catalog yet: map statuses are unknown, so every map is shown");
    } else if view.all {
        eprintln!(
            "{} lost plays, {ranked} of them on ranked maps",
            found.len()
        );
    } else {
        eprintln!(
            "{ranked} lost plays on ranked maps; {} with loved and unranked maps (--all)",
            found.len()
        );
    }
    Ok(())
}

fn load_catalog(
    maps: Option<&Path>,
    scan: &Scan,
    mut api: Option<&mut Online>,
    timings: &mut Timings,
) -> Result<MapCatalog> {
    let started = Instant::now();
    let now = unix_now();
    let catalog_path = catalog_file(maps);
    let mut catalog = open_catalog(catalog_path.as_deref())?;
    let wanted = maps::wanted(&scan.replays, &catalog);
    if !wanted.is_empty() {
        match api.as_deref_mut() {
            Some(api) => {
                update_catalog(&mut catalog, catalog_path.as_deref(), &wanted, api, now)?;
            }
            None => eprintln!(
                "{} maps have no status yet; after `lost-scores login` they are fetched from the osu! API",
                wanted.len()
            ),
        }
    }
    timings.add("map catalog", started.elapsed());
    if let Some(api) = api {
        let ids = maps::stale(&catalog, now);
        if !ids.is_empty() {
            let changed = timings.time("status refresh", || {
                maps::refresh(&mut catalog, &ids, api, now)
            });
            if let Some(path) = &catalog_path {
                catalog.save(path)?;
            }
            eprintln!(
                "asked again about {} maps whose status can change, {} changed",
                ids.len(),
                changed?
            );
        }
    }
    Ok(catalog)
}

fn unix_now() -> i64 {
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

fn add_missing_maps(
    scan: &mut Scan,
    maps: Option<&Path>,
    api: &mut Online,
    timings: &mut Timings,
) -> Result<()> {
    let cache_dir = setup::osu_cache_dir().context("cannot find a folder for the map cache")?;
    let catalog_path = catalog_file(maps);
    let mut catalog = open_catalog(catalog_path.as_deref())?;
    let report = timings.time("missing maps", || {
        fetch_missing(&mut scan.replays, &mut catalog, &cache_dir, api)
    });
    if let Some(path) = &catalog_path {
        catalog.save(path)?;
    }
    let report = report?;
    if report.maps > 0 {
        eprintln!(
            "{} maps missing from Songs looked up: {} from the cache, {} downloaded, {} not on the server, {} changed since the replay",
            report.maps, report.cached, report.downloaded, report.not_found, report.mismatched
        );
    }
    Ok(())
}

fn show_potential(
    source: &Source,
    maps: Option<&Path>,
    limit: usize,
    json: bool,
    timings: &mut Timings,
) -> Result<()> {
    let mut api = connect(source)
        .context("the potential top needs the osu! API: run lost-scores login first")?;
    let (scan, _, player) = read(source, maps, Some(&mut api), timings)?;
    let player =
        player.context("the potential top needs the player's profile from the osu! API")?;
    let catalog = load_catalog(maps, &scan, Some(&mut api), timings)?;
    let found = timings.time("lost scores", || lost::find(&scan.replays, &catalog));
    let lost = potential::lost_plays(&found, &catalog);
    let scores: Vec<Score> = timings
        .time("online top", || api.top(player.id))?
        .into_iter()
        .filter(|score| score.pp.is_some())
        .collect();
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
    let rows = report::potential_rows(&result, &scores, &found, &lost);
    if json {
        write_json(
            io::stdout().lock(),
            &PotentialReport {
                current_pp: player.pp,
                potential_pp: result.pp,
                current_accuracy: player.accuracy,
                potential_accuracy: result.accuracy,
                global_rank: player.global_rank,
                lost_in_top: result.lost_in_top,
                top: &rows,
            },
        )?;
    } else {
        println!("{}", report::potential_table(&rows, limit));
    }
    eprintln!(
        "{} → {}pp ({:+.1}), {:.2}% → {:.2}% ({:+.2}%), {} lost plays in the top 200",
        report::thousands(player.pp),
        report::thousands(result.pp),
        result.pp - player.pp,
        player.accuracy,
        result.accuracy,
        result.accuracy - player.accuracy,
        result.lost_in_top
    );
    Ok(())
}

fn update_catalog(
    catalog: &mut MapCatalog,
    path: Option<&Path>,
    wanted: &[Wanted],
    api: &mut Online,
    now: i64,
) -> Result<()> {
    let with_id = wanted.iter().filter(|w| w.beatmap_id.is_some()).count();
    let requests = with_id.div_ceil(50) + (wanted.len() - with_id);
    eprintln!(
        "asking the osu! API about {} maps, about {requests} requests at one per second",
        wanted.len()
    );
    let result = maps::fill(catalog, wanted, api, now);
    if let Some(path) = path {
        catalog.save(path)?;
    }
    let report = result?;
    eprintln!(
        "{} maps found by id, {} by md5, {} played on an older version, {} not on the server",
        report.by_id, report.by_md5, report.outdated, report.not_found
    );
    Ok(())
}

fn login() -> Result<()> {
    let path = setup::credentials_file().context("cannot find a folder for the settings")?;
    eprintln!(
        "Create an OAuth application at https://osu.ppy.sh/home/account/edit (OAuth section, any name, callback URL http://localhost) and paste its keys."
    );
    eprint!("Client ID: ");
    io::stderr().flush()?;
    let mut client_id = String::new();
    io::stdin().read_line(&mut client_id)?;
    let client_secret = rpassword::prompt_password("Client secret: ")?;
    let credentials = Credentials {
        client_id: client_id.trim().to_owned(),
        client_secret: client_secret.trim().to_owned(),
    };
    ensure!(
        !credentials.client_id.is_empty() && !credentials.client_secret.is_empty(),
        "both the client ID and the client secret are needed"
    );
    Online::new(credentials.clone())
        .check()
        .context("the osu! API did not accept these keys")?;
    keys::save(&path, &credentials)?;
    eprintln!(
        "keys checked and saved to {}",
        report::printable(&path.display().to_string())
    );
    Ok(())
}

fn logout() -> Result<()> {
    let path = setup::credentials_file().context("cannot find a folder for the settings")?;
    if keys::delete(&path)? {
        eprintln!(
            "keys deleted from {}",
            report::printable(&path.display().to_string())
        );
    } else {
        eprintln!("no saved keys");
    }
    Ok(())
}

fn write_json(mut writer: impl Write, value: &impl Serialize) -> Result<()> {
    serde_json::to_writer_pretty(&mut writer, value)?;
    writeln!(writer)?;
    writer.flush()?;
    Ok(())
}
