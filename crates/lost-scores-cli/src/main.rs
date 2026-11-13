mod detect;
mod dump;
mod keys;
mod online;
mod report;
mod setup;

use std::cmp::Reverse;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use anyhow::{Context, Result, ensure};
use clap::{Args, Parser, Subcommand, ValueEnum};
use lost_scores_core::maps::{self, MapCatalog, Wanted};
use lost_scores_core::{Scan, lost, pp};
use osu_api::Credentials;
use serde::Serialize;

use crate::dump::Dump;
use crate::online::Online;

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
    #[arg(long, global = true, help = "Do not use the osu! API")]
    offline: bool,
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
    #[command(about = "Save the keys of your own osu! OAuth application")]
    Login,
    #[command(about = "Delete the saved osu! OAuth keys")]
    Logout,
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
        Some(Command::Login) => login(),
        Some(Command::Logout) => logout(),
        None => show_lost(&cli.source, &cli.view),
    }
}

fn connect(source: &Source) -> Option<Online> {
    if source.offline {
        return None;
    }
    keys::from_env()
        .or_else(|| setup::credentials_file().and_then(|path| keys::load(&path)))
        .map(Online::new)
}

fn read(source: &Source, api: Option<&mut Online>) -> Result<(Scan, usize)> {
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
    let names = match api {
        Some(api) => api.names(&player).unwrap_or_else(|error| {
            eprintln!(
                "could not look the player up on the osu! API: {}",
                report::printable(&error.to_string())
            );
            vec![player.clone()]
        }),
        None => vec![player.clone()],
    };
    eprintln!(
        "osu!: {}",
        report::printable(&osu_dir.display().to_string())
    );
    eprintln!("player: {}", report::printable(&names.join(", ")));
    let started = Instant::now();
    let mut scan = lost_scores_core::scan(&osu_dir, &songs_dir, &names)?;
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
    let mut api = connect(source);
    let (scan, pp_failed) = read(source, api.as_mut())?;
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
    let mut api = connect(source);
    let (scan, _) = read(source, api.as_mut())?;
    let catalog_path = view.maps.clone().or_else(setup::maps_file);
    let mut catalog = match &catalog_path {
        Some(path) if path.is_file() => MapCatalog::load(path)?,
        _ => MapCatalog::default(),
    };
    let wanted = maps::wanted(&scan.replays, &catalog);
    if !wanted.is_empty() {
        match api.as_mut() {
            Some(api) => update_catalog(&mut catalog, catalog_path.as_deref(), &wanted, api)?,
            None => eprintln!(
                "{} maps have no status yet; after `lost-scores login` they are fetched from the osu! API",
                wanted.len()
            ),
        }
    }
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

fn update_catalog(
    catalog: &mut MapCatalog,
    path: Option<&Path>,
    wanted: &[Wanted],
    api: &mut Online,
) -> Result<()> {
    let with_id = wanted.iter().filter(|w| w.beatmap_id.is_some()).count();
    let requests = with_id.div_ceil(50) + (wanted.len() - with_id);
    eprintln!(
        "asking the osu! API about {} maps, about {requests} requests at one per second",
        wanted.len()
    );
    let result = maps::fill(catalog, wanted, api);
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
