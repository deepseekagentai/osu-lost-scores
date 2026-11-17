use std::path::Path;

use lost_scores_app::pipeline::{Event, Step};
use lost_scores_app::rows::thousands;
use lost_scores_app::{detect, setup};

use crate::log::stamp;

pub const NEEDS_ACCOUNT: &str = "Log in or add your own API keys to start";
pub const LOGIN_EXPIRED: &str = "Your login has expired, log in again";
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
    check_missing: bool,
}

impl ScanState {
    pub fn start(&mut self, check_missing: bool) {
        self.phase = Phase::Scanning;
        self.progress = 0.0;
        self.check_missing = check_missing;
        self.status = "Starting scan...".to_owned();
    }

    pub fn apply(&mut self, event: &Event, clock: (u8, u8, u8)) {
        match event {
            Event::Log(text) => self.log.push(stamp(clock, text)),
            Event::Progress { step, done, total } => {
                self.status = status_text(*step, *done, *total);
                self.progress = self.progress.max(self.position(*step, *done, *total));
            }
        }
    }

    pub fn queued(&mut self, seconds: u64) {
        self.status = format!("Waiting in the shared queue: {seconds} s");
    }

    pub fn saving(&mut self) {
        self.status = "Saving results...".to_owned();
        self.progress = self.progress.max(self.span(Step::Covers).0);
    }

    pub fn finish(&mut self, lost_total: usize, lost_ranked: usize, images: bool) {
        self.phase = Phase::Idle;
        self.progress = 1.0;
        self.status =
            format!("Scan finished: {lost_total} lost plays, {lost_ranked} on ranked maps");
        if images {
            self.status.push_str(" \u{b7} images ready");
        }
    }

    pub fn fail(&mut self, error: &str, clock: (u8, u8, u8)) {
        self.phase = Phase::Failed;
        self.progress = 0.0;
        self.status = "Operation failed".to_owned();
        self.log.push(stamp(clock, &format!("Error: {error}")));
    }

    pub fn expired(&mut self, clock: (u8, u8, u8)) {
        self.fail(LOGIN_EXPIRED, clock);
        self.status = LOGIN_EXPIRED.to_owned();
    }

    pub fn can_start(&self, connected: bool) -> bool {
        connected && self.phase != Phase::Scanning
    }
}

pub fn status_text(step: Step, done: usize, total: usize) -> String {
    let done = thousands(done as f64);
    let total = thousands(total as f64);
    match step {
        Step::Songs => format!("Reading Songs: {done} of {total}"),
        Step::Replays => format!("Reading replays: {done} of {total}"),
        Step::MissingMaps => format!("Looking up missing maps: {done} of {total}"),
        Step::Pp => format!("Calculating pp: {done} of {total}"),
        Step::Catalog | Step::Refresh => format!("Fetching map statuses: {done} of {total}"),
        Step::OnlineTop => "Fetching your top plays...".to_owned(),
        Step::Potential => "Building the potential top...".to_owned(),
        Step::Covers => format!("Preparing covers: {done} of {total}"),
        Step::Images => "Drawing pictures...".to_owned(),
    }
}

const BAR: [(Step, f32); 10] = [
    (Step::Songs, 10.0),
    (Step::Replays, 6.0),
    (Step::MissingMaps, 14.0),
    (Step::Pp, 20.0),
    (Step::Catalog, 14.0),
    (Step::OnlineTop, 6.0),
    (Step::Potential, 4.0),
    (Step::Covers, 14.0),
    (Step::Images, 8.0),
    (Step::Refresh, 4.0),
];

impl ScanState {
    fn span(&self, step: Step) -> (f32, f32) {
        let weight = |candidate: Step, weight: f32| {
            if candidate == Step::MissingMaps && !self.check_missing {
                0.0
            } else {
                weight
            }
        };
        let whole: f32 = BAR.iter().map(|&(step, w)| weight(step, w)).sum();
        let mut start = 0.0;
        for &(candidate, w) in &BAR {
            if candidate == step {
                return (start / whole, weight(candidate, w) / whole);
            }
            start += weight(candidate, w);
        }
        (1.0, 0.0)
    }

    fn position(&self, step: Step, done: usize, total: usize) -> f32 {
        let (start, weight) = self.span(step);
        let fraction = if total == 0 {
            1.0
        } else {
            (done as f32 / total as f32).clamp(0.0, 1.0)
        };
        start + weight * fraction
    }
}

#[cfg(test)]
mod tests {
    use lost_scores_app::pipeline::{Event, Step};

    use super::{Phase, ScanState, check_folders, status_text};

    const CLOCK: (u8, u8, u8) = (9, 30, 5);

    fn progress(step: Step, done: usize, total: usize) -> Event {
        Event::Progress { step, done, total }
    }

    #[test]
    fn a_finished_scan_with_images_says_so() {
        let mut state = ScanState::default();
        state.start(false);
        state.finish(340, 238, true);
        assert_eq!(
            state.status,
            "Scan finished: 340 lost plays, 238 on ranked maps \u{b7} images ready"
        );
    }

