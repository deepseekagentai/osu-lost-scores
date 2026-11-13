mod dump;

use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use crate::dump::Dump;

#[derive(Parser)]
#[command(
    version,
    about = "Find osu!stable plays that ScoreV1 overwrote and the pp they were worth"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    #[command(about = "List the player's replays and their local beatmaps as JSON")]
    Scan {
        #[arg(long, help = "osu! folder that contains Data/r and Songs")]
        osu_dir: PathBuf,
        #[arg(long, help = "Player name as stored in the replays")]
        player: String,
        #[arg(long, help = "Write the JSON to this file instead of standard output")]
        output: Option<PathBuf>,
    },
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Scan {
            osu_dir,
            player,
            output,
        } => scan(&osu_dir, &player, output.as_deref()),
    }
}

fn scan(osu_dir: &Path, player: &str, output: Option<&Path>) -> Result<()> {
    let mut report = lost_scores_core::scan(osu_dir, &osu_dir.join("Songs"), player)?;
    let failed = lost_scores_core::pp::fill(osu_dir, &mut report.replays);
    let dump = Dump::new(&report, failed);
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

fn write_json(mut writer: impl Write, dump: &Dump) -> Result<()> {
    serde_json::to_writer_pretty(&mut writer, dump)?;
    writeln!(writer)?;
    writer.flush()?;
    Ok(())
}
