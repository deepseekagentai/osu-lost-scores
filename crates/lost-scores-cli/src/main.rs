mod dump;
mod progress;
mod table;

use std::cmp::Reverse;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::thread;

use anyhow::{Context, Result, ensure};
use clap::{Args, Parser, Subcommand, ValueEnum};
use lost_scores_app::keys;
use lost_scores_app::login;
use lost_scores_app::online::{Online, USER_AGENT};
use lost_scores_app::pipeline::{self, Api, Event};
use lost_scores_app::rows;
use lost_scores_app::session::{self, Session};
use lost_scores_app::setup;
use lost_scores_app::shared::{self, Shared};
use lost_scores_app::timings::Timings;
use osu_api::{Credentials, Http, Server, Transport};
use serde::Serialize;

use crate::dump::Dump;
use crate::progress::{Notes, Printer};

const OFFLINE_HINT: &str = "run without --offline";
const LOGIN_HINT: &str = "run `lost-scores login` first";

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
    #[arg(
        long,
        help = "Do not go online: use only the map statuses saved earlier, without the profile"
    )]
    offline: bool,
    #[arg(long, hide = true)]
    server: Option<String>,
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
    #[command(
        about = "Log in with your osu! account in the browser to scan through the shared server"
    )]
    Login {
        #[arg(
            long,
            help = "Save the keys of your own osu! OAuth application instead"
        )]
        keys: bool,
    },
    #[command(about = "Log out of osu! and delete the saved osu! OAuth keys")]
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

