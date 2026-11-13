mod detect;
mod dump;
mod keys;
mod online;
mod progress;
mod report;
mod setup;
mod timings;

use std::cmp::Reverse;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, ensure};
use clap::{Args, Parser, Subcommand, ValueEnum};
use lost_scores_core::maps::{self, MapCatalog, Wanted};
use lost_scores_core::missing::{fetch_missing, missing_maps};
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
    about = "Find osu!stable plays that ScoreV1 overwrote and the pp they were worth",
    after_help = guide(),
    args_conflicts_with_subcommands = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    #[command(flatten)]
    view: View,
    #[command(flatten)]
    source: Source,
}

#[derive(Args)]
struct Source {
    #[arg(
        long,
        help = "osu! folder; found automatically and remembered once given"
    )]
    osu_dir: Option<PathBuf>,
    #[arg(long, help = "Player name; read from the osu! config by default")]
    player: Option<String>,
    #[arg(long, help = "Songs folder; read from the osu! config by default")]
    songs_dir: Option<PathBuf>,
    #[arg(long, help = "Do not use the osu! API, even with saved keys")]
    offline: bool,
    #[arg(
        long,
        help = "Also find lost plays on maps missing from Songs by downloading them (slow the first time)"
    )]
    check_missing: bool,
    #[arg(long, help = "Print how long each step took")]
    timings: bool,
}

#[derive(Args)]
struct View {
    #[arg(long, hide = true)]
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
    #[command(about = "Show your top 200 as it would be if ScoreV1 had kept your best plays")]
    Potential {
        #[arg(long, hide = true)]
        maps: Option<PathBuf>,
        #[arg(long, default_value_t = 50, help = "Rows to show, 0 shows all")]
        limit: usize,
        #[arg(long, help = "Print JSON instead of a table")]
        json: bool,
        #[command(flatten)]
        source: Source,
    },
    #[command(about = "Save the keys of your own osu! OAuth application")]
    Login,
    #[command(about = "Delete the saved osu! OAuth keys")]
    Logout,
    #[command(
        about = "Dump every replay of the player with its beatmap and pp as JSON, for checks"
    )]
    Scan {
        #[arg(long, help = "Write the JSON to this file instead of standard output")]
        output: Option<PathBuf>,
        #[command(flatten)]
        source: Source,
    },
}

const EXAMPLES: [(&str, &str); 5] = [
    ("lost-scores", "lost plays on ranked maps"),
    (
        "lost-scores --all --sort date",
        "with loved and unranked maps, newest first",
    ),
    (
        "lost-scores --check-missing",
        "also maps that are no longer in Songs",
    ),
    (
        "lost-scores potential",
        "your top 200 with the lost plays put back",
    ),
    ("lost-scores login", "save your osu! API keys, needed once"),
];

fn guide() -> String {
    let mut text = String::from(
        "Without a command, lists your lost plays on ranked maps, best first.\n\nExamples:\n",
    );
    for (command, what) in EXAMPLES {
        text.push_str(&format!("  {command:<31}{what}\n"));
    }
    text.push_str("\nFiles:\n");
    let files = [
        ("settings", setup::config_file()),
        ("API keys", setup::credentials_file()),
        ("map data", setup::maps_file()),
        ("map cache", setup::osu_cache_dir()),
    ];
    for (name, path) in files {
        let path = path.map_or_else(
            || "no folder found".to_owned(),
            |path| report::printable(&path.display().to_string()),
        );
        text.push_str(&format!("  {name:<11}{path}\n"));
    }
    text.trim_end().to_owned()
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
    let (result, source) = match &cli.command {
        Some(Command::Scan { output, source }) => (
            dump_replays(source, output.as_deref(), &mut timings),
            Some(source),
        ),
        Some(Command::Potential {
            maps,
            limit,
            json,
            source,
        }) => (
            show_potential(source, maps.as_deref(), *limit, *json, &mut timings),
            Some(source),
        ),
        Some(Command::Login) => (login(), None),
        Some(Command::Logout) => (logout(), None),
        None => (
            show_lost(&cli.source, &cli.view, &mut timings),
            Some(&cli.source),
        ),
    };
    if source.is_some_and(|source| source.timings) {
        eprintln!("{}", timings.report());
    }
    result
}

