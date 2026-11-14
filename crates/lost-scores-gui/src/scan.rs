use std::path::Path;

use lost_scores_app::pipeline::{Event, Step};
use lost_scores_app::rows::thousands;
use lost_scores_app::{detect, setup};

use crate::log::stamp;

pub const NEEDS_KEYS: &str = "Log in or add your own API keys to start";
pub const NEEDS_FOLDER: &str =
    "Could not find your osu! folder: pick it with the button next to the path";

pub fn check_folders(osu_dir: &str) -> Result<(), String> {
    if osu_dir.is_empty() || !detect::is_osu_dir(Path::new(osu_dir)) {
        return Err(NEEDS_FOLDER.to_owned());
    }
    let dir = Path::new(osu_dir);
    let config = setup::read_user_config(dir);
    match setup::songs_dir(dir, &config) {
        Ok(_) => Ok(()),
        Err(_) => Err(format!(
            "The Songs folder set in osu! does not exist: {}",
            config.beatmap_directory.unwrap_or_default()
        )),
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Idle,
    Scanning,
    Failed,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScanState {
    pub phase: Phase,
    pub progress: f32,
    pub status: String,
    pub log: Vec<String>,
}

impl ScanState {
    pub fn start(&mut self) {
        self.phase = Phase::Scanning;
        self.progress = 0.0;
        self.status = "Starting scan...".to_owned();
    }

    pub fn apply(&mut self, event: &Event, clock: (u8, u8, u8)) {
        match event {
            Event::Log(text) => self.log.push(stamp(clock, text)),
            Event::Progress { step, done, total } => {
                self.status = status_text(*step, *done, *total);
                self.progress = self.progress.max(progress_value(*step, *done, *total));
            }
        }
    }

    pub fn saving(&mut self) {
        self.status = "Saving results...".to_owned();
        self.progress = 1.0;
    }

    pub fn finish(&mut self, lost_total: usize, lost_ranked: usize) {
        self.phase = Phase::Idle;
        self.progress = 1.0;
        self.status =
            format!("Scan finished: {lost_total} lost plays, {lost_ranked} on ranked maps");
    }

    pub fn fail(&mut self, error: &str, clock: (u8, u8, u8)) {
        self.phase = Phase::Failed;
        self.progress = 0.0;
        self.status = "Operation failed".to_owned();
        self.log.push(stamp(clock, &format!("Error: {error}")));
    }

    pub fn can_start(&self, connected: bool) -> bool {
        connected && self.phase != Phase::Scanning
    }
}

pub fn status_text(step: Step, done: usize, total: usize) -> String {
    let done = thousands(done as f64);
    let total = thousands(total as f64);
    match step {
        Step::Songs => format!("Reading Songs: {done} of {total} files"),
        Step::Replays => format!("Reading replays: {done} of {total}"),
        Step::MissingMaps => format!("Looking up missing maps: {done} of {total}"),
        Step::Pp => format!("Calculating pp: {done} of {total} replays"),
        Step::Catalog | Step::Refresh => {
            format!("Asking the osu! API about map statuses: {done} of {total} requests")
        }
        Step::OnlineTop => "Fetching your top plays...".to_owned(),
        Step::Potential => "Building the potential top...".to_owned(),
    }
}

fn span(step: Step) -> (f32, f32) {
    match step {
        Step::Songs => (0.0, 0.15),
        Step::Replays => (0.15, 0.10),
        Step::MissingMaps => (0.25, 0.15),
        Step::Pp => (0.40, 0.25),
        Step::Catalog => (0.65, 0.25),
        Step::OnlineTop => (0.90, 0.05),
        Step::Potential => (0.95, 0.05),
        Step::Refresh => (1.0, 0.0),
    }
}

pub fn progress_value(step: Step, done: usize, total: usize) -> f32 {
    let (start, weight) = span(step);
    let fraction = if total == 0 {
        1.0
    } else {
        (done as f32 / total as f32).clamp(0.0, 1.0)
    };
    start + weight * fraction
}

#[cfg(test)]
mod tests {
    use lost_scores_app::pipeline::{Event, Step};

    use super::{Phase, ScanState, check_folders, progress_value, status_text};

    const CLOCK: (u8, u8, u8) = (9, 30, 5);

    fn progress(step: Step, done: usize, total: usize) -> Event {
        Event::Progress { step, done, total }
    }

    #[test]
    fn a_failed_scan_returns_to_idle_with_the_error_in_the_log() {
        let mut state = ScanState::default();
        state.start();
        state.apply(&progress(Step::Pp, 5, 10), CLOCK);
        state.fail("the osu! API answered 401", CLOCK);
        assert_eq!(state.phase, Phase::Failed);
        assert_eq!(state.progress, 0.0);
        assert_eq!(state.status, "Operation failed");
        assert_eq!(
            state.log.last().map(String::as_str),
            Some("[09:30:05] Error: the osu! API answered 401")
        );
        assert!(state.can_start(true));
    }

    #[test]
    fn starting_without_a_valid_folder_shows_a_window_message() {
        let dir = tempfile::tempdir().unwrap();
        let missing = check_folders("").unwrap_err();
        assert!(missing.contains("pick it with the button"), "{missing}");
        let invalid = check_folders(&dir.path().display().to_string()).unwrap_err();
        assert!(invalid.contains("pick it with the button"), "{invalid}");
        std::fs::create_dir_all(dir.path().join("Data").join("r")).unwrap();
        assert_eq!(check_folders(&dir.path().display().to_string()), Ok(()));
        let gone = dir.path().join("elsewhere");
        std::fs::write(
            dir.path().join("osu!.Bob.cfg"),
            format!("BeatmapDirectory = {}\r\n", gone.display()),
        )
        .unwrap();
        let songs = check_folders(&dir.path().display().to_string());
        let songs = songs.unwrap_err();
        assert!(songs.contains("Songs folder"), "{songs}");
        for text in [&missing, &invalid, &songs] {
            assert!(!text.contains("--"), "{text}");
        }
    }

    #[test]
    fn status_texts_use_thousands_separators() {
        assert_eq!(
            status_text(Step::Pp, 1798, 14982),
            "Calculating pp: 1,798 of 14,982 replays"
        );
        assert_eq!(
            status_text(Step::Songs, 1200, 14000),
            "Reading Songs: 1,200 of 14,000 files"
        );
        assert_eq!(
            status_text(Step::Replays, 3, 14982),
            "Reading replays: 3 of 14,982"
        );
        assert_eq!(
            status_text(Step::MissingMaps, 1, 2),
            "Looking up missing maps: 1 of 2"
        );
        assert_eq!(
            status_text(Step::Catalog, 4, 190),
            "Asking the osu! API about map statuses: 4 of 190 requests"
        );
        assert_eq!(
            status_text(Step::OnlineTop, 0, 1),
            "Fetching your top plays..."
        );
        assert_eq!(
            status_text(Step::Potential, 0, 1),
            "Building the potential top..."
        );
    }

    #[test]
    fn progress_never_goes_back_within_a_scan() {
        let mut state = ScanState::default();
        state.start();
        state.apply(&progress(Step::Pp, 50, 100), CLOCK);
        let reached = state.progress;
        state.apply(&progress(Step::Replays, 1, 100), CLOCK);
        assert_eq!(state.progress, reached);
        state.apply(&progress(Step::Pp, 100, 100), CLOCK);
        assert!(state.progress > reached);
    }

    #[test]
    fn step_weights_add_up_to_the_whole_bar() {
        assert_eq!(progress_value(Step::Songs, 0, 10), 0.0);
        assert!((progress_value(Step::Songs, 10, 10) - 0.15).abs() < 1e-6);
        assert!((progress_value(Step::Replays, 10, 10) - 0.25).abs() < 1e-6);
        assert!((progress_value(Step::MissingMaps, 10, 10) - 0.40).abs() < 1e-6);
        assert!((progress_value(Step::Pp, 10, 10) - 0.65).abs() < 1e-6);
        assert!((progress_value(Step::Catalog, 10, 10) - 0.90).abs() < 1e-6);
        assert!((progress_value(Step::OnlineTop, 1, 1) - 0.95).abs() < 1e-6);
        assert!((progress_value(Step::Potential, 1, 1) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn an_empty_stage_counts_as_complete_and_never_divides_by_zero() {
        let value = progress_value(Step::Pp, 0, 0);
        assert!(value.is_finite());
        assert!((value - 0.65).abs() < 1e-6);
        assert_eq!(
            status_text(Step::Pp, 0, 0),
            "Calculating pp: 0 of 0 replays"
        );
    }

    #[test]
    fn log_lines_get_a_timestamp() {
        let mut state = ScanState::default();
        state.start();
        state.apply(&Event::Log("player: Lemon".into()), CLOCK);
        assert_eq!(state.log, ["[09:30:05] player: Lemon"]);
    }

    #[test]
    fn start_needs_keys() {
        let mut state = ScanState::default();
        assert!(!state.can_start(false));
        assert!(state.can_start(true));
        state.start();
        assert_eq!(state.status, "Starting scan...");
        assert!(!state.can_start(true));
    }

    #[test]
    fn a_finished_scan_reports_its_counts() {
        let mut state = ScanState::default();
        state.start();
        state.finish(340, 238);
        assert_eq!(state.phase, Phase::Idle);
        assert_eq!(state.progress, 1.0);
        assert_eq!(
            state.status,
            "Scan finished: 340 lost plays, 238 on ranked maps"
        );
        assert!(state.can_start(true));
    }
}