const EXAMPLES: [(&str, &str); 6] = [
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
    (
        "lost-scores login",
        "log in with osu! to scan through the shared server",
    ),
    (
        "lost-scores login --keys",
        "use your own osu! API keys instead of the shared server",
    ),
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
        ("osu! login", setup::session_file()),
        ("map data", setup::maps_file()),
        ("map cache", setup::osu_cache_dir()),
    ];
    for (name, path) in files {
        let path = path.map_or_else(
            || "no folder found".to_owned(),
            |path| rows::printable(&path.display().to_string()),
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
    top: &'a [rows::PotentialRow],
}

#[derive(Serialize)]
struct LostReport<'a> {
    lost_total: usize,
    lost_ranked: usize,
    lost: &'a [rows::LostRow],
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {}", rows::printable(&format!("{err:#}")));
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<()> {
    let mut timings = Timings::new();
    let mut printer = Printer::new();
    let notes = printer.notes();
    let mut events = |event| printer.handle(event);
    let (result, source) = match &cli.command {
        Some(Command::Scan { output, source }) => (
            dump_replays(source, output.as_deref(), &notes, &mut timings, &mut events),
            Some(source),
        ),
        Some(Command::Potential {
            maps,
            limit,
            json,
            source,
        }) => (
            show_potential(
                source,
                maps.as_deref(),
                (*limit, *json),
                &notes,
                &mut timings,
                &mut events,
            ),
            Some(source),
        ),
        Some(Command::Login { keys }) => (if *keys { save_keys() } else { log_in() }, None),
        Some(Command::Logout) => (logout(), None),
        None => (
            show_lost(&cli.source, &cli.view, &notes, &mut timings, &mut events),
            Some(&cli.source),
        ),
    };
    finish_run(
        &mut printer,
        &timings.report(),
        source.is_some_and(|source| source.timings),
    );
    result
}

fn finish_run(printer: &mut Printer, timings: &str, show_timings: bool) {
    printer.clear();
    if show_timings {
        eprintln!("{timings}");
    }
}

fn saved_keys() -> Option<Credentials> {
    keys::from_env().or_else(|| setup::credentials_file().and_then(|path| keys::load(&path)))
}

fn saved_session() -> Option<Session> {
    setup::session_file().and_then(|path| session::load(&path))
}

fn connect(source: &Source, notes: &Notes) -> (Option<Box<dyn Api>>, &'static str) {
    client(
        saved_keys(),
        saved_session(),
        source.offline,
        source.server.as_deref(),
        notes.clone(),
    )
}

fn client(
    keys: Option<Credentials>,
    session: Option<Session>,
    offline: bool,
    server: Option<&str>,
    notes: Notes,
) -> (Option<Box<dyn Api>>, &'static str) {
    if offline {
        return (None, OFFLINE_HINT);
    }
    if let (Some(keys), None) = (&keys, server) {
        return (Some(Box::new(Online::new(keys.clone()))), OFFLINE_HINT);
    }
    let base = server.map_or_else(shared::server_url, str::to_owned);
    let Some(session) = session.filter(|session| session.server == base.trim_end_matches('/'))
    else {
        return (None, LOGIN_HINT);
    };
    let api = Shared::at(&base)
        .with_session(&session.token)
        .on_wait(move |seconds| notes.waiting(seconds));
    (Some(Box::new(api)), OFFLINE_HINT)
}

fn pipeline_source(source: &Source, maps: Option<&Path>) -> pipeline::Source {
    pipeline::Source {
        osu_dir: source.osu_dir.clone(),
        player: source.player.clone(),
        songs_dir: source.songs_dir.clone(),
        check_missing: source.check_missing,
        maps: maps.map(Path::to_path_buf),
        stop_on_lookup_error: false,
    }
}

fn dump_replays(
    source: &Source,
    output: Option<&Path>,
    notes: &Notes,
    timings: &mut Timings,
    events: &mut dyn FnMut(Event),
) -> Result<()> {
    let (mut api, hint) = connect(source, notes);
    let read = pipeline::read(
        &pipeline_source(source, None),
        api.as_deref_mut().map(|api| api as &mut dyn Api),
        hint,
        timings,
        events,
    )?;
    let dump = Dump::new(&read.scan, read.pp_failed);
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

fn show_lost(
    source: &Source,
    view: &View,
    notes: &Notes,
    timings: &mut Timings,
    events: &mut dyn FnMut(Event),
) -> Result<()> {
    let (mut api, hint) = connect(source, notes);
    let read = pipeline::read(
        &pipeline_source(source, view.maps.as_deref()),
        api.as_deref_mut().map(|api| api as &mut dyn Api),
        hint,
        timings,
        events,
    )?;
    let mut catalog = pipeline::load_catalog(
        view.maps.as_deref(),
        &read.scan,
        api.as_deref_mut().map(|api| api as &mut dyn Api),
        hint,
        pipeline::unix_now(),
        timings,
        events,
    )?;
    if let Some(api) = api.as_deref_mut().map(|api| api as &mut dyn Api) {
        pipeline::refresh_statuses(
            view.maps.as_deref(),
            &mut catalog,
            api,
            pipeline::unix_now(),
            timings,
            events,
        )?;
    }
    let scores = pipeline::find(&read.scan, &catalog, timings);
    let found = pipeline::lost(&scores, &catalog);
    let show_all = view.all || catalog.is_empty();
    let mut rows = found.rows;
    rows.retain(|row| show_all || row.status.gives_pp());
    if let SortBy::Date = view.sort {
        rows.sort_by_key(|row| Reverse(row.unix_time));
    }
    if view.json {
        return write_json(
            io::stdout().lock(),
            &LostReport {
                lost_total: found.total,
                lost_ranked: found.ranked,
                lost: &rows,
            },
        );
    }
    println!("{}", table::table(&rows, view.limit, show_all));
    if catalog.is_empty() {
        eprintln!("no map catalog yet: map statuses are unknown, so every map is shown");
    } else if view.all {
        eprintln!(
            "{} lost plays, {} of them on ranked maps",
            found.total, found.ranked
        );
    } else {
        eprintln!(
            "{} lost plays on ranked maps; {} with loved and unranked maps (--all)",
            found.ranked, found.total
        );
    }
    Ok(())
}

fn show_potential(
    source: &Source,
    maps: Option<&Path>,
    (limit, json): (usize, bool),
    notes: &Notes,
    timings: &mut Timings,
    events: &mut dyn FnMut(Event),
) -> Result<()> {
    let (api, hint) = connect(source, notes);
    let mut api = api.with_context(|| format!("the potential top needs the osu! API: {hint}"))?;
    let read = pipeline::read(
        &pipeline_source(source, maps),
        Some(api.as_mut() as &mut dyn Api),
        hint,
        timings,
        events,
    )?;
    let player = read
        .player
        .context("the potential top needs the player's profile from the osu! API")?;
    let mut catalog = pipeline::load_catalog(
        maps,
        &read.scan,
        Some(api.as_mut() as &mut dyn Api),
        hint,
        pipeline::unix_now(),
        timings,
        events,
    )?;
    pipeline::refresh_statuses(
        maps,
        &mut catalog,
        api.as_mut(),
        pipeline::unix_now(),
        timings,
        events,
    )?;
    let scores = pipeline::find(&read.scan, &catalog, timings);
    let result = pipeline::potential(api.as_mut(), &player, &scores, &catalog, timings, events)?;
    if json {
        write_json(
            io::stdout().lock(),
            &PotentialReport {
                current_pp: result.current_pp,
                potential_pp: result.potential_pp,
                current_accuracy: result.current_accuracy,
                potential_accuracy: result.potential_accuracy,
                global_rank: result.global_rank,
                lost_in_top: result.lost_in_top,
                top: &result.rows,
            },
        )?;
    } else {
        println!("{}", table::potential_table(&result.rows, limit));
    }
    eprintln!(
        "{} → {}pp ({:+.1}), {:.2}% → {:.2}% ({:+.2}%), {} lost plays in the top 200",
        rows::thousands(result.current_pp),
        rows::thousands(result.potential_pp),
        result.potential_pp - result.current_pp,
        result.current_accuracy,
        result.potential_accuracy,
        result.potential_accuracy - result.current_accuracy,
        result.lost_in_top
    );
    Ok(())
}

fn log_in() -> Result<()> {
    let path = setup::session_file().context("cannot find a folder for the settings")?;
    let server = Server::new(Http::new(USER_AGENT), &shared::server_url());
    let session = log_in_with(server, |url: &str| open::that_detached(url))?;
    session::save(&path, &session)?;
    eprintln!("logged in as {}", rows::printable(&session.username));
    Ok(())
}

fn log_in_with<T: Transport>(
    mut server: Server<T>,
    open: impl FnOnce(&str) -> io::Result<()> + Send + 'static,
) -> Result<Session> {
    let pending = login::begin(&server).context("cannot listen for the browser")?;
    eprintln!(
        "Log in with osu! in the browser. If it does not open, go to:\n  {}",
        pending.url
    );
    let url = pending.url.clone();
    thread::spawn(move || {
        if open(&url).is_err() {
            eprintln!("cannot open the browser; open the address above yourself");
        }
    });
    Ok(login::wait(&mut server, pending, login::LIMIT)?)
}

fn save_keys() -> Result<()> {
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
        rows::printable(&path.display().to_string())
    );
    Ok(())
}

fn logout() -> Result<()> {
    let keys_path = setup::credentials_file().context("cannot find a folder for the settings")?;
    let session_path = setup::session_file().context("cannot find a folder for the settings")?;
    if let Some(saved) = session::load(&session_path) {
        let revoked = Server::new(Http::new(USER_AGENT), &saved.server)
            .with_session(&saved.token)
            .logout();
        if let Err(error) = revoked {
            eprintln!(
                "cannot end the session on the server ({}); it expires by itself",
                rows::printable(&error.to_string())
            );
        }
    }
    let logged_out = session::delete(&session_path)?;
    if logged_out {
        eprintln!("logged out of osu!");
    }
    let keys_deleted = keys::delete(&keys_path)?;
    if keys_deleted {
        eprintln!(
            "keys deleted from {}",
            rows::printable(&keys_path.display().to_string())
        );
    }
    if !logged_out && !keys_deleted {
        eprintln!("not logged in and no saved keys");
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
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    use clap::{CommandFactory, Parser};
    use lost_scores_app::session::Session;
    use osu_api::{ApiError, Credentials, Response, Server, Transport};

    use super::{Cli, Command, LOGIN_HINT, OFFLINE_HINT, client, log_in_with};
    use crate::progress::Notes;

    #[test]
    fn arguments_are_consistent() {
        Cli::command().debug_assert();
    }

    fn keys() -> Credentials {
        Credentials {
            client_id: "1".into(),
            client_secret: "x".into(),
        }
    }

    const SERVER: &str = "http://127.0.0.1:9";

    fn session() -> Session {
        Session {
            token: "t".into(),
            osu_id: 7,
            username: "p".into(),
            server: SERVER.into(),
        }
    }

    #[test]
    fn without_keys_or_session_nothing_is_asked_and_the_hint_says_to_log_in() {
        let (api, hint) = client(None, None, false, None, Notes::default());
        assert!(api.is_none());
        assert_eq!(hint, LOGIN_HINT);
    }

    #[test]
    fn a_saved_session_picks_the_shared_server() {
        let (api, _) = client(None, Some(session()), false, Some(SERVER), Notes::default());
        assert!(api.unwrap().shared());
    }

    #[test]
    fn a_session_from_another_server_is_not_used() {
        let elsewhere = Session {
            server: "https://elsewhere.test".into(),
            ..session()
        };
        let (api, hint) = client(None, Some(elsewhere), false, Some(SERVER), Notes::default());
        assert!(api.is_none());
        assert_eq!(hint, LOGIN_HINT);
    }

    #[test]
    fn keys_still_win_over_a_session() {
        let (own, _) = client(Some(keys()), Some(session()), false, None, Notes::default());
        assert!(!own.unwrap().shared());
        let server = Some(SERVER);
        let (shared, _) = client(
            Some(keys()),
            Some(session()),
            false,
            server,
            Notes::default(),
        );
        assert!(shared.unwrap().shared());
        let (none, hint) = client(Some(keys()), None, false, server, Notes::default());
        assert!(none.is_none());
        assert_eq!(hint, LOGIN_HINT);
    }

    #[test]
    fn offline_goes_nowhere() {
        for (keys, session) in [(Some(keys()), Some(session())), (None, None)] {
            let (api, hint) = client(keys, session, true, None, Notes::default());
            assert!(api.is_none());
            assert_eq!(hint, OFFLINE_HINT);
        }
    }

    struct Redeeming;

    impl Transport for Redeeming {
        fn post_form(&mut self, url: &str, _: &[(&str, &str)]) -> Result<Response, ApiError> {
            panic!("unexpected form to {url}")
        }

        fn get(&mut self, url: &str, _: &str) -> Result<Response, ApiError> {
            panic!("unexpected GET {url}")
        }

        fn post_json(&mut self, url: &str, _: &[u8], _: &str) -> Result<Response, ApiError> {
            assert!(url.ends_with("/v1/auth/token"), "{url}");
            Ok(Response {
                status: 200,
                retry_after: None,
                body: br#"{"token":"t","osu_id":7,"username":"p"}"#.to_vec(),
            })
        }
    }

    #[test]
    fn the_login_waits_for_the_browser_even_while_the_opener_hangs() {
        let (stop, hang) = mpsc::channel::<()>();
        let opener = move |url: &str| {
            let port: u16 = url.rsplit("port=").next().unwrap().parse().unwrap();
            let challenge = url
                .split("challenge=")
                .nth(1)
                .unwrap()
                .split('&')
                .next()
                .unwrap()
                .to_owned();
            thread::spawn(move || {
                let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
                write!(
                    stream,
                    "GET /?code=abc&challenge={challenge} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n"
                )
                .unwrap();
                let mut reply = String::new();
                let _ = stream.read_to_string(&mut reply);
            });
            let _ = hang.recv();
            Ok(())
        };
        let (done, finished) = mpsc::channel();
        thread::spawn(move || {
            let result = log_in_with(Server::new(Redeeming, "https://api.test"), opener);
            let _ = done.send(
                result
                    .map(|session| session.osu_id)
                    .map_err(|error| error.to_string()),
            );
        });
        let outcome = finished.recv_timeout(Duration::from_secs(10));
        drop(stop);
        assert_eq!(outcome, Ok(Ok(7)), "the login waited for the opener");
    }

    #[test]
    fn login_keys_keeps_the_old_prompt() {
        let parsed = Cli::try_parse_from(["lost-scores", "login", "--keys"]).unwrap();
        assert!(matches!(
            parsed.command,
            Some(Command::Login { keys: true })
        ));
        let parsed = Cli::try_parse_from(["lost-scores", "login"]).unwrap();
        assert!(matches!(
            parsed.command,
            Some(Command::Login { keys: false })
        ));
    }
}
