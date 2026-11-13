mod dump;
mod report;
mod setup;

use std::cmp::Reverse;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};
use lost_scores_core::maps::MapCatalog;
use lost_scores_core::{Scan, lost, pp};
use serde::Serialize;

use crate::dump::Dump;

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
    match cli.command {
        Some(Command::Scan { output }) => dump_replays(&cli.source, output.as_deref()),
        None => show_lost(&cli.source, &cli.view),
    }
}

fn read(source: &Source) -> Result<(Scan, usize)> {
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
    eprintln!(
        "osu!: {}",
        report::printable(&osu_dir.display().to_string())
    );
    eprintln!("player: {}", report::printable(&player));
    let started = Instant::now();
    let mut scan = lost_scores_core::scan(&osu_dir, &songs_dir, &player)?;
    let pp_failed = pp::fill(&osu_dir, &mut scan.replays);
    let with_pp = scan.replays.iter().filter(|r| r.pp.is_some()).count();
    eprintln!(
        "{} replays read in {:.1} s, {with_pp} of the player's have a local beatmap and pp",
        scan.replay_files,
        started.elapsed().as_secs_f64()
    );
    if pp_failed > 0 {
        eprintln!("{pp_failed} beatmaps could not be loaded for pp");
    }
    Ok((scan, pp_failed))
}

fn dump_replays(source: &Source, output: Option<&Path>) -> Result<()> {
    let (scan, pp_failed) = read(source)?;
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

fn show_lost(source: &Source, view: &View) -> Result<()> {
    let (scan, _) = read(source)?;
    let catalog = load_catalog(view.maps.as_deref())?;
    let found = lost::find(&scan.replays, &catalog);
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

fn load_catalog(path: Option<&Path>) -> Result<MapCatalog> {
    if let Some(path) = path {
        return Ok(MapCatalog::load(path)?);
    }
    match setup::maps_file().filter(|path| path.is_file()) {
        Some(path) => Ok(MapCatalog::load(&path)?),
        None => Ok(MapCatalog::default()),
    }
}

fn write_json(mut writer: impl Write, value: &impl Serialize) -> Result<()> {
    serde_json::to_writer_pretty(&mut writer, value)?;
    writeln!(writer)?;
    writer.flush()?;
    Ok(())
}