    #[test]
    fn a_failed_scan_returns_to_idle_with_the_error_in_the_log() {
        let mut state = ScanState::default();
        state.start(false);
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
            "Calculating pp: 1,798 of 14,982"
        );
        assert_eq!(
            status_text(Step::Songs, 1200, 14000),
            "Reading Songs: 1,200 of 14,000"
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
            "Fetching map statuses: 4 of 190"
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
    fn a_wait_in_the_shared_queue_shows_in_the_status_line() {
        let mut state = ScanState::default();
        state.start(false);
        state.apply(&progress(Step::Catalog, 1, 15), CLOCK);
        let reached = state.progress;
        state.queued(12);
        assert_eq!(state.status, "Waiting in the shared queue: 12 s");
        assert_eq!(state.progress, reached);
        state.apply(&progress(Step::Catalog, 2, 15), CLOCK);
        assert_eq!(state.status, "Fetching map statuses: 2 of 15");
    }

    #[test]
    fn progress_never_goes_back_within_a_scan() {
        let mut state = ScanState::default();
        state.start(false);
        state.apply(&progress(Step::Pp, 50, 100), CLOCK);
        let reached = state.progress;
        state.apply(&progress(Step::Replays, 1, 100), CLOCK);
        assert_eq!(state.progress, reached);
        state.apply(&progress(Step::Pp, 100, 100), CLOCK);
        assert!(state.progress > reached);
    }

    #[test]
    fn step_weights_add_up_to_the_whole_bar() {
        for check_missing in [false, true] {
            let mut state = ScanState::default();
            state.start(check_missing);
            let mut reached = 0.0;
            for step in [
                Step::Songs,
                Step::Replays,
                Step::MissingMaps,
                Step::Pp,
                Step::Catalog,
                Step::OnlineTop,
                Step::Potential,
                Step::Covers,
                Step::Images,
                Step::Refresh,
            ] {
                state.apply(&progress(step, 0, 4), CLOCK);
                assert!(
                    (state.progress - reached).abs() < 1e-6,
                    "{step:?} starts at {}",
                    state.progress
                );
                state.apply(&progress(step, 4, 4), CLOCK);
                reached = state.progress;
            }
            assert!((reached - 1.0).abs() < 1e-6, "{reached}");
        }
    }

    #[test]
    fn missing_maps_take_a_part_of_the_bar_only_when_they_are_looked_up() {
        let mut off = ScanState::default();
        off.start(false);
        off.apply(&progress(Step::Replays, 10, 10), CLOCK);
        let replays = off.progress;
        off.apply(&progress(Step::Pp, 0, 10), CLOCK);
        assert!((off.progress - replays).abs() < 1e-6);
        let mut on = ScanState::default();
        on.start(true);
        on.apply(&progress(Step::Replays, 10, 10), CLOCK);
        let before = on.progress;
        on.apply(&progress(Step::MissingMaps, 5, 10), CLOCK);
        assert!(on.progress > before);
    }

    #[test]
    fn the_bar_fills_only_when_the_pictures_and_statuses_are_done() {
        let mut state = ScanState::default();
        state.start(false);
        state.apply(&progress(Step::Potential, 1, 1), CLOCK);
        let scanned = state.progress;
        assert!(scanned < 0.8, "{scanned}");
        state.saving();
        assert!((state.progress - scanned).abs() < 1e-6);
        state.apply(&progress(Step::Covers, 160, 319), CLOCK);
        let covers = state.progress;
        assert!(covers > scanned && covers < 1.0, "{covers}");
        assert_eq!(state.status, "Preparing covers: 160 of 319");
        state.apply(&progress(Step::Images, 0, 1), CLOCK);
        assert!(state.progress > covers && state.progress < 1.0);
        state.apply(&progress(Step::Refresh, 1, 2), CLOCK);
        assert!(state.progress < 1.0);
        state.finish(339, 239, true);
        assert_eq!(state.progress, 1.0);
    }

    #[test]
    fn an_empty_stage_counts_as_complete_and_never_divides_by_zero() {
        let mut state = ScanState::default();
        state.start(false);
        state.apply(&progress(Step::Pp, 0, 0), CLOCK);
        let empty = state.progress;
        state.apply(&progress(Step::Catalog, 0, 1), CLOCK);
        assert!(empty.is_finite());
        assert!((state.progress - empty).abs() < 1e-6);
        assert_eq!(status_text(Step::Pp, 0, 0), "Calculating pp: 0 of 0");
    }

    #[test]
    fn log_lines_get_a_timestamp() {
        let mut state = ScanState::default();
        state.start(false);
        state.apply(&Event::Log("player: Lemon".into()), CLOCK);
        assert_eq!(state.log, ["[09:30:05] player: Lemon"]);
    }

    #[test]
    fn start_needs_keys() {
        let mut state = ScanState::default();
        assert!(!state.can_start(false));
        assert!(state.can_start(true));
        state.start(false);
        assert_eq!(state.status, "Starting scan...");
        assert!(!state.can_start(true));
    }

    #[test]
    fn image_steps_have_their_own_status() {
        assert_eq!(
            status_text(Step::Covers, 3, 12),
            "Preparing covers: 3 of 12"
        );
        assert_eq!(status_text(Step::Images, 0, 1), "Drawing pictures...");
    }

    #[test]
    fn a_finished_scan_reports_its_counts() {
        let mut state = ScanState::default();
        state.start(false);
        state.finish(340, 238, false);
        assert_eq!(state.phase, Phase::Idle);
        assert_eq!(state.progress, 1.0);
        assert_eq!(
            state.status,
            "Scan finished: 340 lost plays, 238 on ranked maps"
        );
        assert!(state.can_start(true));
    }
}