fn saved_keys() -> Option<Credentials> {
    keys::from_env().or_else(|| setup::credentials_file().and_then(|path| keys::load(&path)))
}

fn connect(source: &Source) -> Option<Online> {
    if source.offline {
        return None;
    }
    saved_keys().map(Online::new)
}

fn api_hint(source: &Source) -> &'static str {
    if source.offline && saved_keys().is_some() {
        "run without --offline"
    } else {
        "run `lost-scores login` first"
    }
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
    let mut checking = Duration::ZERO;
    if source.check_missing {
        match api {
            Some(api) => {
                let started = Instant::now();
                add_missing_maps(&mut scan, maps, api, timings)?;
                checking = started.elapsed();
            }
            None => eprintln!(
                "--check-missing needs the osu! API: {}; continuing without it",
                api_hint(source)
            ),
        }
    }
    let pp_failed = timings.time("pp", || pp::fill(&osu_dir, &mut scan.replays));
    let with_pp = scan.replays.iter().filter(|r| r.pp.is_some()).count();
    eprintln!(
        "{} replays read in {:.1} s, {with_pp} of the player's have a local beatmap and pp",
        scan.replay_files,
        (started.elapsed() - checking).as_secs_f64()
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
    let catalog = load_catalog(source, view.maps.as_deref(), &scan, api.as_mut(), timings)?;
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
    source: &Source,
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
                "{} maps have no status yet; to fetch them from the osu! API, {}",
                wanted.len(),
                api_hint(source)
            ),
        }
    }
    timings.add("map catalog", started.elapsed());
    if let Some(api) = api {
        let ids = maps::stale(&catalog, now);
        if !ids.is_empty() {
            let bar = progress::bar("status refresh");
            let changed = timings.time("status refresh", || {
                maps::refresh(&mut catalog, &ids, api, now, progress::track(&bar))
            });
            bar.finish_and_clear();
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
    let md5s = missing_maps(&scan.replays, &catalog);
    if md5s.is_empty() {
        return Ok(());
    }
    eprintln!(
        "{} maps missing from Songs could hold a lost play; each one not in the cache takes two requests at one per second",
        md5s.len()
    );
    let bar = progress::bar("missing maps");
    let report = timings.time("missing maps", || {
        fetch_missing(
            &md5s,
            &mut scan.replays,
            &mut catalog,
            &cache_dir,
            api,
            progress::track(&bar),
        )
    });
    bar.finish_and_clear();
    if let Some(path) = &catalog_path {
        catalog.save(path)?;
    }
    let report = report?;
    eprintln!(
        "{} maps missing from Songs looked up: {} from the cache, {} downloaded, {} not on the server, {} changed since the replay",
        report.maps, report.cached, report.downloaded, report.not_found, report.mismatched
    );
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
        .with_context(|| format!("the potential top needs the osu! API: {}", api_hint(source)))?;
    let (scan, _, player) = read(source, maps, Some(&mut api), timings)?;
    let player =
        player.context("the potential top needs the player's profile from the osu! API")?;
    let catalog = load_catalog(source, maps, &scan, Some(&mut api), timings)?;
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
    let requests = with_id.div_ceil(maps::BATCH_SIZE) + (wanted.len() - with_id);
    eprintln!(
        "asking the osu! API about {} maps, about {requests} requests at one per second",
        wanted.len()
    );
    let bar = progress::bar("map statuses");
    let result = maps::fill(catalog, wanted, api, now, progress::track(&bar));
    bar.finish_and_clear();
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

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::Cli;

    #[test]
    fn arguments_are_consistent() {
        Cli::command().debug_assert();
    }
}
