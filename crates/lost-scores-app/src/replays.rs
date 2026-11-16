use std::fs;
use std::path::{Path, PathBuf};

use osu_api::{ApiError, MAX_REPLAY};
use osu_formats::ReplayHeader;

use crate::history::{SavedScan, Sent};
use crate::rows::printable;

#[derive(Debug)]
pub enum Outcome {
    Sent { row: usize },
    Unreadable { row: usize, reason: String },
    Refused { row: usize, error: ApiError },
    Waiting { error: ApiError },
    NoFolder { dir: PathBuf },
}

fn waits(error: &ApiError) -> bool {
    matches!(
        error,
        ApiError::ReplaysPaused { .. }
            | ApiError::ReplayQuota
            | ApiError::Network(_)
            | ApiError::Server(_)
            | ApiError::Busy { .. }
            | ApiError::BusyToday
            | ApiError::LoginRequired
            | ApiError::ServerStatus { .. }
    )
}

fn readable(path: &Path) -> Result<Vec<u8>, String> {
    let bytes =
        fs::read(path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    if bytes.len() > MAX_REPLAY {
        return Err(format!(
            "{} is larger than the 4 MB a replay may have",
            path.display()
        ));
    }
    ReplayHeader::check(&bytes).map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(bytes)
}

const PP_MATCH: f64 = 1e-6;

pub type Upload<'a> = dyn FnMut(&str, usize, &[u8]) -> Result<String, ApiError> + 'a;

pub fn in_top(scan: &SavedScan) -> Vec<usize> {
    let Some(top) = &scan.potential else {
        return Vec::new();
    };
    let mut rows: Vec<usize> = top
        .rows
        .iter()
        .filter(|place| place.lost)
        .filter_map(|place| {
            scan.lost.iter().position(|lost| {
                lost.beatmap_id == Some(place.beatmap_id) && (lost.pp - place.pp).abs() <= PP_MATCH
            })
        })
        .collect();
    rows.sort_unstable();
    rows.dedup();
    rows
}

pub fn send_queued(
    upload: &mut Upload<'_>,
    sent: &mut Sent,
    scan: &SavedScan,
    replays_dir: &Path,
) -> Vec<Outcome> {
    if !sent.queued.is_empty() && !replays_dir.is_dir() {
        return vec![Outcome::NoFolder {
            dir: replays_dir.to_path_buf(),
        }];
    }
    let mut outcomes = Vec::new();
    for row in sent.queued.clone() {
        let Some(lost) = scan.lost.get(row) else {
            sent.queued.remove(&row);
            continue;
        };
        let bytes = match readable(&replays_dir.join(&lost.file)) {
            Ok(bytes) => bytes,
            Err(reason) => {
                sent.queued.remove(&row);
                outcomes.push(Outcome::Unreadable { row, reason });
                continue;
            }
        };
        match upload(&sent.id, row, &bytes) {
            Ok(sha256) => {
                sent.queued.remove(&row);
                sent.replays.insert(row, sha256);
                outcomes.push(Outcome::Sent { row });
            }
            Err(error) if waits(&error) => {
                outcomes.push(Outcome::Waiting { error });
                break;
            }
            Err(ApiError::NotOnServer) => {
                sent.queued.clear();
                outcomes.push(Outcome::Refused {
                    row,
                    error: ApiError::NotOnServer,
                });
                break;
            }
            Err(error) => {
                sent.queued.remove(&row);
                outcomes.push(Outcome::Refused { row, error });
            }
        }
    }
    outcomes
}

pub fn describe(outcome: &Outcome, scan: &SavedScan) -> String {
    let play = |row: usize| {
        scan.lost.get(row).map_or_else(
            || format!("play {row}"),
            |lost| {
                format!(
                    "{} - {} [{}]",
                    printable(&lost.artist),
                    printable(&lost.title),
                    printable(&lost.version)
                )
            },
        )
    };
    match outcome {
        Outcome::Sent { row } => format!("sent the replay of {}", play(*row)),
        Outcome::Unreadable { row, reason } => {
            format!("kept back the replay of {}: {reason}", play(*row))
        }
        Outcome::Refused { row, error } => {
            format!("the server refused the replay of {}: {error}", play(*row))
        }
        Outcome::Waiting { error } => error.to_string(),
        Outcome::NoFolder { dir } => {
            format!("kept the replays queued: {} is not there", dir.display())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::fs;
    use std::path::Path;

    use lost_scores_core::grade::Grade;
    use lost_scores_core::maps::MapStatus;
    use osu_api::{ApiError, Submission};

    use super::{Outcome, describe, in_top, send_queued};
    use crate::history::{SavedScan, Sent, VERSION};
    use crate::pipeline::PotentialResult;
    use crate::rows::{Hits, LostRow, PotentialRow};

    struct Uploads {
        answers: VecDeque<Result<String, ApiError>>,
        calls: Vec<String>,
    }

    impl Uploads {
        fn new(answers: Vec<Result<&str, ApiError>>) -> Self {
            Self {
                answers: answers
                    .into_iter()
                    .map(|answer| answer.map(str::to_owned))
                    .collect(),
                calls: Vec::new(),
            }
        }

        fn send(&mut self, record: &mut Sent, saved: &SavedScan, dir: &Path) -> Vec<Outcome> {
            send_queued(
                &mut |id: &str, row: usize, bytes: &[u8]| {
                    self.calls.push(format!("{id} {row} {}", bytes.len()));
                    self.answers.pop_front().expect("no scripted answer")
                },
                record,
                saved,
                dir,
            )
        }
    }

    fn row(file: &str) -> LostRow {
        LostRow {
            pp: 321.5,
            kept_pp: 300.25,
            score: 1_234_567,
            kept_score: 1_300_000,
            mods: "HDDT".into(),
            unix_time: 1_700_000_000,
            played: "2023-11-14 22:13".into(),
            status: MapStatus::Ranked,
            beatmap_md5: "0b577cbe9bc774600d20cfa17c0aa94c".into(),
            beatmap_id: Some(11),
            beatmapset_id: None,
            artist: "A".into(),
            title: "B".into(),
            version: "x".into(),
            file: file.into(),
            creator: "m".into(),
            hits: Hits {
                n300: 300,
                n100: 20,
                n50: 1,
                misses: 2,
            },
            accuracy: 96.5,
            grade: Grade::S,
            max_combo: 500,
        }
    }

    fn scan(files: &[&str]) -> SavedScan {
        SavedScan {
            version: VERSION,
            made_at: 1_790_000_000,
            player_id: 7,
            player_name: "Lemon".into(),
            include_all: false,
            check_missing: false,
            lost: files.iter().map(|file| row(file)).collect(),
            lost_total: files.len(),
            lost_ranked: files.len(),
            potential: None,
        }
    }

    fn replay() -> Vec<u8> {
        let mut out = vec![0];
        out.extend(20210520i32.to_le_bytes());
        for text in ["0b577cbe9bc774600d20cfa17c0aa94c", "Lemon", ""] {
            if text.is_empty() {
                out.push(0x00);
            } else {
                out.push(0x0b);
                out.push(u8::try_from(text.len()).unwrap());
                out.extend(text.as_bytes());
            }
        }
        for count in [300u16, 20, 1, 0, 0, 2] {
            out.extend(count.to_le_bytes());
        }
        out.extend(1_234_567u32.to_le_bytes());
        out.extend(500u16.to_le_bytes());
        out.push(0);
        out.extend(72u32.to_le_bytes());
        out.push(0x00);
        out.extend(638_357_968_000_000_000i64.to_le_bytes());
        out.extend(0i32.to_le_bytes());
        out.extend(1i64.to_le_bytes());
        out
    }

    fn sent(queued: &[usize]) -> Sent {
        let answer = Submission {
            id: "abc".into(),
            visibility: "private".into(),
            status: "unverified".into(),
        };
        let mut sent = Sent::new(&answer, "https://api.test");
        sent.queued.extend(queued.iter().copied());
        sent
    }

    fn files(dir: &Path, names: &[(&str, Vec<u8>)]) {
        for (name, bytes) in names {
            fs::write(dir.join(name), bytes).unwrap();
        }
    }

    #[test]
    fn each_outcome_is_told_with_its_play() {
        let saved = scan(&["a.osr"]);
        let told = [
            Outcome::Sent { row: 0 },
            Outcome::Unreadable {
                row: 0,
                reason: "a.osr: replay ends too early".into(),
            },
            Outcome::Refused {
                row: 0,
                error: ApiError::ReplayMismatch,
            },
            Outcome::Waiting {
                error: ApiError::ReplaysPaused { retry_after: 60 },
            },
        ]
        .iter()
        .map(|outcome| describe(outcome, &saved))
        .collect::<Vec<_>>();
        assert_eq!(
            told,
            [
                "sent the replay of A - B [x]",
                "kept back the replay of A - B [x]: a.osr: replay ends too early",
                "the server refused the replay of A - B [x]: this replay is not the lost play it was chosen for",
                "the Lost Scores server takes no new replays right now; they will go out later",
            ]
        );
    }

    #[test]
    fn the_chosen_replays_are_sent_and_remembered() {
        let dir = tempfile::tempdir().unwrap();
        files(dir.path(), &[("a.osr", replay()), ("b.osr", replay())]);
        let mut uploads = Uploads::new(vec![Ok("aa"), Ok("bb")]);
        let mut record = sent(&[0, 1]);
        let outcomes = uploads.send(&mut record, &scan(&["a.osr", "b.osr"]), dir.path());
        assert!(matches!(
            outcomes[..],
            [Outcome::Sent { row: 0 }, Outcome::Sent { row: 1 }]
        ));
        assert_eq!(
            record.replays.into_iter().collect::<Vec<_>>(),
            [(0, "aa".to_owned()), (1, "bb".to_owned())]
        );
        assert!(record.queued.is_empty());
        let size = replay().len();
        assert_eq!(
            uploads.calls,
            [format!("abc 0 {size}"), format!("abc 1 {size}")]
        );
    }

    #[test]
    fn a_paused_upload_stays_queued_and_goes_out_later() {
        let dir = tempfile::tempdir().unwrap();
        files(
            dir.path(),
            &[
                ("a.osr", replay()),
                ("b.osr", replay()),
                ("c.osr", replay()),
            ],
        );
        let saved = scan(&["a.osr", "b.osr", "c.osr"]);
        let mut record = sent(&[0, 1, 2]);
        let mut uploads = Uploads::new(vec![
            Ok("aa"),
            Err(ApiError::ReplaysPaused { retry_after: 3600 }),
        ]);
        let outcomes = uploads.send(&mut record, &saved, dir.path());
        assert!(matches!(
            outcomes[..],
            [
                Outcome::Sent { row: 0 },
                Outcome::Waiting {
                    error: ApiError::ReplaysPaused { retry_after: 3600 }
                }
            ]
        ));
        assert_eq!(record.queued.iter().copied().collect::<Vec<_>>(), [1, 2]);
        let mut later = Uploads::new(vec![Ok("bb"), Ok("cc")]);
        later.send(&mut record, &saved, dir.path());
        assert!(record.queued.is_empty());
        assert_eq!(record.replays.len(), 3);
    }

    #[test]
    fn a_file_the_app_cannot_read_is_never_sent() {
        let dir = tempfile::tempdir().unwrap();
        let mut broken = replay();
        broken.push(0);
        files(
            dir.path(),
            &[("b.osr", broken), ("c.osr", vec![0; 4 * 1024 * 1024 + 1])],
        );
        let mut uploads = Uploads::new(Vec::new());
        let mut record = sent(&[0, 1, 2]);
        let outcomes = uploads.send(&mut record, &scan(&["a.osr", "b.osr", "c.osr"]), dir.path());
        assert!(matches!(
            outcomes[..],
            [
                Outcome::Unreadable { row: 0, .. },
                Outcome::Unreadable { row: 1, .. },
                Outcome::Unreadable { row: 2, .. }
            ]
        ));
        assert!(record.queued.is_empty() && record.replays.is_empty());
        assert!(uploads.calls.is_empty());
    }

    #[test]
    fn the_queue_waits_while_the_replays_folder_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("Data").join("r");
        let mut uploads = Uploads::new(Vec::new());
        let mut record = sent(&[0, 1]);
        let saved = scan(&["a.osr", "b.osr"]);
        let outcomes = uploads.send(&mut record, &saved, &missing);
        assert!(matches!(outcomes[..], [Outcome::NoFolder { .. }]));
        assert_eq!(record.queued.iter().copied().collect::<Vec<_>>(), [0, 1]);
        assert!(uploads.calls.is_empty());
        assert_eq!(
            describe(&outcomes[0], &saved),
            format!(
                "kept the replays queued: {} is not there",
                missing.display()
            )
        );
    }

    #[test]
    fn a_refused_replay_is_reported_with_the_servers_reason() {
        let dir = tempfile::tempdir().unwrap();
        files(dir.path(), &[("a.osr", replay()), ("b.osr", replay())]);
        let mut uploads = Uploads::new(vec![
            Err(ApiError::ReplayMismatch),
            Err(ApiError::NotOnServer),
        ]);
        let mut record = sent(&[0, 1, 2]);
        let outcomes = uploads.send(&mut record, &scan(&["a.osr", "b.osr", "c.osr"]), dir.path());
        assert!(matches!(
            outcomes[..],
            [
                Outcome::Refused {
                    row: 0,
                    error: ApiError::ReplayMismatch
                },
                Outcome::Refused {
                    row: 1,
                    error: ApiError::NotOnServer
                }
            ]
        ));
        assert!(record.queued.is_empty() && record.replays.is_empty());
        assert_eq!(uploads.calls.len(), 2);
    }

    #[test]
    fn the_plays_in_the_top_are_the_lost_places_of_the_potential_top() {
        let mut saved = scan(&["a.osr", "b.osr", "c.osr"]);
        saved.lost[1].beatmap_id = Some(12);
        saved.lost[1].pp = 200.0;
        saved.lost[2].beatmap_id = Some(13);
        let place = |pp: f64, lost: bool, beatmap_id: u32| PotentialRow {
            place: 1,
            pp,
            weight: 1.0,
            weighted: pp,
            lost,
            beatmap_id,
            beatmapset_id: None,
            artist: String::new(),
            title: String::new(),
            version: String::new(),
            creator: String::new(),
            mods: String::new(),
            accuracy: 0.0,
            unix_time: 0,
            played: String::new(),
            hits: Hits {
                n300: 0,
                n100: 0,
                n50: 0,
                misses: 0,
            },
            score: 0,
            grade: Grade::S,
            status: MapStatus::Ranked,
            kept_pp: None,
        };
        assert!(in_top(&saved).is_empty());
        saved.potential = Some(PotentialResult {
            current_pp: 1000.0,
            potential_pp: 1100.0,
            current_accuracy: 98.0,
            potential_accuracy: 98.0,
            global_rank: None,
            lost_in_top: 2,
            rows: vec![
                place(400.0, false, 99),
                place(321.5, true, 11),
                place(200.0, true, 12),
                place(150.0, false, 13),
            ],
            online: Vec::new(),
        });
        assert_eq!(in_top(&saved), [0, 1]);
    }
}
